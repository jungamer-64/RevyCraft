mod plugin_host;

use bytes::BytesMut;
use mc_core::{
    ConnectionId, CoreCommand, CoreConfig, CoreEvent, EventTarget, PlayerId, PlayerSummary,
    PluginGenerationId, ServerCore, SessionCapabilitySet, TargetedEvent,
};
use mc_plugin_api::PluginAbiVersion;
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeNextState, HandshakeProbe, LoginRequest, MinecraftWireCodec,
    PacketWriter, PlayEncodingContext, ProtocolAdapter, ProtocolError, ServerListStatus,
    StatusRequest, StorageAdapter, TransportKind, WireCodec,
};
use md5::{Digest, Md5};
pub use plugin_host::{
    InProcessProtocolPlugin, PluginAbiRange, PluginCatalog, PluginFailurePolicy, PluginHost,
    plugin_host_from_config, plugin_reload_poll_interval_ms,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use uuid::Uuid;

const BE_PLACEHOLDER_ADAPTER_ID: &str = "be-placeholder";

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("plugin load error: {0}")]
    PluginLoad(#[from] libloading::Error),
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("storage error: {0}")]
    Storage(#[from] mc_proto_common::StorageError),
    #[error("unsupported configuration: {0}")]
    Unsupported(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
}

pub trait Authenticator: Send + Sync {
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the username cannot be authenticated under
    /// the current authentication mode.
    fn authenticate(&self, username: &str) -> Result<PlayerId, RuntimeError>;
}

#[derive(Default)]
pub struct OfflineAuthenticator;

impl Authenticator for OfflineAuthenticator {
    fn authenticate(&self, username: &str) -> Result<PlayerId, RuntimeError> {
        let mut hasher = Md5::new();
        hasher.update(format!("OfflinePlayer:{username}").as_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest);
        bytes[6] = (bytes[6] & 0x0f) | 0x30;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Ok(PlayerId(Uuid::from_bytes(bytes)))
    }
}

pub struct OnlineAuthenticator;

impl Authenticator for OnlineAuthenticator {
    fn authenticate(&self, _username: &str) -> Result<PlayerId, RuntimeError> {
        Err(RuntimeError::Unsupported(
            "online-mode is not implemented yet".to_string(),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LevelType {
    Flat,
}

impl LevelType {
    fn parse(value: &str) -> Result<Self, RuntimeError> {
        if value.eq_ignore_ascii_case("flat") {
            Ok(Self::Flat)
        } else {
            Err(RuntimeError::Unsupported(format!(
                "level-type={value} is not supported; only FLAT is implemented"
            )))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub server_ip: Option<IpAddr>,
    pub server_port: u16,
    pub be_enabled: bool,
    pub motd: String,
    pub max_players: u8,
    pub online_mode: bool,
    pub level_name: String,
    pub level_type: LevelType,
    pub game_mode: u8,
    pub difficulty: u8,
    pub view_distance: u8,
    pub default_adapter: String,
    pub enabled_adapters: Option<Vec<String>>,
    pub storage_profile: String,
    pub plugins_dir: PathBuf,
    pub plugin_allowlist: Option<Vec<String>>,
    pub plugin_failure_policy: PluginFailurePolicy,
    pub plugin_reload_watch: bool,
    pub plugin_abi_min: PluginAbiVersion,
    pub plugin_abi_max: PluginAbiVersion,
    pub world_dir: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            server_ip: None,
            server_port: 25565,
            be_enabled: false,
            motd: "Multi-version Rust server".to_string(),
            max_players: 20,
            online_mode: false,
            level_name: "world".to_string(),
            level_type: LevelType::Flat,
            game_mode: 0,
            difficulty: 1,
            view_distance: 2,
            default_adapter: "je-1_7_10".to_string(),
            enabled_adapters: None,
            storage_profile: "je-anvil-1_7_10".to_string(),
            plugins_dir: cwd.join("dist").join("plugins"),
            plugin_allowlist: None,
            plugin_failure_policy: PluginFailurePolicy::Quarantine,
            plugin_reload_watch: false,
            plugin_abi_min: PluginAbiVersion { major: 1, minor: 0 },
            plugin_abi_max: PluginAbiVersion { major: 1, minor: 0 },
            world_dir: cwd.join("world"),
        }
    }
}

impl ServerConfig {
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when `server.properties` cannot be read or
    /// parsed, or when it contains unsupported configuration values.
    pub fn from_properties(path: &Path) -> Result<Self, RuntimeError> {
        let mut config = Self::default();
        if path.exists() {
            let contents = fs::read_to_string(path)?;
            for raw_line in contents.lines() {
                let line = raw_line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let value = value.trim();
                match key.trim() {
                    "server-ip" => {
                        if value.is_empty() {
                            config.server_ip = None;
                        } else {
                            config.server_ip = Some(value.parse().map_err(|_| {
                                RuntimeError::Config("invalid server-ip".to_string())
                            })?);
                        }
                    }
                    "server-port" => {
                        config.server_port = value
                            .parse()
                            .map_err(|_| RuntimeError::Config("invalid server-port".to_string()))?;
                    }
                    "be-enabled" => {
                        config.be_enabled = value.eq_ignore_ascii_case("true");
                    }
                    "motd" => config.motd = value.to_string(),
                    "max-players" => {
                        config.max_players = value
                            .parse()
                            .map_err(|_| RuntimeError::Config("invalid max-players".to_string()))?;
                    }
                    "online-mode" => {
                        config.online_mode = value.eq_ignore_ascii_case("true");
                    }
                    "level-name" => config.level_name = value.to_string(),
                    "level-type" => {
                        config.level_type = LevelType::parse(value)?;
                    }
                    "gamemode" => {
                        config.game_mode = value
                            .parse()
                            .map_err(|_| RuntimeError::Config("invalid gamemode".to_string()))?;
                    }
                    "difficulty" => {
                        config.difficulty = value
                            .parse()
                            .map_err(|_| RuntimeError::Config("invalid difficulty".to_string()))?;
                    }
                    "view-distance" => {
                        config.view_distance = value.parse().map_err(|_| {
                            RuntimeError::Config("invalid view-distance".to_string())
                        })?;
                    }
                    "default-adapter" => {
                        config.default_adapter = value.to_string();
                    }
                    "enabled-adapters" => {
                        config.enabled_adapters = parse_enabled_adapters(value)?;
                    }
                    "storage-profile" => {
                        config.storage_profile = value.to_string();
                    }
                    "plugins-dir" => {
                        config.plugins_dir = PathBuf::from(value);
                    }
                    "plugin-allowlist" => {
                        config.plugin_allowlist = parse_enabled_adapters(value)?;
                    }
                    "plugin-failure-policy" => {
                        config.plugin_failure_policy = PluginFailurePolicy::parse(value)?;
                    }
                    "plugin-reload-watch" => {
                        config.plugin_reload_watch = value.eq_ignore_ascii_case("true");
                    }
                    "plugin-abi-min" => {
                        config.plugin_abi_min = PluginAbiRange::parse_version(value)?;
                    }
                    "plugin-abi-max" => {
                        config.plugin_abi_max = PluginAbiRange::parse_version(value)?;
                    }
                    unknown => {
                        eprintln!("warning: ignoring unknown server.properties key `{unknown}`");
                    }
                }
            }
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        config.world_dir = parent.join(&config.level_name);
        if config.plugins_dir.is_relative() {
            config.plugins_dir = parent.join(&config.plugins_dir);
        }
        Ok(config)
    }

    #[must_use]
    pub fn bind_addr(&self) -> SocketAddr {
        SocketAddr::new(
            self.server_ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            self.server_port,
        )
    }

    fn effective_enabled_adapters(&self) -> Vec<String> {
        match &self.enabled_adapters {
            Some(enabled_adapters) => enabled_adapters.clone(),
            None => vec![self.default_adapter.clone()],
        }
    }
}

fn parse_enabled_adapters(value: &str) -> Result<Option<Vec<String>>, RuntimeError> {
    let adapters = value
        .split(',')
        .map(str::trim)
        .filter(|adapter_id| !adapter_id.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if adapters.is_empty() {
        return Ok(None);
    }
    Ok(Some(adapters))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenerBinding {
    pub transport: TransportKind,
    pub local_addr: SocketAddr,
    pub adapter_ids: Vec<String>,
}

#[derive(Clone, Default)]
pub struct ProtocolRegistry {
    adapters_by_id: HashMap<String, Arc<dyn ProtocolAdapter>>,
    adapters_by_route: HashMap<(TransportKind, Edition, i32), Arc<dyn ProtocolAdapter>>,
    probes: Vec<Arc<dyn HandshakeProbe>>,
}

impl ProtocolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_adapter(&mut self, adapter: Arc<dyn ProtocolAdapter>) -> &mut Self {
        let descriptor = adapter.descriptor();
        self.adapters_by_route.insert(
            (
                descriptor.transport,
                descriptor.edition,
                descriptor.protocol_number,
            ),
            Arc::clone(&adapter),
        );
        self.adapters_by_id
            .insert(descriptor.adapter_id.to_string(), adapter);
        self
    }

    pub fn register_probe(&mut self, probe: Arc<dyn HandshakeProbe>) -> &mut Self {
        self.probes.push(probe);
        self
    }

    #[must_use]
    pub fn resolve_adapter(&self, adapter_id: &str) -> Option<Arc<dyn ProtocolAdapter>> {
        self.adapters_by_id.get(adapter_id).cloned()
    }

    #[must_use]
    pub fn resolve_route(
        &self,
        transport_kind: TransportKind,
        edition: Edition,
        protocol_number: i32,
    ) -> Option<Arc<dyn ProtocolAdapter>> {
        self.adapters_by_route
            .get(&(transport_kind, edition, protocol_number))
            .cloned()
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when `enabled_adapters` contains duplicates or
    /// unknown adapter identifiers.
    pub fn filter_enabled(&self, enabled_adapters: &[String]) -> Result<Self, RuntimeError> {
        let mut filtered = Self::new();
        let mut seen = HashSet::new();
        for adapter_id in enabled_adapters {
            if !seen.insert(adapter_id.clone()) {
                return Err(RuntimeError::Config(format!(
                    "enabled-adapters contains duplicate adapter `{adapter_id}`"
                )));
            }
            let Some(adapter) = self.resolve_adapter(adapter_id) else {
                return Err(RuntimeError::Config(format!(
                    "enabled-adapters contains unknown adapter `{adapter_id}`"
                )));
            };
            filtered.register_adapter(adapter);
        }
        filtered.probes = self.probes.clone();
        Ok(filtered)
    }

    #[must_use]
    pub fn adapter_ids_for_transport(&self, transport_kind: TransportKind) -> Vec<String> {
        let mut adapter_ids = self
            .adapters_by_id
            .iter()
            .filter(|(_, adapter)| adapter.descriptor().transport == transport_kind)
            .map(|(adapter_id, _)| adapter_id.clone())
            .collect::<Vec<_>>();
        adapter_ids.sort();
        adapter_ids
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when a registered probe matches the frame's
    /// protocol family but the payload is malformed for that family.
    pub fn route_handshake(
        &self,
        transport_kind: TransportKind,
        frame: &[u8],
    ) -> Result<Option<mc_proto_common::HandshakeIntent>, ProtocolError> {
        for probe in &self.probes {
            if probe.transport_kind() != transport_kind {
                continue;
            }
            if let Some(intent) = probe.try_route(frame)? {
                return Ok(Some(intent));
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Default)]
pub struct StorageRegistry {
    profiles: HashMap<String, Arc<dyn StorageAdapter>>,
}

impl StorageRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_profile(
        &mut self,
        storage_profile: impl Into<String>,
        adapter: Arc<dyn StorageAdapter>,
    ) -> &mut Self {
        self.profiles.insert(storage_profile.into(), adapter);
        self
    }

    #[must_use]
    pub fn resolve(&self, storage_profile: &str) -> Option<Arc<dyn StorageAdapter>> {
        self.profiles.get(storage_profile).cloned()
    }
}

#[derive(Clone, Default)]
pub struct RuntimeRegistries {
    protocols: ProtocolRegistry,
    storage: StorageRegistry,
    plugin_host: Option<Arc<PluginHost>>,
}

impl RuntimeRegistries {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_adapter(&mut self, adapter: Arc<dyn ProtocolAdapter>) -> &mut Self {
        self.protocols.register_adapter(adapter);
        self
    }

    pub fn register_probe(&mut self, probe: Arc<dyn HandshakeProbe>) -> &mut Self {
        self.protocols.register_probe(probe);
        self
    }

    pub fn register_storage_profile(
        &mut self,
        storage_profile: impl Into<String>,
        adapter: Arc<dyn StorageAdapter>,
    ) -> &mut Self {
        self.storage.register_profile(storage_profile, adapter);
        self
    }

    pub fn attach_plugin_host(&mut self, plugin_host: Arc<PluginHost>) -> &mut Self {
        self.plugin_host = Some(plugin_host);
        self
    }

    #[must_use]
    pub fn plugin_host(&self) -> Option<Arc<PluginHost>> {
        self.plugin_host.clone()
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_je_1_7_10() -> Self {
        let mut registries = Self::new();
        use mc_proto_je_1_7_10::{
            JE_1_7_10_STORAGE_PROFILE_ID, Je1710Adapter, Je1710StorageAdapter,
        };
        let adapter = Arc::new(Je1710Adapter::new());
        registries.register_adapter(adapter.clone());
        registries.register_probe(adapter);
        registries
            .register_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID, Arc::new(Je1710StorageAdapter));
        registries
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_builtin_adapters() -> Self {
        let mut registries = Self::with_je_1_7_10();
        use mc_proto_be_placeholder::BePlaceholderAdapter;
        use mc_proto_je_1_12_2::Je1122Adapter;
        use mc_proto_je_1_8_x::Je18xAdapter;
        let adapter = Arc::new(Je18xAdapter::new());
        registries.register_adapter(adapter.clone());
        registries.register_probe(adapter);
        let adapter = Arc::new(Je1122Adapter::new());
        registries.register_adapter(adapter.clone());
        registries.register_probe(adapter);
        let adapter = Arc::new(BePlaceholderAdapter::new());
        registries.register_adapter(adapter.clone());
        registries.register_probe(adapter);
        registries
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_je_and_be_placeholder() -> Self {
        Self::with_builtin_adapters()
    }

    #[must_use]
    pub const fn protocols(&self) -> &ProtocolRegistry {
        &self.protocols
    }

    #[must_use]
    pub const fn storage(&self) -> &StorageRegistry {
        &self.storage
    }
}

pub struct RunningServer {
    listener_bindings: Vec<ListenerBinding>,
    plugin_host: Option<Arc<PluginHost>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<Result<(), RuntimeError>>,
}

impl RunningServer {
    #[must_use]
    pub fn listener_bindings(&self) -> &[ListenerBinding] {
        &self.listener_bindings
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a loaded protocol plugin cannot be
    /// reloaded successfully.
    pub fn reload_plugins(&self) -> Result<Vec<String>, RuntimeError> {
        match &self.plugin_host {
            Some(plugin_host) => plugin_host.reload_modified(),
            None => Ok(Vec::new()),
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the server task fails while shutting down.
    pub async fn shutdown(mut self) -> Result<(), RuntimeError> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.join_handle.await?
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ListenerPlan {
    transport: TransportKind,
    bind_addr: SocketAddr,
    adapter_ids: Vec<String>,
}

struct AcceptedTransportSession {
    transport: TransportKind,
    io: TransportSessionIo,
}

enum TransportSessionIo {
    Tcp(TcpStream),
}

impl TransportSessionIo {
    async fn read_into(&mut self, buffer: &mut BytesMut) -> Result<usize, std::io::Error> {
        match self {
            Self::Tcp(stream) => stream.read_buf(buffer).await,
        }
    }

    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), std::io::Error> {
        match self {
            Self::Tcp(stream) => stream.write_all(bytes).await,
        }
    }
}

enum BoundTransportListener {
    Tcp {
        listener: TcpListener,
        adapter_ids: Vec<String>,
    },
    Udp {
        socket: UdpSocket,
        adapter_ids: Vec<String>,
    },
}

impl BoundTransportListener {
    fn listener_binding(&self) -> Result<ListenerBinding, std::io::Error> {
        match self {
            Self::Tcp {
                listener,
                adapter_ids,
            } => Ok(ListenerBinding {
                transport: TransportKind::Tcp,
                local_addr: listener.local_addr()?,
                adapter_ids: adapter_ids.clone(),
            }),
            Self::Udp {
                socket,
                adapter_ids,
            } => Ok(ListenerBinding {
                transport: TransportKind::Udp,
                local_addr: socket.local_addr()?,
                adapter_ids: adapter_ids.clone(),
            }),
        }
    }
}

#[derive(Clone)]
struct SessionHandle {
    tx: mpsc::UnboundedSender<SessionMessage>,
    player_id: Option<PlayerId>,
}

#[derive(Clone, Debug)]
enum SessionMessage {
    Event(CoreEvent),
}

struct SessionState {
    transport: TransportKind,
    phase: ConnectionPhase,
    adapter: Option<Arc<dyn ProtocolAdapter>>,
    player_id: Option<PlayerId>,
    entity_id: Option<mc_core::EntityId>,
    plugin_generation_id: Option<PluginGenerationId>,
    session_capabilities: Option<SessionCapabilitySet>,
}

struct RuntimeState {
    core: ServerCore,
    dirty: bool,
}

struct RuntimeServer {
    config: ServerConfig,
    protocol_registry: ProtocolRegistry,
    plugin_host: Option<Arc<PluginHost>>,
    default_adapter: Arc<dyn ProtocolAdapter>,
    authenticator: Arc<dyn Authenticator>,
    storage_adapter: Arc<dyn StorageAdapter>,
    state: Mutex<RuntimeState>,
    sessions: Mutex<HashMap<ConnectionId, SessionHandle>>,
    next_connection_id: Mutex<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UdpDatagramAction {
    Ignore,
    UnsupportedBedrock,
}

fn classify_udp_datagram(
    protocol_registry: &ProtocolRegistry,
    datagram: &[u8],
) -> Result<UdpDatagramAction, ProtocolError> {
    match protocol_registry.route_handshake(TransportKind::Udp, datagram)? {
        Some(intent) if intent.edition == Edition::Be => Ok(UdpDatagramAction::UnsupportedBedrock),
        Some(_) | None => Ok(UdpDatagramAction::Ignore),
    }
}

impl RuntimeServer {
    fn refresh_session_capabilities(session: &mut SessionState) {
        let Some(adapter) = session.adapter.as_ref() else {
            session.plugin_generation_id = None;
            session.session_capabilities = None;
            return;
        };
        let plugin_generation = adapter.plugin_generation_id();
        session.plugin_generation_id = plugin_generation;
        session.session_capabilities = Some(SessionCapabilitySet {
            protocol: adapter.capability_set(),
            gameplay_profile: mc_core::GameplayProfileId::new("canonical"),
            plugin_generation,
        });
    }

    async fn spawn_session(self: &Arc<Self>, transport_session: AcceptedTransportSession) {
        let connection_id = {
            let mut next_connection_id = self.next_connection_id.lock().await;
            let connection_id = ConnectionId(*next_connection_id);
            *next_connection_id = next_connection_id.saturating_add(1);
            connection_id
        };

        let (tx, rx) = mpsc::unbounded_channel();
        self.sessions.lock().await.insert(
            connection_id,
            SessionHandle {
                tx,
                player_id: None,
            },
        );

        let server = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = server
                .run_session(
                    connection_id,
                    transport_session.io,
                    transport_session.transport,
                    rx,
                )
                .await
            {
                eprintln!("session {connection_id:?} ended with error: {error}");
            }
        });
    }

    async fn run_session(
        self: Arc<Self>,
        connection_id: ConnectionId,
        mut transport_io: TransportSessionIo,
        transport: TransportKind,
        mut rx: mpsc::UnboundedReceiver<SessionMessage>,
    ) -> Result<(), RuntimeError> {
        let mut read_buffer = BytesMut::with_capacity(8192);
        let mut session = SessionState {
            transport,
            phase: ConnectionPhase::Handshaking,
            adapter: None,
            player_id: None,
            entity_id: None,
            plugin_generation_id: None,
            session_capabilities: None,
        };

        loop {
            tokio::select! {
            read = transport_io.read_into(&mut read_buffer) => {
                let bytes_read = read?;
                if bytes_read == 0 {
                    break;
                }
                loop {
                    let codec: &dyn WireCodec = session
                        .adapter
                        .as_ref()
                        .map_or(default_wire_codec(session.transport), |current| current.wire_codec());
                    let Some(frame) = codec.try_decode_frame(&mut read_buffer)? else {
                        break;
                    };
                    let should_close = self
                        .handle_incoming_frame(
                            connection_id,
                            &mut transport_io,
                            &mut session,
                            frame,
                        )
                        .await?;
                    if should_close {
                        self.unregister_session(connection_id, session.player_id).await?;
                        return Ok(());
                    }
                }
            }
            maybe_message = rx.recv() => {
                    let Some(message) = maybe_message else {
                        break;
                    };
                    let should_close = self
                        .handle_outgoing_message(
                            connection_id,
                            &mut transport_io,
                            &mut session,
                            message,
                        )
                        .await?;
                    if should_close {
                        self.unregister_session(connection_id, session.player_id).await?;
                        return Ok(());
                    }
                }
            }
        }

        self.unregister_session(connection_id, session.player_id)
            .await?;
        Ok(())
    }

    async fn handle_incoming_frame(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        frame: Vec<u8>,
    ) -> Result<bool, RuntimeError> {
        Self::refresh_session_capabilities(session);
        match session.phase {
            ConnectionPhase::Handshaking => {
                self.handle_handshake_frame(transport_io, session, &frame)
                    .await
            }
            ConnectionPhase::Status => {
                self.handle_status_frame(transport_io, session, &frame)
                    .await
            }
            ConnectionPhase::Login => {
                self.handle_login_frame(connection_id, transport_io, session, &frame)
                    .await
            }
            ConnectionPhase::Play => self.handle_play_frame(session, &frame).await,
        }
    }

    async fn handle_handshake_frame(
        &self,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let Some(intent) = self
            .protocol_registry
            .route_handshake(session.transport, frame)?
        else {
            return Ok(true);
        };
        let next_phase = match intent.next_state {
            HandshakeNextState::Status => ConnectionPhase::Status,
            HandshakeNextState::Login => ConnectionPhase::Login,
        };
        if let Some(next_adapter) = self.protocol_registry.resolve_route(
            session.transport,
            intent.edition,
            intent.protocol_number,
        ) {
            session.adapter = Some(next_adapter);
            session.phase = next_phase;
            Self::refresh_session_capabilities(session);
            return Ok(false);
        }

        let fallback = Arc::clone(&self.default_adapter);
        let descriptor = fallback.descriptor();
        match next_phase {
            ConnectionPhase::Status => {
                session.adapter = Some(fallback);
                session.phase = ConnectionPhase::Status;
                Self::refresh_session_capabilities(session);
                Ok(false)
            }
            ConnectionPhase::Login => {
                let disconnect = fallback.encode_disconnect(
                    ConnectionPhase::Login,
                    &format!(
                        "Unsupported protocol {}. This server supports {} (protocol {}).",
                        intent.protocol_number, descriptor.version_name, descriptor.protocol_number
                    ),
                )?;
                write_payload(transport_io, fallback.wire_codec(), &disconnect).await?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    async fn handle_status_frame(
        &self,
        transport_io: &mut TransportSessionIo,
        session: &SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        match current.decode_status(frame)? {
            StatusRequest::Query => {
                let summary = self.player_summary().await;
                let response = current.encode_status_response(&ServerListStatus {
                    version: current.descriptor(),
                    players_online: summary.online_players,
                    max_players: usize::from(summary.max_players),
                    description: self.config.motd.clone(),
                })?;
                write_payload(transport_io, current.wire_codec(), &response).await?;
                Ok(false)
            }
            StatusRequest::Ping { payload } => {
                let response = current.encode_status_pong(payload)?;
                write_payload(transport_io, current.wire_codec(), &response).await?;
                Ok(true)
            }
        }
    }

    async fn handle_login_frame(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        match current.decode_login(frame)? {
            LoginRequest::LoginStart { username } => {
                let authenticated = self.authenticator.authenticate(&username)?;
                self.apply_command(CoreCommand::LoginStart {
                    connection_id,
                    username,
                    player_id: authenticated,
                })
                .await?;
                Ok(false)
            }
            LoginRequest::EncryptionResponse => {
                let disconnect = current
                    .encode_disconnect(ConnectionPhase::Login, "online-mode is not implemented")?;
                write_payload(transport_io, current.wire_codec(), &disconnect).await?;
                Ok(true)
            }
        }
    }

    async fn handle_play_frame(
        &self,
        session: &SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        let Some(current_player_id) = session.player_id else {
            return Ok(true);
        };
        if let Some(command) = current.decode_play(current_player_id, frame)? {
            self.apply_command(command).await?;
        }
        Ok(false)
    }

    async fn handle_outgoing_message(
        &self,
        _connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        message: SessionMessage,
    ) -> Result<bool, RuntimeError> {
        let SessionMessage::Event(event) = message;
        Self::refresh_session_capabilities(session);
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        let packets = match &event {
            CoreEvent::LoginAccepted { player, .. } => vec![current.encode_login_success(player)?],
            CoreEvent::Disconnect { reason } => {
                vec![current.encode_disconnect(session.phase, reason)?]
            }
            _ => {
                let player_id = session.player_id.ok_or_else(|| {
                    RuntimeError::Config("missing player id for play event encoding".to_string())
                })?;
                let entity_id = session.entity_id.ok_or_else(|| {
                    RuntimeError::Config("missing entity id for play event encoding".to_string())
                })?;
                current.encode_play_event(
                    &event,
                    &PlayEncodingContext {
                        player_id,
                        entity_id,
                    },
                )?
            }
        };
        for packet in packets {
            write_payload(transport_io, current.wire_codec(), &packet).await?;
        }

        match event {
            CoreEvent::LoginAccepted {
                player_id: accepted_player_id,
                entity_id: accepted_entity_id,
                ..
            } => {
                session.player_id = Some(accepted_player_id);
                session.entity_id = Some(accepted_entity_id);
                session.phase = ConnectionPhase::Play;
                Self::refresh_session_capabilities(session);
            }
            CoreEvent::Disconnect { .. } => return Ok(true),
            _ => {}
        }
        Ok(false)
    }

    async fn apply_command(&self, command: CoreCommand) -> Result<(), RuntimeError> {
        let should_persist = matches!(
            command,
            CoreCommand::LoginStart { .. }
                | CoreCommand::MoveIntent { .. }
                | CoreCommand::SetHeldSlot { .. }
                | CoreCommand::CreativeInventorySet { .. }
                | CoreCommand::DigBlock { .. }
                | CoreCommand::PlaceBlock { .. }
                | CoreCommand::Disconnect { .. }
        );
        let events = {
            let mut state = self.state.lock().await;
            let events = state.core.apply_command(command, now_ms());
            if should_persist {
                state.dirty = true;
            }
            events
        };
        self.dispatch_events(events).await;
        Ok(())
    }

    async fn tick(&self) -> Result<(), RuntimeError> {
        let events = {
            let mut state = self.state.lock().await;
            state.core.tick(now_ms())
        };
        self.dispatch_events(events).await;
        Ok(())
    }

    async fn maybe_save(&self) -> Result<(), RuntimeError> {
        let snapshot = {
            let mut state = self.state.lock().await;
            if !state.dirty {
                return Ok(());
            }
            state.dirty = false;
            state.core.snapshot()
        };
        self.storage_adapter
            .save_snapshot(&self.config.world_dir, &snapshot)?;
        Ok(())
    }

    async fn dispatch_events(&self, events: Vec<TargetedEvent>) {
        for event in events {
            let target = event.target.clone();
            let payload = event.event.clone();
            if let EventTarget::Connection(connection_id) = target
                && let CoreEvent::LoginAccepted { player_id, .. } = payload
                && let Some(session) = self.sessions.lock().await.get_mut(&connection_id)
            {
                session.player_id = Some(player_id);
            }

            let recipients = {
                let sessions = self.sessions.lock().await;
                match target {
                    EventTarget::Connection(connection_id) => sessions
                        .get(&connection_id)
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                    EventTarget::Player(target_player_id) => sessions
                        .values()
                        .filter(|session| session.player_id == Some(target_player_id))
                        .cloned()
                        .collect::<Vec<_>>(),
                    EventTarget::EveryoneExcept(excluded_player_id) => sessions
                        .values()
                        .filter(|session| {
                            session.player_id.is_some()
                                && session.player_id != Some(excluded_player_id)
                        })
                        .cloned()
                        .collect::<Vec<_>>(),
                }
            };

            for recipient in recipients {
                let _ = recipient.tx.send(SessionMessage::Event(payload.clone()));
            }
        }
    }

    async fn unregister_session(
        &self,
        connection_id: ConnectionId,
        player_id: Option<PlayerId>,
    ) -> Result<(), RuntimeError> {
        self.sessions.lock().await.remove(&connection_id);
        if let Some(player_id) = player_id {
            self.apply_command(CoreCommand::Disconnect { player_id })
                .await?;
        }
        Ok(())
    }

    async fn player_summary(&self) -> PlayerSummary {
        self.state.lock().await.core.player_summary()
    }
}

fn build_listener_plans(
    config: &ServerConfig,
    protocols: &ProtocolRegistry,
) -> Result<Vec<ListenerPlan>, RuntimeError> {
    let tcp_adapter_ids = protocols.adapter_ids_for_transport(TransportKind::Tcp);
    if tcp_adapter_ids.is_empty() {
        return Err(RuntimeError::Config(
            "no tcp protocol adapters registered".to_string(),
        ));
    }
    let mut plans = vec![ListenerPlan {
        transport: TransportKind::Tcp,
        bind_addr: config.bind_addr(),
        adapter_ids: tcp_adapter_ids,
    }];
    if config.be_enabled {
        let udp_adapter_ids = protocols.adapter_ids_for_transport(TransportKind::Udp);
        if udp_adapter_ids.is_empty() {
            return Err(RuntimeError::Config(
                "be-enabled=true requires at least one udp protocol adapter".to_string(),
            ));
        }
        plans.push(ListenerPlan {
            transport: TransportKind::Udp,
            bind_addr: config.bind_addr(),
            adapter_ids: udp_adapter_ids,
        });
    }
    Ok(plans)
}

async fn bind_transport_listener(
    plan: ListenerPlan,
) -> Result<BoundTransportListener, RuntimeError> {
    match plan.transport {
        TransportKind::Tcp => Ok(BoundTransportListener::Tcp {
            listener: TcpListener::bind(plan.bind_addr).await?,
            adapter_ids: plan.adapter_ids,
        }),
        TransportKind::Udp => Ok(BoundTransportListener::Udp {
            socket: UdpSocket::bind(plan.bind_addr).await?,
            adapter_ids: plan.adapter_ids,
        }),
    }
}

fn default_wire_codec(transport: TransportKind) -> &'static dyn WireCodec {
    static TCP_CODEC: MinecraftWireCodec = MinecraftWireCodec;
    match transport {
        TransportKind::Tcp => &TCP_CODEC,
        TransportKind::Udp => unreachable!("udp transport sessions are not implemented"),
    }
}

/// # Errors
///
/// Returns [`RuntimeError`] when the server cannot bind, load its persisted
/// world state, or starts with unsupported configuration such as
/// `online-mode=true`.
pub async fn spawn_server(
    config: ServerConfig,
    registries: RuntimeRegistries,
) -> Result<RunningServer, RuntimeError> {
    let plugin_host = registries.plugin_host();
    if config.online_mode {
        return Err(RuntimeError::Unsupported(
            "online-mode=true is not implemented".to_string(),
        ));
    }
    if registries
        .protocols()
        .resolve_adapter(&config.default_adapter)
        .is_none()
    {
        return Err(RuntimeError::Config(format!(
            "unknown default-adapter `{}`",
            config.default_adapter
        )));
    }

    let mut enabled_adapter_ids = config.effective_enabled_adapters();
    if config.enabled_adapters.is_none()
        && config.be_enabled
        && registries
            .protocols()
            .resolve_adapter(BE_PLACEHOLDER_ADAPTER_ID)
            .is_some()
    {
        enabled_adapter_ids.push(BE_PLACEHOLDER_ADAPTER_ID.to_string());
    }
    if !enabled_adapter_ids
        .iter()
        .any(|adapter_id| adapter_id == &config.default_adapter)
    {
        return Err(RuntimeError::Config(format!(
            "default-adapter `{}` must be included in enabled-adapters",
            config.default_adapter
        )));
    }
    let active_protocols = registries
        .protocols()
        .filter_enabled(&enabled_adapter_ids)?;
    if !config.be_enabled
        && !active_protocols
            .adapter_ids_for_transport(TransportKind::Udp)
            .is_empty()
    {
        return Err(RuntimeError::Config(
            "enabled-adapters contains udp adapters but be-enabled=false".to_string(),
        ));
    }

    let default_adapter = active_protocols
        .resolve_adapter(&config.default_adapter)
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "default-adapter `{}` is not active",
                config.default_adapter
            ))
        })?;
    let storage_adapter = registries
        .storage()
        .resolve(&config.storage_profile)
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "unknown storage-profile `{}`",
                config.storage_profile
            ))
        })?;
    let snapshot = storage_adapter.load_snapshot(&config.world_dir)?;
    let core_config = CoreConfig {
        level_name: config.level_name.clone(),
        seed: 0,
        max_players: config.max_players,
        view_distance: config.view_distance,
        game_mode: config.game_mode,
        difficulty: config.difficulty,
        ..CoreConfig::default()
    };
    let core = match snapshot {
        Some(snapshot) => ServerCore::from_snapshot(core_config, snapshot),
        None => ServerCore::new(core_config),
    };
    let listener_plans = build_listener_plans(&config, &active_protocols)?;
    let mut bound_listeners = Vec::with_capacity(listener_plans.len());
    for plan in listener_plans {
        bound_listeners.push(bind_transport_listener(plan).await?);
    }
    let listener_bindings = bound_listeners
        .iter()
        .map(BoundTransportListener::listener_binding)
        .collect::<Result<Vec<_>, _>>()?;
    let mut tcp_listener = None;
    let mut udp_socket = None;
    for listener in bound_listeners {
        match listener {
            BoundTransportListener::Tcp { listener, .. } => {
                if tcp_listener.replace(listener).is_some() {
                    return Err(RuntimeError::Config(
                        "multiple tcp listeners are not supported".to_string(),
                    ));
                }
            }
            BoundTransportListener::Udp { socket, .. } => {
                if udp_socket.replace(socket).is_some() {
                    return Err(RuntimeError::Config(
                        "multiple udp listeners are not supported".to_string(),
                    ));
                }
            }
        }
    }
    let tcp_listener = tcp_listener
        .ok_or_else(|| RuntimeError::Config("no tcp transport listeners were bound".to_string()))?;

    let server = Arc::new(RuntimeServer {
        config,
        protocol_registry: active_protocols,
        plugin_host: plugin_host.clone(),
        default_adapter,
        authenticator: Arc::new(OfflineAuthenticator),
        storage_adapter,
        state: Mutex::new(RuntimeState { core, dirty: false }),
        sessions: Mutex::new(HashMap::new()),
        next_connection_id: Mutex::new(1),
    });

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let run_server = Arc::clone(&server);
    let join_handle = tokio::spawn(async move {
        let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
        let mut save_interval = tokio::time::interval(Duration::from_secs(2));
        let mut plugin_reload_interval =
            tokio::time::interval(Duration::from_millis(plugin_reload_poll_interval_ms()));
        let mut udp_buffer = [0_u8; 2048];
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    run_server.maybe_save().await?;
                    return Ok(());
                }
                accepted = tcp_listener.accept() => {
                    let (stream, _) = accepted?;
                    let session = AcceptedTransportSession {
                        transport: TransportKind::Tcp,
                        io: TransportSessionIo::Tcp(stream),
                    };
                    run_server.spawn_session(session).await;
                }
                udp_recv = async {
                    let socket = udp_socket
                        .as_ref()
                        .expect("udp socket branch should only run when configured");
                    socket.recv_from(&mut udp_buffer).await
                }, if udp_socket.is_some() => {
                    match udp_recv {
                        Ok((bytes_read, peer_addr)) => {
                            match classify_udp_datagram(&run_server.protocol_registry, &udp_buffer[..bytes_read]) {
                                Ok(UdpDatagramAction::Ignore) => {}
                                Ok(UdpDatagramAction::UnsupportedBedrock) => {
                                    eprintln!(
                                        "received unsupported bedrock datagram from {peer_addr}; bedrock support is not implemented yet"
                                    );
                                }
                                Err(error) => {
                                    eprintln!(
                                        "failed to classify udp datagram from {peer_addr}: {error}"
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            eprintln!("udp receive failed: {error}");
                        }
                    }
                }
                _ = tick_interval.tick() => {
                    run_server.tick().await?;
                }
                _ = plugin_reload_interval.tick(), if run_server.config.plugin_reload_watch && run_server.plugin_host.is_some() => {
                    if let Some(plugin_host) = &run_server.plugin_host
                        && let Err(error) = plugin_host.reload_modified()
                    {
                        eprintln!("plugin reload failed: {error}");
                    }
                }
                _ = save_interval.tick() => {
                    run_server.maybe_save().await?;
                }
            }
        }
    });

    Ok(RunningServer {
        listener_bindings,
        plugin_host,
        shutdown_tx: Some(shutdown_tx),
        join_handle,
    })
}

async fn write_payload(
    transport_io: &mut TransportSessionIo,
    codec: &dyn WireCodec,
    payload: &[u8],
) -> Result<(), RuntimeError> {
    let frame = codec.encode_frame(payload)?;
    transport_io.write_all(&frame).await?;
    Ok(())
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .expect("current unix time in milliseconds should fit into u64")
}

/// # Errors
///
/// Returns [`RuntimeError`] when the handshake payload cannot be encoded.
pub fn encode_handshake(protocol_version: i32, next_state: i32) -> Result<Vec<u8>, RuntimeError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    writer.write_varint(protocol_version);
    writer.write_string("localhost")?;
    writer.write_u16(25565);
    writer.write_varint(next_state);
    Ok(writer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{
        LevelType, ProtocolRegistry, RuntimeError, RuntimeRegistries, ServerConfig,
        StorageRegistry, UdpDatagramAction, build_listener_plans, classify_udp_datagram,
        encode_handshake, plugin_host_from_config, spawn_server,
    };
    use bytes::BytesMut;
    use mc_core::WorldSnapshot;
    use mc_proto_be_placeholder::BE_PLACEHOLDER_ADAPTER_ID;
    use mc_proto_common::{
        Edition, HandshakeIntent, HandshakeNextState, HandshakeProbe, MinecraftWireCodec,
        PacketReader, PacketWriter, StorageAdapter, TransportKind, WireCodec,
    };
    use mc_proto_je_1_7_10::{
        JE_1_7_10_ADAPTER_ID, JE_1_7_10_STORAGE_PROFILE_ID, Je1710Adapter,
        Je1710StorageAdapter,
    };
    use mc_proto_je_1_8_x::JE_1_8_X_ADAPTER_ID;
    use mc_proto_je_1_12_2::JE_1_12_2_ADAPTER_ID;
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UdpSocket;

    const RAKNET_MAGIC: [u8; 16] = [
        0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56,
        0x78,
    ];

    #[derive(Default)]
    struct RecordingStorageAdapter {
        load_calls: AtomicUsize,
        save_calls: AtomicUsize,
    }

    struct FakeUdpProbe;

    impl HandshakeProbe for FakeUdpProbe {
        fn transport_kind(&self) -> TransportKind {
            TransportKind::Udp
        }

        fn try_route(
            &self,
            _frame: &[u8],
        ) -> Result<Option<HandshakeIntent>, mc_proto_common::ProtocolError> {
            Ok(Some(HandshakeIntent {
                edition: Edition::Be,
                protocol_number: 999,
                server_host: "localhost".to_string(),
                server_port: 19132,
                next_state: HandshakeNextState::Status,
            }))
        }
    }

    #[cfg(target_os = "linux")]
    fn packaged_protocol_registries(
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<RuntimeRegistries, RuntimeError> {
        run_xtask_package_plugins(dist_dir, target_dir, build_tag)?;
        let config = ServerConfig {
            plugins_dir: dist_dir.to_path_buf(),
            ..ServerConfig::default()
        };
        let plugin_host = plugin_host_from_config(&config)?.ok_or_else(|| {
            RuntimeError::Config("packaged protocol plugins should be discovered".to_string())
        })?;
        let mut registries = RuntimeRegistries::new();
        registries.register_storage_profile(
            JE_1_7_10_STORAGE_PROFILE_ID,
            Arc::new(Je1710StorageAdapter),
        );
        plugin_host.load_into_registries(&mut registries)?;
        Ok(registries)
    }

    #[cfg(target_os = "linux")]
    fn run_xtask_package_plugins(
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<(), RuntimeError> {
        let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
        let status = Command::new(cargo)
            .current_dir(workspace_root())
            .env("CARGO_TARGET_DIR", target_dir)
            .env("REVY_PLUGIN_BUILD_TAG", build_tag)
            .arg("run")
            .arg("-p")
            .arg("xtask")
            .arg("--")
            .arg("package-plugins")
            .arg("--dist-dir")
            .arg(dist_dir)
            .status()
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(RuntimeError::Config(
                "xtask package-plugins failed".to_string(),
            ))
        }
    }

    #[cfg(target_os = "linux")]
    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("server-runtime crate should live under the workspace root")
            .to_path_buf()
    }

    impl StorageAdapter for RecordingStorageAdapter {
        fn load_snapshot(
            &self,
            _world_dir: &Path,
        ) -> Result<Option<WorldSnapshot>, mc_proto_common::StorageError> {
            self.load_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }

        fn save_snapshot(
            &self,
            _world_dir: &Path,
            _snapshot: &WorldSnapshot,
        ) -> Result<(), mc_proto_common::StorageError> {
            self.save_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    async fn write_packet(
        stream: &mut tokio::net::TcpStream,
        codec: &MinecraftWireCodec,
        payload: &[u8],
    ) -> Result<(), RuntimeError> {
        let frame = codec.encode_frame(payload)?;
        stream.write_all(&frame).await?;
        Ok(())
    }

    async fn connect_tcp(addr: SocketAddr) -> Result<tokio::net::TcpStream, RuntimeError> {
        Ok(tokio::net::TcpStream::connect(addr).await?)
    }

    fn listener_addr(server: &super::RunningServer) -> SocketAddr {
        server
            .listener_bindings()
            .iter()
            .find(|binding| binding.transport == TransportKind::Tcp)
            .expect("tcp listener binding should exist")
            .local_addr
    }

    fn udp_listener_addr(server: &super::RunningServer) -> SocketAddr {
        server
            .listener_bindings()
            .iter()
            .find(|binding| binding.transport == TransportKind::Udp)
            .expect("udp listener binding should exist")
            .local_addr
    }

    async fn read_packet(
        stream: &mut tokio::net::TcpStream,
        codec: &MinecraftWireCodec,
        buffer: &mut BytesMut,
    ) -> Result<Vec<u8>, RuntimeError> {
        loop {
            if let Some(frame) = codec.try_decode_frame(buffer)? {
                return Ok(frame);
            }
            let bytes_read = stream.read_buf(buffer).await?;
            if bytes_read == 0 {
                return Err(RuntimeError::Config("connection closed".to_string()));
            }
        }
    }

    async fn read_until_packet_id(
        stream: &mut tokio::net::TcpStream,
        codec: &MinecraftWireCodec,
        buffer: &mut BytesMut,
        wanted_packet_id: i32,
        max_attempts: usize,
    ) -> Result<Vec<u8>, RuntimeError> {
        let max_attempts = max_attempts.max(64);
        for _ in 0..max_attempts {
            let packet = tokio::time::timeout(
                Duration::from_millis(250),
                read_packet(stream, codec, buffer),
            )
            .await
            .map_err(|_| {
                RuntimeError::Config(format!(
                    "timed out waiting for packet id 0x{wanted_packet_id:02x}"
                ))
            })??;
            if packet_id(&packet) == wanted_packet_id {
                return Ok(packet);
            }
        }
        Err(RuntimeError::Config(format!(
            "did not receive packet id 0x{wanted_packet_id:02x}"
        )))
    }

    fn packet_id(frame: &[u8]) -> i32 {
        let mut reader = PacketReader::new(frame);
        reader.read_varint().expect("packet id should decode")
    }

    #[test]
    fn protocol_registry_resolves_registered_adapter() {
        let mut registry = ProtocolRegistry::new();
        let adapter = Arc::new(Je1710Adapter::new());
        registry.register_adapter(adapter.clone());
        registry.register_probe(adapter);

        let by_id = registry
            .resolve_adapter(JE_1_7_10_ADAPTER_ID)
            .expect("registered adapter should resolve by id");
        let by_route = registry
            .resolve_route(TransportKind::Tcp, Edition::Je, 5)
            .expect("registered adapter should resolve by route");

        assert_eq!(by_id.descriptor().adapter_id, JE_1_7_10_ADAPTER_ID);
        assert_eq!(by_id.descriptor().transport, TransportKind::Tcp);
        assert_eq!(by_route.descriptor().version_name, "1.7.10");
    }

    #[test]
    fn storage_registry_resolves_registered_profile() {
        let mut registry = StorageRegistry::new();
        let storage = Arc::new(RecordingStorageAdapter::default());
        registry.register_profile("recording", storage);

        assert!(registry.resolve("recording").is_some());
        assert!(registry.resolve("missing").is_none());
    }

    #[test]
    fn handshake_probe_transport_kind_filters_routing() {
        let mut registry = ProtocolRegistry::new();
        registry.register_probe(Arc::new(FakeUdpProbe));

        let tcp_route = registry
            .route_handshake(TransportKind::Tcp, &[0x00])
            .expect("tcp routing should not fail");
        let udp_route = registry
            .route_handshake(TransportKind::Udp, &[0x00])
            .expect("udp routing should not fail");

        assert!(tcp_route.is_none());
        assert!(udp_route.is_some());
    }

    #[test]
    fn listener_plan_includes_tcp_binding_and_registered_adapter() -> Result<(), RuntimeError> {
        let registries = RuntimeRegistries::with_je_1_7_10();
        let plans = build_listener_plans(&ServerConfig::default(), registries.protocols())?;

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].transport, TransportKind::Tcp);
        assert!(
            plans[0]
                .adapter_ids
                .iter()
                .any(|adapter_id| adapter_id == JE_1_7_10_ADAPTER_ID)
        );
        Ok(())
    }

    #[test]
    fn listener_plan_includes_udp_binding_when_bedrock_is_enabled() -> Result<(), RuntimeError> {
        let registries = RuntimeRegistries::with_je_and_be_placeholder();
        let plans = build_listener_plans(
            &ServerConfig {
                be_enabled: true,
                ..ServerConfig::default()
            },
            registries.protocols(),
        )?;

        assert_eq!(plans.len(), 2);
        assert_eq!(plans[1].transport, TransportKind::Udp);
        assert!(
            plans[1]
                .adapter_ids
                .iter()
                .any(|adapter_id| adapter_id == BE_PLACEHOLDER_ADAPTER_ID)
        );
        Ok(())
    }

    #[tokio::test]
    async fn running_server_exposes_listener_bindings() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;

        let binding = server
            .listener_bindings()
            .first()
            .expect("tcp listener binding should exist")
            .clone();
        assert_eq!(binding.transport, TransportKind::Tcp);
        assert!(binding.local_addr.port() > 0);
        assert!(
            binding
                .adapter_ids
                .iter()
                .any(|adapter_id| adapter_id == JE_1_7_10_ADAPTER_ID)
        );

        server.shutdown().await
    }

    #[tokio::test]
    async fn running_server_exposes_udp_listener_binding_when_enabled() -> Result<(), RuntimeError>
    {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                be_enabled: true,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_and_be_placeholder(),
        )
        .await?;

        assert_eq!(server.listener_bindings().len(), 2);
        let binding = server
            .listener_bindings()
            .iter()
            .find(|binding| binding.transport == TransportKind::Udp)
            .expect("udp listener binding should exist");
        assert!(binding.local_addr.port() > 0);
        assert!(
            binding
                .adapter_ids
                .iter()
                .any(|adapter_id| adapter_id == BE_PLACEHOLDER_ADAPTER_ID)
        );

        server.shutdown().await
    }

    #[test]
    fn server_properties_accept_flat_level_type() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let path = temp_dir.path().join("server.properties");
        fs::write(
            &path,
            "level-name=flatland\nlevel-type=FLAT\nbe-enabled=true\nonline-mode=false\ndefault-adapter=je-1_7_10\nstorage-profile=je-anvil-1_7_10\n",
        )?;

        let config = ServerConfig::from_properties(&path)?;

        assert_eq!(config.level_name, "flatland");
        assert_eq!(config.level_type, LevelType::Flat);
        assert!(config.be_enabled);
        assert_eq!(config.default_adapter, JE_1_7_10_ADAPTER_ID);
        assert_eq!(config.storage_profile, JE_1_7_10_STORAGE_PROFILE_ID);
        assert_eq!(config.world_dir, temp_dir.path().join("flatland"));
        Ok(())
    }

    #[test]
    fn server_properties_use_default_adapter_and_storage_profile() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let path = temp_dir.path().join("server.properties");
        fs::write(&path, "level-name=flatland\nlevel-type=FLAT\n")?;

        let config = ServerConfig::from_properties(&path)?;

        assert!(!config.be_enabled);
        assert_eq!(config.default_adapter, JE_1_7_10_ADAPTER_ID);
        assert_eq!(config.storage_profile, JE_1_7_10_STORAGE_PROFILE_ID);
        Ok(())
    }

    #[test]
    fn server_properties_parse_enabled_adapters() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let path = temp_dir.path().join("server.properties");
        fs::write(&path, "enabled-adapters=je-1_7_10, je-1_8_x,je-1_12_2\n")?;

        let config = ServerConfig::from_properties(&path)?;
        assert_eq!(
            config.enabled_adapters,
            Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_8_X_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ])
        );
        Ok(())
    }

    #[test]
    fn server_properties_reject_non_flat_level_type() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let path = temp_dir.path().join("server.properties");
        fs::write(&path, "level-type=DEFAULT\n")?;

        let error = ServerConfig::from_properties(&path).expect_err("DEFAULT should be rejected");
        assert!(
            matches!(error, RuntimeError::Unsupported(message) if message.contains("only FLAT"))
        );
        Ok(())
    }

    #[test]
    fn be_enabled_requires_udp_adapter() {
        let registry = RuntimeRegistries::with_je_1_7_10();
        let error = build_listener_plans(
            &ServerConfig {
                be_enabled: true,
                ..ServerConfig::default()
            },
            registry.protocols(),
        )
        .expect_err("be-enabled should require udp adapter");
        assert!(matches!(
            error,
            RuntimeError::Config(message) if message.contains("be-enabled=true")
        ));
    }

    #[tokio::test]
    async fn enabled_adapters_must_include_default_adapter() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let result = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                enabled_adapters: Some(vec![JE_1_8_X_ADAPTER_ID.to_string()]),
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_builtin_adapters(),
        )
        .await;

        let Err(error) = result else {
            panic!("default adapter missing from enabled list should fail");
        };
        assert!(matches!(
            error,
            RuntimeError::Config(message) if message.contains("default-adapter")
        ));
        Ok(())
    }

    #[tokio::test]
    async fn duplicate_enabled_adapters_fail_fast() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let result = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                enabled_adapters: Some(vec![
                    JE_1_7_10_ADAPTER_ID.to_string(),
                    JE_1_7_10_ADAPTER_ID.to_string(),
                ]),
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_builtin_adapters(),
        )
        .await;

        let Err(error) = result else {
            panic!("duplicate enabled adapters should fail");
        };
        assert!(matches!(
            error,
            RuntimeError::Config(message) if message.contains("duplicate adapter")
        ));
        Ok(())
    }

    #[tokio::test]
    async fn tcp_listener_binding_reports_enabled_java_versions() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                enabled_adapters: Some(vec![
                    JE_1_7_10_ADAPTER_ID.to_string(),
                    JE_1_8_X_ADAPTER_ID.to_string(),
                    JE_1_12_2_ADAPTER_ID.to_string(),
                ]),
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_builtin_adapters(),
        )
        .await?;

        let binding = server
            .listener_bindings()
            .iter()
            .find(|binding| binding.transport == TransportKind::Tcp)
            .expect("tcp listener binding should exist");
        assert_eq!(binding.adapter_ids.len(), 3);
        assert!(
            binding
                .adapter_ids
                .iter()
                .any(|adapter_id| adapter_id == JE_1_7_10_ADAPTER_ID)
        );
        assert!(
            binding
                .adapter_ids
                .iter()
                .any(|adapter_id| adapter_id == JE_1_8_X_ADAPTER_ID)
        );
        assert!(
            binding
                .adapter_ids
                .iter()
                .any(|adapter_id| adapter_id == JE_1_12_2_ADAPTER_ID)
        );

        server.shutdown().await
    }

    fn login_start(username: &str) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x00);
        let _ = writer.write_string(username);
        writer.into_inner()
    }

    fn status_request() -> Vec<u8> {
        vec![0x00]
    }

    fn status_ping(value: i64) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x01);
        writer.write_i64(value);
        writer.into_inner()
    }

    fn raknet_unconnected_ping() -> Vec<u8> {
        let mut frame = Vec::with_capacity(33);
        frame.push(0x01);
        frame.extend_from_slice(&123_i64.to_be_bytes());
        frame.extend_from_slice(&RAKNET_MAGIC);
        frame.extend_from_slice(&456_i64.to_be_bytes());
        frame
    }

    fn player_position_look(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x06);
        writer.write_f64(x);
        writer.write_f64(y + 1.62);
        writer.write_f64(y);
        writer.write_f64(z);
        writer.write_f32(yaw);
        writer.write_f32(pitch);
        writer.write_bool(true);
        writer.into_inner()
    }

    fn held_item_change(slot: i16) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x09);
        writer.write_i16(slot);
        writer.into_inner()
    }

    fn creative_inventory_action(slot: i16, item_id: i16, count: u8, damage: i16) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x10);
        writer.write_i16(slot);
        writer.write_i16(item_id);
        writer.write_u8(count);
        writer.write_i16(damage);
        writer.write_i16(-1);
        writer.into_inner()
    }

    fn player_block_placement(
        x: i32,
        y: u8,
        z: i32,
        face: u8,
        held_item: Option<(i16, u8, i16)>,
    ) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x08);
        writer.write_i32(x);
        writer.write_u8(y);
        writer.write_i32(z);
        writer.write_u8(face);
        if let Some((item_id, count, damage)) = held_item {
            writer.write_i16(item_id);
            writer.write_u8(count);
            writer.write_i16(damage);
        }
        writer.write_i16(-1);
        writer.write_u8(8);
        writer.write_u8(8);
        writer.write_u8(8);
        writer.into_inner()
    }

    fn player_digging(status: u8, x: i32, y: u8, z: i32, face: u8) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x07);
        writer.write_u8(status);
        writer.write_i32(x);
        writer.write_u8(y);
        writer.write_i32(z);
        writer.write_u8(face);
        writer.into_inner()
    }

    fn player_position_look_1_8(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x06);
        writer.write_f64(x);
        writer.write_f64(y);
        writer.write_f64(z);
        writer.write_f32(yaw);
        writer.write_f32(pitch);
        writer.write_bool(true);
        writer.into_inner()
    }

    fn creative_inventory_action_1_12(slot: i16, item_id: i16, count: u8, damage: i16) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x1b);
        writer.write_i16(slot);
        writer.write_i16(item_id);
        writer.write_u8(count);
        writer.write_i16(damage);
        writer.write_i16(-1);
        writer.into_inner()
    }

    fn player_block_placement_1_12(x: i32, y: i32, z: i32, face: i32, hand: i32) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x1f);
        writer.write_i64(mc_proto_je_common::pack_block_position(
            mc_core::BlockPos::new(x, y, z),
        ));
        writer.write_varint(face);
        writer.write_varint(hand);
        writer.write_f32(0.5);
        writer.write_f32(0.5);
        writer.write_f32(0.5);
        writer.into_inner()
    }

    fn read_slot(reader: &mut PacketReader<'_>) -> Result<Option<(i16, u8, i16)>, RuntimeError> {
        let item_id = reader.read_i16()?;
        if item_id < 0 {
            return Ok(None);
        }
        let count = reader.read_u8()?;
        let damage = reader.read_i16()?;
        let nbt_length = reader.read_i16()?;
        if nbt_length != -1 {
            return Err(RuntimeError::Config(
                "test helper only supports empty slot nbt".to_string(),
            ));
        }
        Ok(Some((item_id, count, damage)))
    }

    fn window_items_slot(
        packet: &[u8],
        wanted_slot: usize,
    ) -> Result<Option<(i16, u8, i16)>, RuntimeError> {
        window_items_slot_with_packet_id(packet, 0x30, wanted_slot)
    }

    fn window_items_slot_with_packet_id(
        packet: &[u8],
        expected_packet_id: i32,
        wanted_slot: usize,
    ) -> Result<Option<(i16, u8, i16)>, RuntimeError> {
        let mut reader = PacketReader::new(packet);
        if reader.read_varint()? != expected_packet_id {
            return Err(RuntimeError::Config(
                "expected window items packet".to_string(),
            ));
        }
        let _window_id = reader.read_u8()?;
        let count = usize::try_from(reader.read_i16()?)
            .map_err(|_| RuntimeError::Config("negative window item count".to_string()))?;
        if wanted_slot >= count {
            return Err(RuntimeError::Config(
                "wanted slot out of bounds".to_string(),
            ));
        }
        for slot in 0..count {
            let item = read_slot(&mut reader)?;
            if slot == wanted_slot {
                return Ok(item);
            }
        }
        Err(RuntimeError::Config("wanted slot missing".to_string()))
    }

    fn set_slot_slot(packet: &[u8], expected_packet_id: i32) -> Result<i16, RuntimeError> {
        let mut reader = PacketReader::new(packet);
        if reader.read_varint()? != expected_packet_id {
            return Err(RuntimeError::Config("expected set slot packet".to_string()));
        }
        let _window_id = reader.read_i8()?;
        reader.read_i16().map_err(RuntimeError::from)
    }

    fn held_item_from_packet(packet: &[u8]) -> Result<i8, RuntimeError> {
        let mut reader = PacketReader::new(packet);
        if reader.read_varint()? != 0x09 {
            return Err(RuntimeError::Config(
                "expected held item change packet".to_string(),
            ));
        }
        reader.read_i8().map_err(RuntimeError::from)
    }

    fn block_change_from_packet(packet: &[u8]) -> Result<(i32, u8, i32, i32, u8), RuntimeError> {
        let mut reader = PacketReader::new(packet);
        if reader.read_varint()? != 0x23 {
            return Err(RuntimeError::Config(
                "expected block change packet".to_string(),
            ));
        }
        let x = reader.read_i32()?;
        let y = reader.read_u8()?;
        let z = reader.read_i32()?;
        let block_id = reader.read_varint()?;
        let metadata = reader.read_u8()?;
        Ok((x, y, z, block_id, metadata))
    }

    fn block_change_from_packet_1_8(packet: &[u8]) -> Result<(i32, i32, i32, i32), RuntimeError> {
        let mut reader = PacketReader::new(packet);
        if reader.read_varint()? != 0x23 {
            return Err(RuntimeError::Config(
                "expected 1.8 block change packet".to_string(),
            ));
        }
        let position = mc_proto_je_common::unpack_block_position(reader.read_i64()?);
        let block_state = reader.read_varint()?;
        Ok((position.x, position.y, position.z, block_state))
    }

    fn player_abilities_flags(packet: &[u8]) -> Result<u8, RuntimeError> {
        let mut reader = PacketReader::new(packet);
        if reader.read_varint()? != 0x39 {
            return Err(RuntimeError::Config(
                "expected player abilities packet".to_string(),
            ));
        }
        reader.read_u8().map_err(RuntimeError::from)
    }

    #[tokio::test]
    async fn status_ping_login_and_initial_world_work() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&server);
        let codec = MinecraftWireCodec;

        let mut status_stream = connect_tcp(addr).await?;
        write_packet(&mut status_stream, &codec, &encode_handshake(5, 1)?).await?;
        write_packet(&mut status_stream, &codec, &status_request()).await?;
        let mut buffer = BytesMut::new();
        let status_response = read_packet(&mut status_stream, &codec, &mut buffer).await?;
        assert_eq!(packet_id(&status_response), 0x00);
        write_packet(&mut status_stream, &codec, &status_ping(42)).await?;
        let pong = read_packet(&mut status_stream, &codec, &mut buffer).await?;
        assert_eq!(packet_id(&pong), 0x01);

        let mut login_stream = connect_tcp(addr).await?;
        write_packet(&mut login_stream, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut login_stream, &codec, &login_start("alpha")).await?;
        let mut login_buffer = BytesMut::new();
        let login_success = read_packet(&mut login_stream, &codec, &mut login_buffer).await?;
        assert_eq!(packet_id(&login_success), 0x02);
        let join_game = read_packet(&mut login_stream, &codec, &mut login_buffer).await?;
        assert_eq!(packet_id(&join_game), 0x01);
        let chunk_bulk =
            read_until_packet_id(&mut login_stream, &codec, &mut login_buffer, 0x26, 8).await?;
        assert_eq!(packet_id(&chunk_bulk), 0x26);

        server.shutdown().await
    }

    #[tokio::test]
    async fn creative_join_sends_inventory_selected_slot_and_abilities() -> Result<(), RuntimeError>
    {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                game_mode: 1,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&server);
        let codec = MinecraftWireCodec;

        let mut stream = connect_tcp(addr).await?;
        write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut stream, &codec, &login_start("creative")).await?;
        let mut buffer = BytesMut::new();
        let mut window_items = None;
        let mut held_item = None;
        let mut abilities = None;
        for _ in 0..12 {
            let packet = read_packet(&mut stream, &codec, &mut buffer).await?;
            match packet_id(&packet) {
                0x30 if window_items.is_none() => window_items = Some(packet),
                0x09 if held_item.is_none() => held_item = Some(packet),
                0x39 if abilities.is_none() => abilities = Some(packet),
                _ => {}
            }
            if window_items.is_some() && held_item.is_some() && abilities.is_some() {
                break;
            }
        }
        let window_items = window_items
            .ok_or_else(|| RuntimeError::Config("window items not received".to_string()))?;
        let held_item = held_item
            .ok_or_else(|| RuntimeError::Config("held item change not received".to_string()))?;
        let abilities = abilities
            .ok_or_else(|| RuntimeError::Config("player abilities not received".to_string()))?;

        assert_eq!(window_items_slot(&window_items, 36)?, Some((1, 64, 0)));
        assert_eq!(window_items_slot(&window_items, 44)?, Some((45, 64, 0)));
        assert_eq!(held_item_from_packet(&held_item)?, 0);
        assert_eq!(player_abilities_flags(&abilities)? & 0x0d, 0x0d);

        server.shutdown().await
    }

    #[tokio::test]
    async fn unsupported_status_protocol_receives_server_list_response() -> Result<(), RuntimeError>
    {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&server);
        let codec = MinecraftWireCodec;

        let mut stream = connect_tcp(addr).await?;
        write_packet(&mut stream, &codec, &encode_handshake(47, 1)?).await?;
        write_packet(&mut stream, &codec, &status_request()).await?;
        let mut buffer = BytesMut::new();
        let status_response = read_packet(&mut stream, &codec, &mut buffer).await?;
        assert_eq!(packet_id(&status_response), 0x00);
        let mut reader = PacketReader::new(&status_response);
        assert_eq!(reader.read_varint().expect("packet id should decode"), 0x00);
        let payload = reader
            .read_string(32767)
            .expect("status json should decode");
        assert!(payload.contains("\"protocol\":5"));
        assert!(payload.contains("\"name\":\"1.7.10\""));

        write_packet(&mut stream, &codec, &status_ping(99)).await?;
        let pong = read_packet(&mut stream, &codec, &mut buffer).await?;
        assert_eq!(packet_id(&pong), 0x01);

        server.shutdown().await
    }

    #[test]
    fn udp_bedrock_probe_classifies_placeholder_datagram() -> Result<(), RuntimeError> {
        let registry = RuntimeRegistries::with_je_and_be_placeholder();
        let action = classify_udp_datagram(registry.protocols(), &raknet_unconnected_ping())?;
        assert_eq!(action, UdpDatagramAction::UnsupportedBedrock);
        Ok(())
    }

    #[test]
    fn udp_unknown_datagram_is_ignored() -> Result<(), RuntimeError> {
        let registry = RuntimeRegistries::with_je_and_be_placeholder();
        let action = classify_udp_datagram(registry.protocols(), &[0xde, 0xad, 0xbe, 0xef])?;
        assert_eq!(action, UdpDatagramAction::Ignore);
        Ok(())
    }

    #[tokio::test]
    async fn udp_bedrock_probe_does_not_block_je_status() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                be_enabled: true,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_and_be_placeholder(),
        )
        .await?;

        let udp_addr = udp_listener_addr(&server);
        let udp_client = UdpSocket::bind("127.0.0.1:0").await?;
        udp_client
            .send_to(&raknet_unconnected_ping(), udp_addr)
            .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;

        let addr = listener_addr(&server);
        let codec = MinecraftWireCodec;
        let mut stream = connect_tcp(addr).await?;
        write_packet(&mut stream, &codec, &encode_handshake(5, 1)?).await?;
        write_packet(&mut stream, &codec, &status_request()).await?;
        let mut buffer = BytesMut::new();
        let status_response = read_packet(&mut stream, &codec, &mut buffer).await?;
        let mut reader = PacketReader::new(&status_response);
        assert_eq!(reader.read_varint().expect("packet id should decode"), 0x00);
        let payload = reader
            .read_string(32767)
            .expect("status json should decode");
        assert!(payload.contains("\"online\":0"));

        server.shutdown().await
    }

    #[tokio::test]
    async fn online_mode_fails_fast() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let result = spawn_server(
            ServerConfig {
                online_mode: true,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await;
        let Err(error) = result else {
            panic!("online-mode should fail fast");
        };
        assert!(
            matches!(error, RuntimeError::Unsupported(message) if message.contains("online-mode=true"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn unknown_default_adapter_fails_fast() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let result = spawn_server(
            ServerConfig {
                default_adapter: "missing".to_string(),
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await;
        let Err(error) = result else {
            panic!("unknown default adapter should fail fast");
        };
        assert!(
            matches!(error, RuntimeError::Config(message) if message.contains("unknown default-adapter"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn unknown_storage_profile_fails_fast() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let result = spawn_server(
            ServerConfig {
                storage_profile: "missing".to_string(),
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await;
        let Err(error) = result else {
            panic!("unknown storage profile should fail fast");
        };
        assert!(
            matches!(error, RuntimeError::Config(message) if message.contains("unknown storage-profile"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn unmatched_probe_closes_without_response() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&server);
        let codec = MinecraftWireCodec;

        let mut stream = connect_tcp(addr).await?;
        write_packet(&mut stream, &codec, &[0x01]).await?;

        let mut bytes = [0_u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut bytes))
            .await
            .map_err(|_| RuntimeError::Config("probe mismatch did not close".to_string()))??;
        assert_eq!(read, 0);

        server.shutdown().await
    }

    #[tokio::test]
    async fn unsupported_login_protocol_receives_disconnect() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&server);
        let codec = MinecraftWireCodec;

        let mut stream = connect_tcp(addr).await?;
        write_packet(&mut stream, &codec, &encode_handshake(47, 2)?).await?;
        let mut buffer = BytesMut::new();
        let disconnect = read_packet(&mut stream, &codec, &mut buffer).await?;
        let mut reader = PacketReader::new(&disconnect);
        assert_eq!(reader.read_varint().expect("packet id should decode"), 0x00);
        let reason = reader
            .read_string(32767)
            .expect("disconnect reason should decode");
        assert!(reason.contains("Unsupported protocol 47"));
        assert!(reason.contains("1.7.10"));

        server.shutdown().await
    }

    #[tokio::test]
    async fn mixed_java_versions_share_login_movement_and_block_sync() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                game_mode: 1,
                enabled_adapters: Some(vec![
                    JE_1_7_10_ADAPTER_ID.to_string(),
                    JE_1_8_X_ADAPTER_ID.to_string(),
                    JE_1_12_2_ADAPTER_ID.to_string(),
                ]),
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_builtin_adapters(),
        )
        .await?;
        let addr = listener_addr(&server);
        let codec = MinecraftWireCodec;

        let mut legacy = connect_tcp(addr).await?;
        write_packet(&mut legacy, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut legacy, &codec, &login_start("legacy")).await?;
        let mut legacy_buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x30, 12).await?;

        let mut modern_18 = connect_tcp(addr).await?;
        write_packet(&mut modern_18, &codec, &encode_handshake(47, 2)?).await?;
        write_packet(&mut modern_18, &codec, &login_start("middle")).await?;
        let mut modern_18_buffer = BytesMut::new();
        let _ =
            read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x30, 24).await?;
        let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

        let mut modern_112 = connect_tcp(addr).await?;
        write_packet(&mut modern_112, &codec, &encode_handshake(340, 2)?).await?;
        write_packet(&mut modern_112, &codec, &login_start("latest")).await?;
        let mut modern_112_buffer = BytesMut::new();
        let _ =
            read_until_packet_id(&mut modern_112, &codec, &mut modern_112_buffer, 0x14, 24).await?;
        let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;
        let _ =
            read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x0c, 12).await?;

        write_packet(
            &mut modern_18,
            &codec,
            &player_position_look_1_8(32.5, 4.0, 0.5, 90.0, 0.0),
        )
        .await?;
        let legacy_teleport =
            read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x18, 16).await?;
        let modern_112_teleport =
            read_until_packet_id(&mut modern_112, &codec, &mut modern_112_buffer, 0x4c, 16).await?;
        assert_eq!(packet_id(&legacy_teleport), 0x18);
        assert_eq!(packet_id(&modern_112_teleport), 0x4c);

        write_packet(
            &mut modern_112,
            &codec,
            &player_block_placement_1_12(2, 3, 0, 1, 0),
        )
        .await?;
        let legacy_block_change =
            read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x23, 16).await?;
        let modern_18_block_change =
            read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x23, 16).await?;
        assert_eq!(
            block_change_from_packet(&legacy_block_change)?,
            (2, 4, 0, 1, 0)
        );
        assert_eq!(
            block_change_from_packet_1_8(&modern_18_block_change)?,
            (2, 4, 0, 16)
        );

        server.shutdown().await
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn packaged_plugins_support_mixed_versions_and_bedrock_probe() -> Result<(), RuntimeError>
    {
        let temp_dir = tempdir()?;
        let dist_dir = temp_dir.path().join("dist").join("plugins");
        let target_dir = temp_dir.path().join("target");
        let registries = packaged_protocol_registries(&dist_dir, &target_dir, "plugin-only")?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                be_enabled: true,
                game_mode: 1,
                enabled_adapters: Some(vec![
                    JE_1_7_10_ADAPTER_ID.to_string(),
                    JE_1_8_X_ADAPTER_ID.to_string(),
                    JE_1_12_2_ADAPTER_ID.to_string(),
                    BE_PLACEHOLDER_ADAPTER_ID.to_string(),
                ]),
                world_dir: temp_dir.path().join("world"),
                plugins_dir: dist_dir,
                ..ServerConfig::default()
            },
            registries,
        )
        .await?;

        let udp_addr = udp_listener_addr(&server);
        let udp_client = UdpSocket::bind("127.0.0.1:0").await?;
        udp_client
            .send_to(&raknet_unconnected_ping(), udp_addr)
            .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;

        let addr = listener_addr(&server);
        let codec = MinecraftWireCodec;

        let mut status_stream = connect_tcp(addr).await?;
        write_packet(&mut status_stream, &codec, &encode_handshake(5, 1)?).await?;
        write_packet(&mut status_stream, &codec, &[0x00]).await?;
        let mut status_buffer = BytesMut::new();
        let status = read_packet(&mut status_stream, &codec, &mut status_buffer).await?;
        assert_eq!(packet_id(&status), 0x00);

        let mut legacy = connect_tcp(addr).await?;
        write_packet(&mut legacy, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut legacy, &codec, &login_start("legacy")).await?;
        let mut legacy_buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x30, 12).await?;

        let mut modern_18 = connect_tcp(addr).await?;
        write_packet(&mut modern_18, &codec, &encode_handshake(47, 2)?).await?;
        write_packet(&mut modern_18, &codec, &login_start("middle")).await?;
        let mut modern_18_buffer = BytesMut::new();
        let _ =
            read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x30, 24).await?;
        let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

        let mut modern_112 = connect_tcp(addr).await?;
        write_packet(&mut modern_112, &codec, &encode_handshake(340, 2)?).await?;
        write_packet(&mut modern_112, &codec, &login_start("latest")).await?;
        let mut modern_112_buffer = BytesMut::new();
        let _ =
            read_until_packet_id(&mut modern_112, &codec, &mut modern_112_buffer, 0x14, 24).await?;
        let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;
        let _ =
            read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x0c, 12).await?;

        write_packet(
            &mut modern_18,
            &codec,
            &player_position_look_1_8(32.5, 4.0, 0.5, 90.0, 0.0),
        )
        .await?;
        let legacy_teleport =
            read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x18, 16).await?;
        let modern_112_teleport =
            read_until_packet_id(&mut modern_112, &codec, &mut modern_112_buffer, 0x4c, 16).await?;
        assert_eq!(packet_id(&legacy_teleport), 0x18);
        assert_eq!(packet_id(&modern_112_teleport), 0x4c);

        write_packet(
            &mut modern_112,
            &codec,
            &player_block_placement_1_12(2, 3, 0, 1, 0),
        )
        .await?;
        let legacy_block_change =
            read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x23, 16).await?;
        let modern_18_block_change =
            read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x23, 16).await?;
        assert_eq!(
            block_change_from_packet(&legacy_block_change)?,
            (2, 4, 0, 1, 0)
        );
        assert_eq!(
            block_change_from_packet_1_8(&modern_18_block_change)?,
            (2, 4, 0, 16)
        );

        server.shutdown().await
    }

    #[tokio::test]
    async fn modern_offhand_persists_without_leaking_legacy_slots() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let world_dir = temp_dir.path().join("world");
        let codec = MinecraftWireCodec;

        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                game_mode: 1,
                enabled_adapters: Some(vec![
                    JE_1_7_10_ADAPTER_ID.to_string(),
                    JE_1_8_X_ADAPTER_ID.to_string(),
                    JE_1_12_2_ADAPTER_ID.to_string(),
                ]),
                world_dir: world_dir.clone(),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_builtin_adapters(),
        )
        .await?;
        let addr = listener_addr(&server);

        let mut modern = connect_tcp(addr).await?;
        write_packet(&mut modern, &codec, &encode_handshake(340, 2)?).await?;
        write_packet(&mut modern, &codec, &login_start("alpha")).await?;
        let mut modern_buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x14, 24).await?;

        write_packet(
            &mut modern,
            &codec,
            &creative_inventory_action_1_12(45, 20, 64, 0),
        )
        .await?;
        let set_slot =
            read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x16, 8).await?;
        assert_eq!(set_slot_slot(&set_slot, 0x16)?, 45);

        server.shutdown().await?;

        let restarted = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                game_mode: 1,
                enabled_adapters: Some(vec![
                    JE_1_7_10_ADAPTER_ID.to_string(),
                    JE_1_8_X_ADAPTER_ID.to_string(),
                    JE_1_12_2_ADAPTER_ID.to_string(),
                ]),
                world_dir,
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_builtin_adapters(),
        )
        .await?;
        let addr = listener_addr(&restarted);

        let mut modern = connect_tcp(addr).await?;
        write_packet(&mut modern, &codec, &encode_handshake(340, 2)?).await?;
        write_packet(&mut modern, &codec, &login_start("alpha")).await?;
        let mut modern_buffer = BytesMut::new();
        let window_items =
            read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x14, 24).await?;
        assert_eq!(
            window_items_slot_with_packet_id(&window_items, 0x14, 45)?,
            Some((20, 64, 0))
        );

        let mut legacy = connect_tcp(addr).await?;
        write_packet(&mut legacy, &codec, &encode_handshake(47, 2)?).await?;
        write_packet(&mut legacy, &codec, &login_start("beta")).await?;
        let mut legacy_buffer = BytesMut::new();
        let legacy_window_items =
            read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x30, 24).await?;
        assert!(window_items_slot(&legacy_window_items, 45).is_err());

        restarted.shutdown().await
    }

    #[tokio::test]
    async fn creative_place_and_break_broadcast_block_changes() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                game_mode: 1,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&server);
        let codec = MinecraftWireCodec;

        let mut first = connect_tcp(addr).await?;
        write_packet(&mut first, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut first, &codec, &login_start("alpha")).await?;
        let mut first_buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x30, 12).await?;

        let mut second = connect_tcp(addr).await?;
        write_packet(&mut second, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut second, &codec, &login_start("beta")).await?;
        let mut second_buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut second, &codec, &mut second_buffer, 0x30, 12).await?;
        let _ = read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x0c, 12).await?;

        write_packet(
            &mut first,
            &codec,
            &player_block_placement(2, 3, 0, 1, Some((1, 64, 0))),
        )
        .await?;
        let place_change =
            read_until_packet_id(&mut second, &codec, &mut second_buffer, 0x23, 8).await?;
        assert_eq!(block_change_from_packet(&place_change)?, (2, 4, 0, 1, 0));

        write_packet(&mut first, &codec, &player_digging(0, 2, 4, 0, 1)).await?;
        let break_change =
            read_until_packet_id(&mut second, &codec, &mut second_buffer, 0x23, 8).await?;
        assert_eq!(block_change_from_packet(&break_change)?, (2, 4, 0, 0, 0));

        server.shutdown().await
    }

    #[tokio::test]
    async fn creative_inventory_and_selected_slot_persist_across_restart()
    -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let world_dir = temp_dir.path().join("world");
        let codec = MinecraftWireCodec;

        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                game_mode: 1,
                world_dir: world_dir.clone(),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&server);

        let mut stream = connect_tcp(addr).await?;
        write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut stream, &codec, &login_start("alpha")).await?;
        let mut buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x30, 12).await?;
        let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x09, 12).await?;

        write_packet(
            &mut stream,
            &codec,
            &creative_inventory_action(36, 20, 64, 0),
        )
        .await?;
        let set_slot = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x2f, 8).await?;
        let mut set_slot_reader = PacketReader::new(&set_slot);
        assert_eq!(set_slot_reader.read_varint()?, 0x2f);
        assert_eq!(set_slot_reader.read_i8()?, 0);
        assert_eq!(set_slot_reader.read_i16()?, 36);
        assert_eq!(read_slot(&mut set_slot_reader)?, Some((20, 64, 0)));

        write_packet(&mut stream, &codec, &held_item_change(4)).await?;
        let held_slot_packet =
            read_until_packet_id(&mut stream, &codec, &mut buffer, 0x09, 8).await?;
        assert_eq!(held_item_from_packet(&held_slot_packet)?, 4);

        server.shutdown().await?;

        let restarted = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                game_mode: 1,
                world_dir,
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&restarted);
        let mut stream = connect_tcp(addr).await?;
        write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut stream, &codec, &login_start("alpha")).await?;
        let mut buffer = BytesMut::new();
        let window_items = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x30, 12).await?;
        let held_item = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x09, 12).await?;

        assert_eq!(window_items_slot(&window_items, 36)?, Some((20, 64, 0)));
        assert_eq!(held_item_from_packet(&held_item)?, 4);

        restarted.shutdown().await
    }

    #[tokio::test]
    async fn storage_profile_is_selected_independently_from_default_adapter()
    -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let codec = MinecraftWireCodec;
        let storage = Arc::new(RecordingStorageAdapter::default());
        let mut registries = RuntimeRegistries::new();
        let adapter = Arc::new(Je1710Adapter::new());
        registries.register_adapter(adapter.clone());
        registries.register_probe(adapter);
        registries.register_storage_profile("recording", storage.clone());

        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                storage_profile: "recording".to_string(),
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            registries,
        )
        .await?;
        assert_eq!(storage.load_calls.load(Ordering::SeqCst), 1);

        let addr = listener_addr(&server);
        let mut stream = connect_tcp(addr).await?;
        write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut stream, &codec, &login_start("alpha")).await?;
        let mut buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x02, 8).await?;

        server.shutdown().await?;

        assert!(storage.save_calls.load(Ordering::SeqCst) >= 1);
        Ok(())
    }

    #[tokio::test]
    async fn unsupported_creative_inventory_action_is_corrected() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                game_mode: 1,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&server);
        let codec = MinecraftWireCodec;

        let mut stream = connect_tcp(addr).await?;
        write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut stream, &codec, &login_start("alpha")).await?;
        let mut buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x30, 12).await?;

        write_packet(
            &mut stream,
            &codec,
            &creative_inventory_action(36, 999, 64, 0),
        )
        .await?;
        let set_slot = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x2f, 8).await?;
        let mut reader = PacketReader::new(&set_slot);
        assert_eq!(reader.read_varint()?, 0x2f);
        assert_eq!(reader.read_i8()?, 0);
        assert_eq!(reader.read_i16()?, 36);
        assert_eq!(read_slot(&mut reader)?, Some((1, 64, 0)));

        server.shutdown().await
    }

    #[tokio::test]
    async fn survival_place_is_rejected_with_block_and_inventory_correction()
    -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&server);
        let codec = MinecraftWireCodec;

        let mut stream = connect_tcp(addr).await?;
        write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut stream, &codec, &login_start("alpha")).await?;
        let mut buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x30, 12).await?;

        write_packet(
            &mut stream,
            &codec,
            &player_block_placement(2, 3, 0, 1, Some((1, 64, 0))),
        )
        .await?;
        let block_change = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x23, 8).await?;
        let set_slot = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x2f, 8).await?;

        assert_eq!(block_change_from_packet(&block_change)?, (2, 4, 0, 0, 0));
        let mut reader = PacketReader::new(&set_slot);
        assert_eq!(reader.read_varint()?, 0x2f);
        assert_eq!(reader.read_i8()?, 0);
        assert_eq!(reader.read_i16()?, 36);
        assert_eq!(read_slot(&mut reader)?, Some((1, 64, 0)));

        server.shutdown().await
    }

    #[tokio::test]
    async fn two_players_can_see_movement_and_restart_persists_position() -> Result<(), RuntimeError>
    {
        let temp_dir = tempdir()?;
        let world_dir = temp_dir.path().join("world");
        let codec = MinecraftWireCodec;

        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                world_dir: world_dir.clone(),
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&server);

        let mut first = connect_tcp(addr).await?;
        write_packet(&mut first, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut first, &codec, &login_start("alpha")).await?;
        let mut first_buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x08, 8).await?;

        let mut second = connect_tcp(addr).await?;
        write_packet(&mut second, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut second, &codec, &login_start("beta")).await?;
        let mut second_buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut second, &codec, &mut second_buffer, 0x08, 8).await?;
        let spawn_packet =
            read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x0c, 8).await?;
        assert_eq!(packet_id(&spawn_packet), 0x0c);

        write_packet(
            &mut second,
            &codec,
            &player_position_look(32.5, 4.0, 0.5, 90.0, 0.0),
        )
        .await?;
        let mut saw_teleport = false;
        for _ in 0..4 {
            let packet = read_packet(&mut first, &codec, &mut first_buffer).await?;
            if packet_id(&packet) == 0x18 {
                saw_teleport = true;
                break;
            }
        }
        assert!(saw_teleport);
        second.shutdown().await.ok();
        first.shutdown().await.ok();
        server.shutdown().await?;

        let restarted = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                world_dir,
                ..ServerConfig::default()
            },
            RuntimeRegistries::with_je_1_7_10(),
        )
        .await?;
        let addr = listener_addr(&restarted);
        let mut alpha = connect_tcp(addr).await?;
        write_packet(&mut alpha, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut alpha, &codec, &login_start("beta")).await?;
        let mut alpha_buffer = BytesMut::new();
        let position_packet =
            read_until_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x08, 8).await?;
        assert_eq!(packet_id(&position_packet), 0x08);
        let mut reader = PacketReader::new(&position_packet);
        assert_eq!(reader.read_varint().expect("packet id should decode"), 0x08);
        let x = reader.read_f64().expect("x should decode");
        let _y = reader.read_f64().expect("y should decode");
        let _z = reader.read_f64().expect("z should decode");
        assert!(x >= 32.0);

        restarted.shutdown().await
    }
}
