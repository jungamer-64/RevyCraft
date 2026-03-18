#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::missing_errors_doc,
    clippy::module_name_repetitions
)]

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
                        eprintln!("warning: ignoring unknown server.properties key `{unknown}`")
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
}

impl VersionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_adapter(&mut self, adapter: Arc<dyn ProtocolAdapter>) -> &mut Self {
        self.adapters.insert(adapter.protocol_version().0, adapter);
        self
    }

    #[must_use]
    pub fn with_je_1_7_10() -> Self {
        let mut registry = Self::new();
        let adapter: Arc<dyn ProtocolAdapter> = Arc::new(Je1710Adapter::new());
        registry.register_adapter(adapter);
        registry
    }

    pub fn resolve(&self, version: ProtocolVersion) -> Option<Arc<dyn ProtocolAdapter>> {
        self.adapters.get(&version.0).cloned()
    }

    fn primary_adapter(&self) -> Result<Arc<dyn ProtocolAdapter>, RuntimeError> {
        self.adapters
            .values()
            .next()
            .cloned()
            .ok_or_else(|| RuntimeError::Config("no protocol adapters registered".to_string()))
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
        let mut phase = ConnectionPhase::Handshaking;
        let mut adapter: Option<Arc<dyn ProtocolAdapter>> = None;
        let mut read_buffer = BytesMut::with_capacity(8192);
        let mut player_id = None;
        let mut entity_id = None;

        loop {
            tokio::select! {
                read = reader.read_buf(&mut read_buffer) => {
                    let bytes_read = read?;
                    if bytes_read == 0 {
                        break;
                    }
                    loop {
                        let codec: &dyn WireCodec = adapter
                            .as_ref()
                            .map_or(&default_codec, |current| current.wire_codec());
                        let Some(frame) = codec.try_decode_frame(&mut read_buffer)? else {
                            break;
                        };
                        let should_close = self
                            .handle_incoming_frame(
                                connection_id,
                                &mut writer,
                                &mut phase,
                                &mut adapter,
                                &mut player_id,
                                &mut entity_id,
                                frame,
                            )
                            .await?;
                        if should_close {
                            self.unregister_session(connection_id, player_id).await?;
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
                            &mut phase,
                            &mut adapter,
                            &mut player_id,
                            &mut entity_id,
                            message,
                        )
                        .await?;
                    if should_close {
                        self.unregister_session(connection_id, player_id).await?;
                        return Ok(());
                    }
                }
            }
        }

        self.unregister_session(connection_id, player_id).await?;
        Ok(())
    }

    async fn handle_incoming_frame(
        &self,
        connection_id: ConnectionId,
        writer: &mut tokio::net::tcp::OwnedWriteHalf,
        phase: &mut ConnectionPhase,
        adapter: &mut Option<Arc<dyn ProtocolAdapter>>,
        player_id: &mut Option<PlayerId>,
        _entity_id: &mut Option<mc_core::EntityId>,
        frame: Vec<u8>,
    ) -> Result<bool, RuntimeError> {
        match phase {
            ConnectionPhase::Handshaking => {
                let intent = handshake_adapter().decode_handshake(&frame)?;
                let Some(next_adapter) = self.registry.resolve(intent.protocol_version) else {
                    return Ok(true);
                };
                *adapter = Some(next_adapter);
                *phase = match intent.next_state {
                    HandshakeNextState::Status => ConnectionPhase::Status,
                    HandshakeNextState::Login => ConnectionPhase::Login,
                };
                Ok(false)
            }
            ConnectionPhase::Status => {
                let current = adapter
                    .as_ref()
                    .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
                match current.decode_status(&frame)? {
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
                    }
                    StatusRequest::Ping { payload } => {
                        let response = current.encode_status_pong(payload)?;
                        write_payload(writer, current.wire_codec(), &response).await?;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            ConnectionPhase::Login => {
                let current = adapter
                    .as_ref()
                    .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
                match current.decode_login(&frame)? {
                    LoginRequest::LoginStart { username } => {
                        let authenticated = self.authenticator.authenticate(&username)?;
                        self.apply_command(CoreCommand::LoginStart {
                            connection_id,
                            protocol_version: current.protocol_version(),
                            username,
                            player_id: authenticated,
                        })
                        .await?;
                    }
                    LoginRequest::EncryptionResponse => {
                        let disconnect = current.encode_disconnect(
                            ConnectionPhase::Login,
                            "online-mode is not implemented",
                        )?;
                        write_payload(writer, current.wire_codec(), &disconnect).await?;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            ConnectionPhase::Play => {
                let current = adapter
                    .as_ref()
                    .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
                let Some(current_player_id) = player_id else {
                    return Ok(true);
                };
                if let Some(command) = current.decode_play(*current_player_id, &frame)? {
                    self.apply_command(command).await?;
                }
                Ok(false)
            }
        }
    }

    async fn handle_outgoing_message(
        &self,
        connection_id: ConnectionId,
        writer: &mut tokio::net::tcp::OwnedWriteHalf,
        phase: &mut ConnectionPhase,
        adapter: &mut Option<Arc<dyn ProtocolAdapter>>,
        player_id: &mut Option<PlayerId>,
        entity_id: &mut Option<mc_core::EntityId>,
        message: SessionMessage,
    ) -> Result<bool, RuntimeError> {
        let SessionMessage::Event(event) = message;
        let current = adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        let context = SessionEncodingContext {
            connection_id,
            phase: *phase,
            player_id: *player_id,
            entity_id: *entity_id,
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
                *player_id = Some(accepted_player_id);
                *entity_id = Some(accepted_entity_id);
                *phase = ConnectionPhase::Play;
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
        Some(snapshot) => ServerCore::from_snapshot(core_config.clone(), snapshot),
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn encode_handshake(protocol_version: i32, next_state: i32) -> Result<Vec<u8>, RuntimeError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    writer.write_varint(protocol_version);
    writer.write_string("localhost")?;
    writer.write_u16(25565);
    writer.write_varint(next_state);
    Ok(writer.into_inner())
}

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
        let mut saw_chunk_bulk = false;
        for _ in 0..4 {
            let packet = read_packet(&mut login_stream, &codec, &mut login_buffer).await?;
            if packet_id(&packet) == 0x26 {
                saw_chunk_bulk = true;
                break;
            }
        }
        assert!(saw_chunk_bulk);

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
        let error = match result {
            Ok(_) => panic!("online-mode should fail fast"),
            Err(error) => error,
        };
        assert!(
            matches!(error, RuntimeError::Unsupported(message) if message.contains("online-mode=true"))
        );
        Ok(())
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
