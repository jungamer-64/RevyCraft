use bytes::BytesMut;
use mc_core::{
    ConnectionId, CoreCommand, CoreConfig, CoreEvent, EventTarget, PlayerId, PlayerSummary,
    ProtocolVersion, ServerCore, TargetedEvent,
};
use mc_proto_common::{
    ConnectionPhase, HandshakeNextState, LoginRequest, MinecraftWireCodec, PacketWriter,
    ProtocolAdapter, ProtocolError, ServerListStatus, SessionEncodingContext, StatusRequest,
    WireCodec,
};
use mc_proto_je_1_7_10::Je1710Adapter;
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
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
    pub motd: String,
    pub max_players: u8,
    pub online_mode: bool,
    pub level_name: String,
    pub level_type: LevelType,
    pub game_mode: u8,
    pub difficulty: u8,
    pub view_distance: u8,
    pub world_dir: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            server_ip: None,
            server_port: 25565,
            motd: "Multi-version Rust server".to_string(),
            max_players: 20,
            online_mode: false,
            level_name: "world".to_string(),
            level_type: LevelType::Flat,
            game_mode: 0,
            difficulty: 1,
            view_distance: 2,
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
                    unknown => {
                        eprintln!("warning: ignoring unknown server.properties key `{unknown}`");
                    }
                }
            }
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        config.world_dir = parent.join(&config.level_name);
        Ok(config)
    }

    #[must_use]
    pub fn bind_addr(&self) -> SocketAddr {
        SocketAddr::new(
            self.server_ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            self.server_port,
        )
    }
}

#[derive(Clone, Default)]
pub struct VersionRegistry {
    adapters: HashMap<i32, Arc<dyn ProtocolAdapter>>,
    primary_protocol: Option<ProtocolVersion>,
}

impl VersionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_adapter(&mut self, adapter: Arc<dyn ProtocolAdapter>) -> &mut Self {
        let protocol_version = adapter.protocol_version();
        self.primary_protocol.get_or_insert(protocol_version);
        self.adapters.insert(protocol_version.0, adapter);
        self
    }

    #[must_use]
    pub fn with_je_1_7_10() -> Self {
        let mut registry = Self::new();
        let adapter: Arc<dyn ProtocolAdapter> = Arc::new(Je1710Adapter::new());
        registry.register_adapter(adapter);
        registry
    }

    #[must_use]
    pub fn resolve(&self, version: ProtocolVersion) -> Option<Arc<dyn ProtocolAdapter>> {
        self.adapters.get(&version.0).cloned()
    }

    fn primary_adapter(&self) -> Result<Arc<dyn ProtocolAdapter>, RuntimeError> {
        let protocol = self
            .primary_protocol
            .ok_or_else(|| RuntimeError::Config("no protocol adapters registered".to_string()))?;
        self.resolve(protocol)
            .ok_or_else(|| RuntimeError::Config("primary protocol adapter is missing".to_string()))
    }
}

pub struct RunningServer {
    local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<Result<(), RuntimeError>>,
}

impl RunningServer {
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
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
    phase: ConnectionPhase,
    adapter: Option<Arc<dyn ProtocolAdapter>>,
    player_id: Option<PlayerId>,
    entity_id: Option<mc_core::EntityId>,
}

struct RuntimeState {
    core: ServerCore,
    dirty: bool,
}

struct RuntimeServer {
    config: ServerConfig,
    registry: VersionRegistry,
    authenticator: Arc<dyn Authenticator>,
    storage_adapter: Arc<dyn ProtocolAdapter>,
    state: Mutex<RuntimeState>,
    sessions: Mutex<HashMap<ConnectionId, SessionHandle>>,
    next_connection_id: Mutex<u64>,
}

impl RuntimeServer {
    async fn spawn_session(self: &Arc<Self>, stream: TcpStream) {
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
            if let Err(error) = server.run_session(connection_id, stream, rx).await {
                eprintln!("session {connection_id:?} ended with error: {error}");
            }
        });
    }

    async fn run_session(
        self: Arc<Self>,
        connection_id: ConnectionId,
        stream: TcpStream,
        mut rx: mpsc::UnboundedReceiver<SessionMessage>,
    ) -> Result<(), RuntimeError> {
        let (mut reader, mut writer) = stream.into_split();
        let default_codec = MinecraftWireCodec;
        let mut read_buffer = BytesMut::with_capacity(8192);
        let mut session = SessionState {
            phase: ConnectionPhase::Handshaking,
            adapter: None,
            player_id: None,
            entity_id: None,
        };

        loop {
            tokio::select! {
            read = reader.read_buf(&mut read_buffer) => {
                let bytes_read = read?;
                if bytes_read == 0 {
                    break;
                }
                loop {
                    let codec: &dyn WireCodec = session
                        .adapter
                        .as_ref()
                        .map_or(&default_codec, |current| current.wire_codec());
                    let Some(frame) = codec.try_decode_frame(&mut read_buffer)? else {
                        break;
                    };
                    let should_close = self
                        .handle_incoming_frame(
                            connection_id,
                            &mut writer,
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
                            &mut writer,
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
        writer: &mut tokio::net::tcp::OwnedWriteHalf,
        session: &mut SessionState,
        frame: Vec<u8>,
    ) -> Result<bool, RuntimeError> {
        match session.phase {
            ConnectionPhase::Handshaking => {
                self.handle_handshake_frame(writer, session, &frame).await
            }
            ConnectionPhase::Status => self.handle_status_frame(writer, session, &frame).await,
            ConnectionPhase::Login => {
                self.handle_login_frame(connection_id, writer, session, &frame)
                    .await
            }
            ConnectionPhase::Play => self.handle_play_frame(session, &frame).await,
        }
    }

    async fn handle_handshake_frame(
        &self,
        writer: &mut tokio::net::tcp::OwnedWriteHalf,
        session: &mut SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let intent = handshake_adapter().decode_handshake(frame)?;
        let next_phase = match intent.next_state {
            HandshakeNextState::Status => ConnectionPhase::Status,
            HandshakeNextState::Login => ConnectionPhase::Login,
        };
        if let Some(next_adapter) = self.registry.resolve(intent.protocol_version) {
            session.adapter = Some(next_adapter);
            session.phase = next_phase;
            return Ok(false);
        }

        let fallback = self.registry.primary_adapter()?;
        match next_phase {
            ConnectionPhase::Status => {
                session.adapter = Some(fallback);
                session.phase = ConnectionPhase::Status;
                Ok(false)
            }
            ConnectionPhase::Login => {
                let disconnect = fallback.encode_disconnect(
                    ConnectionPhase::Login,
                    &format!(
                        "Unsupported protocol {}. This server supports {} (protocol {}).",
                        intent.protocol_version.0,
                        fallback.version_name(),
                        fallback.protocol_version().0
                    ),
                )?;
                write_payload(writer, fallback.wire_codec(), &disconnect).await?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    async fn handle_status_frame(
        &self,
        writer: &mut tokio::net::tcp::OwnedWriteHalf,
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
                    version_name: current.version_name().to_string(),
                    protocol: current.protocol_version(),
                    players_online: summary.online_players,
                    max_players: usize::from(summary.max_players),
                    description: self.config.motd.clone(),
                })?;
                write_payload(writer, current.wire_codec(), &response).await?;
                Ok(false)
            }
            StatusRequest::Ping { payload } => {
                let response = current.encode_status_pong(payload)?;
                write_payload(writer, current.wire_codec(), &response).await?;
                Ok(true)
            }
        }
    }

    async fn handle_login_frame(
        &self,
        connection_id: ConnectionId,
        writer: &mut tokio::net::tcp::OwnedWriteHalf,
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
                    protocol_version: current.protocol_version(),
                    username,
                    player_id: authenticated,
                })
                .await?;
                Ok(false)
            }
            LoginRequest::EncryptionResponse => {
                let disconnect = current
                    .encode_disconnect(ConnectionPhase::Login, "online-mode is not implemented")?;
                write_payload(writer, current.wire_codec(), &disconnect).await?;
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
        connection_id: ConnectionId,
        writer: &mut tokio::net::tcp::OwnedWriteHalf,
        session: &mut SessionState,
        message: SessionMessage,
    ) -> Result<bool, RuntimeError> {
        let SessionMessage::Event(event) = message;
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        let context = SessionEncodingContext {
            connection_id,
            phase: session.phase,
            player_id: session.player_id,
            entity_id: session.entity_id,
        };
        let packets = current.encode_event(&event, &context)?;
        for packet in packets {
            write_payload(writer, current.wire_codec(), &packet).await?;
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
            .storage_adapter()
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

fn handshake_adapter() -> impl ProtocolAdapter {
    Je1710Adapter::new()
}

/// # Errors
///
/// Returns [`RuntimeError`] when the server cannot bind, load its persisted
/// world state, or starts with unsupported configuration such as
/// `online-mode=true`.
pub async fn spawn_server(
    config: ServerConfig,
    registry: VersionRegistry,
) -> Result<RunningServer, RuntimeError> {
    if config.online_mode {
        return Err(RuntimeError::Unsupported(
            "online-mode=true is not implemented".to_string(),
        ));
    }
    let listener = TcpListener::bind(config.bind_addr()).await?;
    let local_addr = listener.local_addr()?;
    let storage_adapter = registry.primary_adapter()?;
    let snapshot = storage_adapter
        .storage_adapter()
        .load_snapshot(&config.world_dir)?;
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

    let server = Arc::new(RuntimeServer {
        config,
        registry,
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
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    run_server.maybe_save().await?;
                    return Ok(());
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    run_server.spawn_session(stream).await;
                }
                _ = tick_interval.tick() => {
                    run_server.tick().await?;
                }
                _ = save_interval.tick() => {
                    run_server.maybe_save().await?;
                }
            }
        }
    });

    Ok(RunningServer {
        local_addr,
        shutdown_tx: Some(shutdown_tx),
        join_handle,
    })
}

async fn write_payload(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    codec: &dyn WireCodec,
    payload: &[u8],
) -> Result<(), RuntimeError> {
    let frame = codec.encode_frame(payload)?;
    writer.write_all(&frame).await?;
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

/// # Errors
///
/// Returns [`RuntimeError`] when the TCP connection cannot be established.
pub async fn connect(addr: SocketAddr) -> Result<TcpStream, RuntimeError> {
    Ok(TcpStream::connect(addr).await?)
}

#[cfg(test)]
mod tests {
    use super::{
        LevelType, RuntimeError, ServerConfig, VersionRegistry, connect, encode_handshake,
        spawn_server,
    };
    use bytes::BytesMut;
    use mc_proto_common::{MinecraftWireCodec, PacketReader, PacketWriter, WireCodec};
    use std::fs;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn write_packet(
        stream: &mut tokio::net::TcpStream,
        codec: &MinecraftWireCodec,
        payload: &[u8],
    ) -> Result<(), RuntimeError> {
        let frame = codec.encode_frame(payload)?;
        stream.write_all(&frame).await?;
        Ok(())
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
        for _ in 0..max_attempts {
            let packet = read_packet(stream, codec, buffer).await?;
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
    fn version_registry_resolves_registered_adapter() {
        let registry = VersionRegistry::with_je_1_7_10();
        let adapter = registry
            .resolve(mc_core::ProtocolVersion(5))
            .expect("registered adapter should resolve");
        assert_eq!(adapter.version_name(), "1.7.10");
        assert_eq!(
            registry
                .primary_adapter()
                .expect("primary adapter should be available")
                .protocol_version(),
            mc_core::ProtocolVersion(5)
        );
    }

    #[test]
    fn server_properties_accept_flat_level_type() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let path = temp_dir.path().join("server.properties");
        fs::write(
            &path,
            "level-name=flatland\nlevel-type=FLAT\nonline-mode=false\n",
        )?;

        let config = ServerConfig::from_properties(&path)?;

        assert_eq!(config.level_name, "flatland");
        assert_eq!(config.level_type, LevelType::Flat);
        assert_eq!(config.world_dir, temp_dir.path().join("flatland"));
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
        let mut reader = PacketReader::new(packet);
        if reader.read_varint()? != 0x30 {
            return Err(RuntimeError::Config(
                "expected window items packet".to_string(),
            ));
        }
        let _window_id = reader.read_i8()?;
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
            VersionRegistry::with_je_1_7_10(),
        )
        .await?;
        let addr = server.local_addr();
        let codec = MinecraftWireCodec;

        let mut status_stream = connect(addr).await?;
        write_packet(&mut status_stream, &codec, &encode_handshake(5, 1)?).await?;
        write_packet(&mut status_stream, &codec, &status_request()).await?;
        let mut buffer = BytesMut::new();
        let status_response = read_packet(&mut status_stream, &codec, &mut buffer).await?;
        assert_eq!(packet_id(&status_response), 0x00);
        write_packet(&mut status_stream, &codec, &status_ping(42)).await?;
        let pong = read_packet(&mut status_stream, &codec, &mut buffer).await?;
        assert_eq!(packet_id(&pong), 0x01);

        let mut login_stream = connect(addr).await?;
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
            VersionRegistry::with_je_1_7_10(),
        )
        .await?;
        let addr = server.local_addr();
        let codec = MinecraftWireCodec;

        let mut stream = connect(addr).await?;
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
            VersionRegistry::with_je_1_7_10(),
        )
        .await?;
        let addr = server.local_addr();
        let codec = MinecraftWireCodec;

        let mut stream = connect(addr).await?;
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

    #[tokio::test]
    async fn online_mode_fails_fast() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let result = spawn_server(
            ServerConfig {
                online_mode: true,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            VersionRegistry::with_je_1_7_10(),
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
    async fn unsupported_login_protocol_receives_disconnect() -> Result<(), RuntimeError> {
        let temp_dir = tempdir()?;
        let server = spawn_server(
            ServerConfig {
                server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
                server_port: 0,
                world_dir: temp_dir.path().join("world"),
                ..ServerConfig::default()
            },
            VersionRegistry::with_je_1_7_10(),
        )
        .await?;
        let addr = server.local_addr();
        let codec = MinecraftWireCodec;

        let mut stream = connect(addr).await?;
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
            VersionRegistry::with_je_1_7_10(),
        )
        .await?;
        let addr = server.local_addr();
        let codec = MinecraftWireCodec;

        let mut first = connect(addr).await?;
        write_packet(&mut first, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut first, &codec, &login_start("alpha")).await?;
        let mut first_buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x30, 12).await?;

        let mut second = connect(addr).await?;
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
            VersionRegistry::with_je_1_7_10(),
        )
        .await?;
        let addr = server.local_addr();

        let mut stream = connect(addr).await?;
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
            VersionRegistry::with_je_1_7_10(),
        )
        .await?;
        let addr = restarted.local_addr();
        let mut stream = connect(addr).await?;
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
            VersionRegistry::with_je_1_7_10(),
        )
        .await?;
        let addr = server.local_addr();
        let codec = MinecraftWireCodec;

        let mut stream = connect(addr).await?;
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
            VersionRegistry::with_je_1_7_10(),
        )
        .await?;
        let addr = server.local_addr();
        let codec = MinecraftWireCodec;

        let mut stream = connect(addr).await?;
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
            VersionRegistry::with_je_1_7_10(),
        )
        .await?;
        let addr = server.local_addr();

        let mut first = connect(addr).await?;
        write_packet(&mut first, &codec, &encode_handshake(5, 2)?).await?;
        write_packet(&mut first, &codec, &login_start("alpha")).await?;
        let mut first_buffer = BytesMut::new();
        let _ = read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x08, 8).await?;

        let mut second = connect(addr).await?;
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
            VersionRegistry::with_je_1_7_10(),
        )
        .await?;
        let addr = restarted.local_addr();
        let mut alpha = connect(addr).await?;
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
