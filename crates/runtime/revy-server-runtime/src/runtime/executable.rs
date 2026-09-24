use super::authority::FrozenDataPlane;
use super::core_store::CORE_PROCESS_DELTA_SLOT_BYTES;
use super::session_registry::{FrozenProcessSessionStates, PreparedProcessSessionSockets};
use super::topology_resources::FrozenTopologyIngress;
use super::{
    ActiveGeneration, BoundSession, CoreProcessDeltaSeal, CoreProcessPrecopy, GenerationId,
    LoginChallenge, LoginSession, PlaySession, RuntimeServer, ServerSupervisor, SessionMessage,
    SessionPhase, SessionStatusSnapshot,
};
use crate::config::{ServerConfigSource, ValidatedServerConfig};
use crate::transport::{
    FrozenProcessTransportState, MinecraftStreamCipherSnapshot, PreparedProcessTransportSocket,
    TransportSessionIo,
};
use crate::{
    CutoverConnectionMix, CutoverOperation, CutoverOutcome, CutoverReport, RuntimeError,
    RuntimeUpgradePhase, RuntimeUpgradeRole,
};
use bytes::BytesMut;
use mc_plugin_host::runtime::RuntimePluginHost;
use mc_proto_common::{ConnectionPhase, TransportKind};
use revy_raknet::{
    Bound as RakNetBound, FrozenPeer as FrozenRakNetPeer, PeerSnapshot, RakNetBudgets, RakNetPeer,
    RakNetServer, ReceivePaused as RakNetReceivePaused, ServerConfig as RakNetServerConfig,
    ValidatedPeerSnapshot, validate_peer_snapshots,
};
use revy_runtime_transfer::{
    ArenaError, ArenaReservation, BootstrapV1, CommitV1, ExportedSocket, PrestageV1,
    ResourceDescriptorV1, ResourceKindV1, SealedArenaRegion, SharedTransferArena,
    SharedTransferArenaReader, SocketTransferTarget,
};
use revy_voxel_core::{
    ConnectionId, CoreEvent, CoreRevision, CoreTransferDeltaDescriptor, CoreVersion, EntityId,
    GameplayProfileId, PlayerId, PluginGenerationId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Cursor, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_EXECUTABLE_DIRECTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_EXECUTABLE_SESSION_STATE_BYTES: usize = 4 * 1024 * 1024;
const MAX_EXECUTABLE_SESSION_READ_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_EXECUTABLE_SESSION_DIAGNOSTIC_BYTES: usize = 4 * 1024;
const EXECUTABLE_SESSION_STATE_FIXED_SLOT_BYTES: usize = 256 * 1024;
const EXECUTABLE_RAKNET_ROUTER_STATE_BYTES: usize = 16 * 1024 * 1024;
const EXECUTABLE_SESSION_PREPARE_WORKER_LIMIT: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutablePluginArtifact {
    pub plugin_id: String,
    pub sha256: [u8; 32],
}

pub struct ExecutableUpgradeStaged {
    runtime: Arc<RuntimeServer>,
    _serial_authority: tokio::sync::OwnedMutexGuard<()>,
    status_authority: ExecutableUpgradeStatusAuthority,
    epoch_revision: u64,
    active_generation_id: GenerationId,
    validated_config_digest: [u8; 32],
    plugin_artifacts: Vec<ExecutablePluginArtifact>,
    arena: Arc<SharedTransferArena>,
    core_snapshot_revision: CoreRevision,
    core_snapshot_region: SealedArenaRegion,
    initial_directory: ExecutableDirectoryPrestage,
    online_auth_keys_region: Option<SealedArenaRegion>,
    final_core_delta_slot: tokio::sync::Mutex<Option<ArenaReservation>>,
    raknet_router_state_slot: tokio::sync::Mutex<Option<ArenaReservation>>,
    persisted_core_revision: CoreRevision,
    latest_dirty_core_revision: Option<CoreRevision>,
    stage_duration: Duration,
}

pub struct ExecutableUpgradePreparing {
    staged: ExecutableUpgradeStaged,
    started_at: Instant,
}

/// An immutable core update sent while the parent retains data-plane authority. The parent
/// consumes the acknowledged update at freeze, fixing the base of the final delta.
pub struct ExecutableCorePrestage {
    epoch_revision: u64,
    revision: CoreRevision,
    kind: ResourceKindV1,
    region: SealedArenaRegion,
}

impl ExecutableCorePrestage {
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision.value()
    }

    #[must_use]
    pub const fn kind(&self) -> ResourceKindV1 {
        self.kind
    }

    #[must_use]
    pub const fn region(&self) -> &SealedArenaRegion {
        &self.region
    }
}

pub struct ExecutableNetworkPrestage {
    target: SocketTransferTarget,
    directory_revision: u64,
    listeners: Vec<super::ExecutableListenerResource>,
    sessions: Vec<(ConnectionId, PreparedProcessTransportSocket)>,
}

pub struct ExecutableNetworkPrepared {
    directory_revision: u64,
    listeners: Vec<crate::ListenerBinding>,
    prepared_sessions: HashMap<ConnectionId, TransportKind>,
    frozen_sessions: Vec<ExecutableSessionTransportResource>,
    frozen_states: Vec<ExecutableSessionStateResource>,
}

pub struct ExecutableFrozenNetwork {
    listeners: Vec<crate::ListenerBinding>,
    sessions: Vec<ExecutableSessionTransportResource>,
    session_states: Vec<ExecutableSessionStateResource>,
    raknet_router_state: Option<SealedArenaRegion>,
}

pub struct ExecutableSessionStateResource {
    connection_id: ConnectionId,
    region: SealedArenaRegion,
}

pub enum ExecutableSessionTransportResource {
    Tcp(ExecutableTcpSessionTransport),
    Bedrock(ExecutableBedrockSessionTransport),
}

pub struct ExecutableTcpSessionTransport {
    actor: ExecutableSessionActorSnapshot,
    snapshot: ExecutableTcpTransportSnapshot,
}

pub struct ExecutableBedrockSessionTransport {
    actor: ExecutableSessionActorSnapshot,
    snapshot: ExecutableBedrockTransportSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutableSessionActorSnapshot {
    pub(crate) connection_id: ConnectionId,
    pub(crate) epoch_revision: u64,
    pub(crate) generation_id: GenerationId,
    pub(crate) phase: ExecutableSessionPhase,
    pub(crate) read_buffer: Vec<u8>,
    pub(crate) queued_messages: Vec<ExecutableQueuedSessionMessage>,
    pub(crate) pending_terminate: Option<String>,
    pub(crate) protocol_session_blob: Vec<u8>,
    pub(crate) gameplay_session_blob: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ExecutableSessionPhase {
    Handshaking,
    Status(ExecutableSessionBinding),
    Login(ExecutableLoginSession),
    Play {
        binding: ExecutableSessionBinding,
        player_id: PlayerId,
        entity_id: EntityId,
    },
    Closing {
        transport: TransportKind,
        phase: ConnectionPhase,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutableSessionBinding {
    transport: TransportKind,
    adapter_id: String,
    gameplay_profile: GameplayProfileId,
    protocol_generation: Option<PluginGenerationId>,
    gameplay_generation: Option<PluginGenerationId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutableLoginSession {
    Negotiating {
        binding: ExecutableSessionBinding,
    },
    Authenticating {
        binding: ExecutableSessionBinding,
        username: String,
        verify_token: [u8; super::LOGIN_VERIFY_TOKEN_LEN],
        auth_generation: PluginGenerationId,
    },
    AcceptedWritePending {
        binding: ExecutableSessionBinding,
        player_id: PlayerId,
        entity_id: EntityId,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ExecutableQueuedSessionMessage {
    Events(Vec<CoreEvent>),
    Terminate { reason: String },
}

pub(crate) struct FrozenProcessSessionState {
    pub(crate) state: ExecutableSessionState,
    pub(crate) region: SealedArenaRegion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutableMinecraftCipherSnapshot {
    shared_secret: [u8; 16],
    shift_register: [u8; 16],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutableTcpTransportSnapshot {
    decrypt: Option<ExecutableMinecraftCipherSnapshot>,
    encrypt: Option<ExecutableMinecraftCipherSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutableBedrockTransportSnapshot {
    peer: PeerSnapshot,
    reader_compression_threshold: Option<u16>,
    writer_compression_threshold: Option<u16>,
}

/// Bounded, socket-independent state for one frozen executable-transfer session.
///
/// Native TCP socket authority is transferred separately. Bedrock sessions share the imported
/// UDP listener and identify their frozen RakNet peer through this payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ExecutableSessionState {
    Tcp {
        actor: ExecutableSessionActorSnapshot,
        transport: ExecutableTcpTransportSnapshot,
    },
    Bedrock {
        actor: ExecutableSessionActorSnapshot,
        transport: ExecutableBedrockTransportSnapshot,
    },
}

pub struct ExecutableSealedCoreDelta {
    descriptor: CoreTransferDeltaDescriptor,
    region: SealedArenaRegion,
    persisted_revision: CoreRevision,
    latest_dirty_revision: Option<CoreRevision>,
}

pub struct ExecutableUpgradeFrozen {
    preparing: ExecutableUpgradePreparing,
    ingress: FrozenTopologyIngress,
    data_plane: FrozenDataPlane,
    final_directory: ExecutableDirectoryPrestage,
    core_delta: ExecutableSealedCoreDelta,
    network: ExecutableFrozenNetwork,
    connection_mix: CutoverConnectionMix,
    session_count: usize,
    prepare_duration: Duration,
}

pub struct ExecutableUpgradeOutcomePending {
    frozen: ExecutableUpgradeFrozen,
}

pub struct ExecutableParentCommitHold {
    _preparing: ExecutableUpgradePreparing,
    _ingress: FrozenTopologyIngress,
    _data_plane: FrozenDataPlane,
}

pub struct ExecutableChildPrepared {
    config_source: ServerConfigSource,
    config: ValidatedServerConfig,
    plugin_host: Arc<mc_plugin_host::host::PluginHost>,
    active_protocols: super::bootstrap::ActiveProtocols,
    selection: super::selection::ResolvedRuntimeSelection,
    online_auth_keys: Option<Arc<super::OnlineAuthKeys>>,
    pending_core: Arc<CoreVersion>,
    persisted_revision: CoreRevision,
    latest_dirty_revision: Option<CoreRevision>,
    directory: ExecutableSessionDirectory,
    prepared_play_phases: BTreeMap<ConnectionId, SessionPhase>,
    raknet_router_peers: Vec<ValidatedPeerSnapshot>,
    active_generation_id: GenerationId,
}

pub struct ExecutableChildSessionsPrepared {
    prepared: ExecutableChildPrepared,
    sessions: Vec<PreparedExecutableSession>,
}

pub struct ExecutableChildCommit {
    config_source: ServerConfigSource,
    config: ValidatedServerConfig,
    plugin_host: Arc<mc_plugin_host::host::PluginHost>,
    active_protocols: super::bootstrap::ActiveProtocols,
    selection: super::selection::ResolvedRuntimeSelection,
    online_auth_keys: Option<Arc<super::OnlineAuthKeys>>,
    core: Arc<CoreVersion>,
    persisted_revision: CoreRevision,
    latest_dirty_revision: Option<CoreRevision>,
    directory: ExecutableSessionDirectory,
    active_generation_id: GenerationId,
    parent_epoch_revision: u64,
    cutover_context: ExecutableChildCutoverContext,
    tcp_listener: tokio::net::TcpListener,
    bedrock_listener: Option<RakNetServer<RakNetReceivePaused>>,
    queued_bedrock_peers: Vec<RakNetPeer<FrozenRakNetPeer>>,
    sessions: Vec<PreparedImportedSession>,
}

pub(in crate::runtime) struct ExecutableChildBootParts {
    pub(in crate::runtime) config_source: ServerConfigSource,
    pub(in crate::runtime) config: ValidatedServerConfig,
    pub(in crate::runtime) plugin_host: Arc<mc_plugin_host::host::PluginHost>,
    pub(in crate::runtime) active_protocols: super::bootstrap::ActiveProtocols,
    pub(in crate::runtime) selection: super::selection::ResolvedRuntimeSelection,
    pub(in crate::runtime) online_auth_keys: Option<Arc<super::OnlineAuthKeys>>,
    pub(in crate::runtime) core: Arc<CoreVersion>,
    pub(in crate::runtime) persisted_revision: CoreRevision,
    pub(in crate::runtime) latest_dirty_revision: Option<CoreRevision>,
    pub(in crate::runtime) active_generation_id: GenerationId,
    pub(in crate::runtime) parent_epoch_revision: u64,
    pub(in crate::runtime) cutover_context: ExecutableChildCutoverContext,
    pub(in crate::runtime) tcp_listener: tokio::net::TcpListener,
    pub(in crate::runtime) bedrock_listener: Option<RakNetServer<RakNetReceivePaused>>,
    pub(in crate::runtime) queued_bedrock_peers: Vec<RakNetPeer<FrozenRakNetPeer>>,
    pub(in crate::runtime) sessions: Vec<PreparedImportedSession>,
}

pub struct ExecutableChildRuntimePrepared {
    running: super::RunningServer,
    data_plane: Option<FrozenDataPlane>,
    activation_receivers: Vec<tokio::sync::oneshot::Receiver<Result<(), String>>>,
    queued_bedrock_peers: Vec<RakNetPeer<FrozenRakNetPeer>>,
    generation_id: GenerationId,
    cutover_context: ExecutableChildCutoverContext,
}

pub struct ExecutableChildActivated {
    server: Arc<ServerSupervisor>,
    cutover_context: ExecutableChildCutoverContext,
    resume_duration: Duration,
}

pub(in crate::runtime) struct ExecutableChildCutoverContext {
    stage_us: u64,
    prepare_us: u64,
    connection_mix: CutoverConnectionMix,
    session_count: usize,
    child_epoch_revision: u64,
}

pub struct ExecutableChildSocketResources {
    listeners: Vec<(TransportKind, ExportedSocket)>,
    tcp_sessions: Vec<(ConnectionId, ExportedSocket)>,
}

pub struct ExecutableChildNativeSockets {
    tcp_listener: tokio::net::TcpListener,
    udp_socket: Option<Arc<tokio::net::UdpSocket>>,
    tcp_sessions: BTreeMap<ConnectionId, TransportSessionIo>,
}

pub struct ExecutableChildNetworkPrepared {
    pub(crate) prepared: ExecutableChildPrepared,
    pub(crate) tcp_listener: tokio::net::TcpListener,
    pub(crate) bedrock_listener: Option<RakNetServer<RakNetReceivePaused>>,
    pub(crate) queued_bedrock_peers: Vec<RakNetPeer<FrozenRakNetPeer>>,
    pub(crate) sessions: Vec<PreparedImportedSession>,
}

pub(crate) struct PreparedImportedSession {
    pub(crate) actor: ExecutableSessionActorSnapshot,
    pub(crate) phase: SessionPhase,
    pub(crate) io: TransportSessionIo,
}

struct PreparedExecutableSession {
    state: ValidatedExecutableSessionState,
    phase: SessionPhase,
}

enum ExecutableSessionImportBinding {
    Prepared(SessionPhase),
    // Non-Play phases need the final login/status state before binding can finish.
    Pending(Arc<ActiveGeneration>),
}

enum ValidatedExecutableSessionState {
    Tcp {
        actor: ExecutableSessionActorSnapshot,
        transport: ExecutableTcpTransportSnapshot,
    },
    Bedrock {
        actor: ExecutableSessionActorSnapshot,
        peer: ValidatedPeerSnapshot,
        reader_compression_threshold: Option<u16>,
        writer_compression_threshold: Option<u16>,
    },
}

struct ExecutableUpgradeStatusAuthority {
    runtime: Arc<RuntimeServer>,
    id: u64,
    clear_on_drop: bool,
}

enum ExecutableCoreDelta {
    Ready {
        descriptor: CoreTransferDeltaDescriptor,
        region: SealedArenaRegion,
        persisted_revision: CoreRevision,
        latest_dirty_revision: Option<CoreRevision>,
    },
    Outpaced {
        requested_revision: CoreRevision,
        earliest_revision: CoreRevision,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutableSessionDirectory {
    revision: u64,
    sessions: Vec<SessionStatusSnapshot>,
}

impl ExecutableSessionDirectory {
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn sessions(&self) -> &[SessionStatusSnapshot] {
        &self.sessions
    }

    /// Decodes a bounded directory projection received through the transfer arena.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the payload exceeds policy, is malformed, or contains
    /// trailing data.
    pub fn decode(bytes: &[u8]) -> Result<Self, RuntimeError> {
        if bytes.len() > MAX_EXECUTABLE_DIRECTORY_BYTES {
            return Err(RuntimeError::BudgetExceeded {
                resource: "executable session directory bytes",
                requested: bytes.len(),
                limit: MAX_EXECUTABLE_DIRECTORY_BYTES,
            });
        }
        let mut reader = Cursor::new(bytes);
        let directory: Self = ciborium::de::from_reader(&mut reader)
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        if usize::try_from(reader.position()).ok() != Some(bytes.len()) {
            return Err(RuntimeError::Config(
                "executable session directory contains trailing bytes".to_string(),
            ));
        }
        Ok(directory)
    }
}

pub struct ExecutableDirectoryPrestage {
    directory: ExecutableSessionDirectory,
    region: SealedArenaRegion,
}

impl ExecutableDirectoryPrestage {
    #[must_use]
    pub const fn directory(&self) -> &ExecutableSessionDirectory {
        &self.directory
    }

    #[must_use]
    pub const fn region(&self) -> &SealedArenaRegion {
        &self.region
    }
}

impl ExecutableNetworkPrestage {
    #[must_use]
    pub const fn target(&self) -> SocketTransferTarget {
        self.target
    }

    #[must_use]
    pub const fn directory_revision(&self) -> u64 {
        self.directory_revision
    }

    #[must_use]
    pub fn listeners(&self) -> &[super::ExecutableListenerResource] {
        &self.listeners
    }

    /// Consumes duplicated descriptors into the child delivery payload and reserves the complete
    /// session index and output capacity before the parent freezes the data plane.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if the prepared sessions contain duplicate identities or the
    /// bounded transfer metadata cannot be allocated.
    pub fn transfer_sockets(
        self,
    ) -> Result<(ExecutableNetworkPrepared, ExecutableChildSocketResources), RuntimeError> {
        let (child_listeners, listeners) = self
            .listeners
            .into_iter()
            .map(|resource| {
                let binding = resource.binding().clone();
                ((binding.transport, resource.into_socket()), binding)
            })
            .unzip();
        let session_count = self.sessions.len();
        let mut prepared_sessions = HashMap::new();
        prepared_sessions
            .try_reserve(session_count)
            .map_err(|_error| RuntimeError::Allocation {
                resource: "prepared executable session index",
                requested: session_count
                    .saturating_mul(std::mem::size_of::<(ConnectionId, TransportKind)>()),
            })?;
        let mut frozen_sessions = Vec::new();
        frozen_sessions
            .try_reserve_exact(session_count)
            .map_err(|_error| RuntimeError::Allocation {
                resource: "frozen executable transport descriptors",
                requested: session_count
                    .saturating_mul(std::mem::size_of::<ExecutableSessionTransportResource>()),
            })?;
        let mut frozen_states = Vec::new();
        frozen_states
            .try_reserve_exact(session_count)
            .map_err(|_error| RuntimeError::Allocation {
                resource: "frozen executable session descriptors",
                requested: session_count
                    .saturating_mul(std::mem::size_of::<ExecutableSessionStateResource>()),
            })?;
        let mut tcp_sessions = Vec::new();
        for (connection_id, socket) in self.sessions {
            let transport = match &socket {
                PreparedProcessTransportSocket::Tcp(_) => TransportKind::Tcp,
                PreparedProcessTransportSocket::Bedrock => TransportKind::Udp,
            };
            if prepared_sessions.insert(connection_id, transport).is_some() {
                return Err(RuntimeError::Config(format!(
                    "process-transfer network preparation duplicated session {connection_id:?}"
                )));
            }
            match socket {
                PreparedProcessTransportSocket::Tcp(socket) => {
                    tcp_sessions.push((connection_id, socket));
                }
                PreparedProcessTransportSocket::Bedrock => {}
            }
        }
        Ok((
            ExecutableNetworkPrepared {
                directory_revision: self.directory_revision,
                listeners,
                prepared_sessions,
                frozen_sessions,
                frozen_states,
            },
            ExecutableChildSocketResources {
                listeners: child_listeners,
                tcp_sessions,
            },
        ))
    }
}

impl ExecutableNetworkPrepared {
    #[must_use]
    pub const fn directory_revision(&self) -> u64 {
        self.directory_revision
    }

    #[must_use]
    pub fn listeners(&self) -> &[crate::ListenerBinding] {
        &self.listeners
    }
}

impl ExecutableFrozenNetwork {
    #[must_use]
    pub fn listeners(&self) -> &[crate::ListenerBinding] {
        &self.listeners
    }

    #[must_use]
    pub fn sessions(&self) -> &[ExecutableSessionTransportResource] {
        &self.sessions
    }

    #[must_use]
    pub fn session_states(&self) -> &[ExecutableSessionStateResource] {
        &self.session_states
    }

    #[must_use]
    pub const fn raknet_router_state(&self) -> Option<&SealedArenaRegion> {
        self.raknet_router_state.as_ref()
    }
}

impl ExecutableChildSocketResources {
    /// Constructs child-side native socket resources received through the authenticated transfer
    /// channel. Semantic validation happens when they are matched against the committed child.
    #[must_use]
    pub fn new(
        listeners: Vec<(TransportKind, ExportedSocket)>,
        tcp_sessions: Vec<(u64, ExportedSocket)>,
    ) -> Self {
        Self {
            listeners,
            tcp_sessions: tcp_sessions
                .into_iter()
                .map(|(connection_id, socket)| (ConnectionId(connection_id), socket))
                .collect(),
        }
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        Vec<(TransportKind, ExportedSocket)>,
        Vec<(u64, ExportedSocket)>,
    ) {
        (
            self.listeners,
            self.tcp_sessions
                .into_iter()
                .map(|(connection_id, socket)| (connection_id.0, socket))
                .collect(),
        )
    }
}

impl ExecutableSessionStateResource {
    #[must_use]
    pub const fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    #[must_use]
    pub const fn region(&self) -> &SealedArenaRegion {
        &self.region
    }
}

impl ExecutableSessionTransportResource {
    #[must_use]
    pub const fn connection_id(&self) -> ConnectionId {
        match self {
            Self::Tcp(transport) => transport.connection_id(),
            Self::Bedrock(transport) => transport.connection_id(),
        }
    }

    #[must_use]
    pub const fn transport(&self) -> TransportKind {
        match self {
            Self::Tcp(_) => TransportKind::Tcp,
            Self::Bedrock(_) => TransportKind::Udp,
        }
    }

    #[must_use]
    pub fn state(&self) -> ExecutableSessionState {
        match self {
            Self::Tcp(transport) => ExecutableSessionState::Tcp {
                actor: transport.actor.clone(),
                transport: transport.snapshot.clone(),
            },
            Self::Bedrock(transport) => ExecutableSessionState::Bedrock {
                actor: transport.actor.clone(),
                transport: transport.snapshot.clone(),
            },
        }
    }
}

impl ExecutableSessionState {
    #[must_use]
    pub const fn actor(&self) -> &ExecutableSessionActorSnapshot {
        match self {
            Self::Tcp { actor, .. } | Self::Bedrock { actor, .. } => actor,
        }
    }

    #[must_use]
    pub const fn transport(&self) -> TransportKind {
        match self {
            Self::Tcp { .. } => TransportKind::Tcp,
            Self::Bedrock { .. } => TransportKind::Udp,
        }
    }

    /// Encodes this checkpoint under the stable process-transfer representation.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when serialization fails or exceeds the per-session policy.
    pub fn encode(&self) -> Result<Vec<u8>, RuntimeError> {
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(self, &mut bytes)
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        ensure_session_blob_budget(
            "executable session state bytes",
            bytes.len(),
            MAX_EXECUTABLE_SESSION_STATE_BYTES,
        )?;
        Ok(bytes)
    }

    /// Decodes and validates the bounded process-transfer representation.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the payload exceeds policy, is malformed, or has trailing
    /// bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, RuntimeError> {
        ensure_session_blob_budget(
            "executable session state bytes",
            bytes.len(),
            MAX_EXECUTABLE_SESSION_STATE_BYTES,
        )?;
        let mut reader = Cursor::new(bytes);
        let state = ciborium::de::from_reader(&mut reader)
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        if usize::try_from(reader.position()).ok() != Some(bytes.len()) {
            return Err(RuntimeError::Config(
                "executable session state contains trailing bytes".to_string(),
            ));
        }
        Ok(state)
    }

    pub(crate) fn write_to_slot(
        &self,
        mut slot: ArenaReservation,
    ) -> Result<SealedArenaRegion, RuntimeError> {
        ciborium::ser::into_writer(self, &mut slot)
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        Ok(slot.seal_written())
    }
}

impl ExecutableTcpSessionTransport {
    #[must_use]
    pub const fn connection_id(&self) -> ConnectionId {
        self.actor.connection_id
    }

    #[must_use]
    pub const fn actor(&self) -> &ExecutableSessionActorSnapshot {
        &self.actor
    }

    #[must_use]
    pub const fn snapshot(&self) -> &ExecutableTcpTransportSnapshot {
        &self.snapshot
    }
}

impl ExecutableBedrockSessionTransport {
    #[must_use]
    pub const fn connection_id(&self) -> ConnectionId {
        self.actor.connection_id
    }

    #[must_use]
    pub const fn actor(&self) -> &ExecutableSessionActorSnapshot {
        &self.actor
    }

    #[must_use]
    pub const fn snapshot(&self) -> &ExecutableBedrockTransportSnapshot {
        &self.snapshot
    }
}

impl ExecutableSessionActorSnapshot {
    #[must_use]
    pub const fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    #[must_use]
    pub const fn epoch_revision(&self) -> u64 {
        self.epoch_revision
    }

    #[must_use]
    pub const fn generation_id(&self) -> GenerationId {
        self.generation_id
    }

    #[must_use]
    pub const fn phase(&self) -> &ExecutableSessionPhase {
        &self.phase
    }

    #[must_use]
    pub fn read_buffer(&self) -> &[u8] {
        &self.read_buffer
    }

    #[must_use]
    pub fn queued_messages(&self) -> &[ExecutableQueuedSessionMessage] {
        &self.queued_messages
    }

    #[must_use]
    pub fn pending_terminate(&self) -> Option<&str> {
        self.pending_terminate.as_deref()
    }

    #[must_use]
    pub fn protocol_session_blob(&self) -> &[u8] {
        &self.protocol_session_blob
    }

    #[must_use]
    pub fn gameplay_session_blob(&self) -> &[u8] {
        &self.gameplay_session_blob
    }
}

impl ExecutableTcpTransportSnapshot {
    #[must_use]
    pub const fn decrypt(&self) -> Option<ExecutableMinecraftCipherSnapshot> {
        self.decrypt
    }

    #[must_use]
    pub const fn encrypt(&self) -> Option<ExecutableMinecraftCipherSnapshot> {
        self.encrypt
    }
}

impl ExecutableMinecraftCipherSnapshot {
    #[must_use]
    pub const fn shared_secret(self) -> [u8; 16] {
        self.shared_secret
    }

    #[must_use]
    pub const fn shift_register(self) -> [u8; 16] {
        self.shift_register
    }
}

impl ExecutableBedrockTransportSnapshot {
    #[must_use]
    pub const fn peer(&self) -> &PeerSnapshot {
        &self.peer
    }

    #[must_use]
    pub const fn reader_compression_threshold(&self) -> Option<u16> {
        self.reader_compression_threshold
    }

    #[must_use]
    pub const fn writer_compression_threshold(&self) -> Option<u16> {
        self.writer_compression_threshold
    }
}

impl RuntimeServer {
    pub(super) async fn prepare_executable_upgrade(
        self: &Arc<Self>,
        arena: Arc<SharedTransferArena>,
    ) -> Result<ExecutableUpgradeStaged, RuntimeError> {
        let started_at = Instant::now();
        let serial_authority = self.reload.lock_reload_serial_owned().await;
        let status_id = self.authority.begin_executable_upgrade(
            RuntimeUpgradeRole::Parent,
            RuntimeUpgradePhase::ParentStaging,
        )?;
        let status_authority = ExecutableUpgradeStatusAuthority {
            runtime: Arc::clone(self),
            id: status_id,
            clear_on_drop: true,
        };
        let active = self.authority.active();
        let reload_host = self.reload.reload_host().ok_or_else(|| {
            RuntimeError::Config(
                "executable upgrade requires a reload-capable plugin host".to_string(),
            )
        })?;
        let plugin_artifacts = reload_host
            .exact_plugin_artifacts()?
            .into_iter()
            .map(|artifact| ExecutablePluginArtifact {
                plugin_id: artifact.plugin_id,
                sha256: artifact.sha256,
            })
            .collect();
        let validated_config_digest = validated_config_digest(&active.selection.config)?;
        let CoreProcessPrecopy {
            version: core_version,
            persisted_revision,
            latest_dirty_revision,
        } = active.core.process_precopy().await;
        let precopy_core = Arc::clone(&active.core);
        let staged = async {
            let core_snapshot =
                tokio::task::spawn_blocking(move || core_version.prepare_process_transfer())
                    .await
                    .map_err(|error| {
                        RuntimeError::Config(format!(
                            "core pre-copy worker failed during executable upgrade: {error}"
                        ))
                    })?
                    .map_err(|error| RuntimeError::Config(error.to_string()))?;
            let core_snapshot_revision = core_snapshot.revision();
            let mut core_snapshot_slot = arena
                .reserve(core_snapshot.bytes().len())
                .map_err(map_arena_error)?;
            core_snapshot_slot
                .write_all(core_snapshot.bytes())
                .map_err(RuntimeError::Io)?;
            let core_snapshot_region = core_snapshot_slot.seal().map_err(map_arena_error)?;
            let (directory_revision, sessions) =
                self.sessions.executable_directory_snapshot().await?;
            let initial_directory = write_directory_to_arena(
                &arena,
                ExecutableSessionDirectory {
                    revision: directory_revision,
                    sessions,
                },
            )?;
            let online_auth_keys_region = match active.online_auth_keys.as_ref() {
                Some(keys) => {
                    let bytes = keys.export_process_transfer()?;
                    let mut slot = arena.reserve(bytes.len()).map_err(map_arena_error)?;
                    slot.write_all(&bytes).map_err(RuntimeError::Io)?;
                    Some(slot.seal().map_err(map_arena_error)?)
                }
                None => None,
            };
            let final_core_delta_slot = arena
                .reserve(CORE_PROCESS_DELTA_SLOT_BYTES)
                .map_err(map_arena_error)?;
            let raknet_router_state_slot = if active.selection.config.topology.be_enabled {
                Some(
                    arena
                        .reserve(EXECUTABLE_RAKNET_ROUTER_STATE_BYTES)
                        .map_err(map_arena_error)?,
                )
            } else {
                None
            };

            Ok(ExecutableUpgradeStaged {
                runtime: Arc::clone(self),
                _serial_authority: serial_authority,
                status_authority,
                epoch_revision: active.revision(),
                active_generation_id: active.topology.generation_id,
                validated_config_digest,
                plugin_artifacts,
                arena,
                core_snapshot_revision,
                core_snapshot_region,
                initial_directory,
                online_auth_keys_region,
                final_core_delta_slot: tokio::sync::Mutex::new(Some(final_core_delta_slot)),
                raknet_router_state_slot: tokio::sync::Mutex::new(raknet_router_state_slot),
                persisted_core_revision: persisted_revision,
                latest_dirty_core_revision: latest_dirty_revision,
                stage_duration: started_at.elapsed(),
            })
        }
        .await;
        if staged.is_err() {
            precopy_core.end_precopy().await;
        }
        staged
    }
}

impl ServerSupervisor {
    /// Builds a child-side candidate without binding listeners or acquiring network authority.
    ///
    /// Config, packaged plugin identities, the immutable core pre-copy, and the directory
    /// projection are validated before the parent enters its data-plane freeze.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the V1 bootstrap is invalid, local config or plugin artifacts
    /// do not exactly match the parent, or an arena resource cannot be imported.
    pub fn prepare_executable_child(
        config_source: ServerConfigSource,
        bootstrap: &BootstrapV1,
        arena: &SharedTransferArenaReader,
    ) -> Result<ExecutableChildPrepared, RuntimeError> {
        bootstrap
            .validate()
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        if arena.generation()
            != bootstrap
                .arena
                .as_ref()
                .expect("validated bootstrap has an arena")
                .generation
        {
            return Err(RuntimeError::Config(
                "executable child mapped an arena from a different generation".to_string(),
            ));
        }
        let config = config_source.load()?;
        let local_digest = validated_config_digest(config.as_inner())?;
        if bootstrap.validated_config_digest.as_slice() != local_digest {
            return Err(RuntimeError::Config(
                "executable child validated config digest does not match the parent".to_string(),
            ));
        }
        let bootstrap_view = config.plugin_host_bootstrap_view();
        let bootstrap_config = mc_plugin_host::config::BootstrapConfig::from(&bootstrap_view);
        let plugin_host = mc_plugin_host::host::plugin_host_from_config(&bootstrap_config)?
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "no packaged plugins discovered under `{}`",
                    config.bootstrap.plugins_dir.display()
                ))
            })?;
        let selection_view = config.plugin_host_runtime_selection_view();
        let selection_config =
            mc_plugin_host::config::RuntimeSelectionConfig::from(&selection_view);
        let loaded_plugins = plugin_host.load_plugin_set(&selection_config)?;
        let local_artifacts = plugin_host
            .exact_plugin_artifacts()?
            .into_iter()
            .map(|artifact| ExecutablePluginArtifact {
                plugin_id: artifact.plugin_id,
                sha256: artifact.sha256,
            })
            .collect::<Vec<_>>();
        let expected_artifacts = bootstrap
            .plugin_artifacts
            .iter()
            .map(|artifact| {
                let sha256 = artifact.sha256.as_slice().try_into().map_err(|_| {
                    RuntimeError::Config(format!(
                        "plugin artifact `{}` has an invalid digest length",
                        artifact.plugin_id
                    ))
                })?;
                Ok(ExecutablePluginArtifact {
                    plugin_id: artifact.plugin_id.clone(),
                    sha256,
                })
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?;
        if local_artifacts != expected_artifacts {
            return Err(RuntimeError::Config(
                "executable child plugin artifact identities do not exactly match the parent"
                    .to_string(),
            ));
        }
        let active_protocols =
            super::bootstrap::activate_protocols(config.as_inner(), loaded_plugins.protocols())?;
        let selection = super::selection::SelectionResolver::resolve(
            config.as_inner().clone(),
            loaded_plugins,
            &[],
        )?;

        let core_snapshot = required_resource(
            &bootstrap.resources,
            ResourceKindV1::CoreSnapshot,
            "core snapshot",
        )?;
        let core_bytes = resource_bytes(arena, core_snapshot)?;
        let snapshot_core = CoreVersion::import_process_transfer(
            core_bytes,
            super::selection::SelectionResolver::content_behavior(),
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
        if snapshot_core.revision().value() != bootstrap.core_snapshot_revision {
            return Err(RuntimeError::Config(format!(
                "core snapshot revision {} does not match bootstrap revision {}",
                snapshot_core.revision().value(),
                bootstrap.core_snapshot_revision
            )));
        }
        let directory_resource = required_resource(
            &bootstrap.resources,
            ResourceKindV1::SessionDirectory,
            "session directory",
        )?;
        let directory =
            ExecutableSessionDirectory::decode(resource_bytes(arena, directory_resource)?)?;
        if directory.revision != bootstrap.directory_revision {
            return Err(RuntimeError::Config(format!(
                "session directory revision {} does not match bootstrap revision {}",
                directory.revision, bootstrap.directory_revision
            )));
        }
        let online_auth_keys = match optional_resource(
            &bootstrap.resources,
            ResourceKindV1::OnlineAuthKeys,
            "online authentication keys",
        )? {
            Some(resource) if config.as_inner().bootstrap.online_mode => Some(Arc::new(
                super::OnlineAuthKeys::import_process_transfer(resource_bytes(arena, resource)?)?,
            )),
            Some(_) => {
                return Err(RuntimeError::Config(
                    "offline child received online authentication keys".to_string(),
                ));
            }
            None if config.as_inner().bootstrap.online_mode => {
                return Err(RuntimeError::Config(
                    "online child is missing transferred authentication keys".to_string(),
                ));
            }
            None => None,
        };

        let mut child = ExecutableChildPrepared {
            config_source,
            config,
            plugin_host,
            active_protocols,
            selection,
            online_auth_keys,
            pending_core: snapshot_core,
            persisted_revision: CoreRevision::from_value(bootstrap.persisted_core_revision),
            latest_dirty_revision: bootstrap
                .latest_dirty_core_revision
                .map(CoreRevision::from_value),
            directory,
            prepared_play_phases: BTreeMap::new(),
            raknet_router_peers: Vec::new(),
            active_generation_id: GenerationId(bootstrap.active_generation_id),
        };
        child.prepared_play_phases = prepare_directory_play_phases(&child, &child.directory)?;
        Ok(child)
    }
}

impl ExecutableChildPrepared {
    #[must_use]
    pub fn core_revision(&self) -> CoreRevision {
        self.pending_core.revision()
    }

    #[must_use]
    pub const fn directory_revision(&self) -> u64 {
        self.directory.revision
    }

    /// Imports duplicated native sockets while the parent still owns live network authority.
    /// The returned sockets remain inert until final session state is bound and the child runtime
    /// is activated.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when native resources do not exactly match the pre-staged topology
    /// and session directory, or when the operating system rejects an imported descriptor.
    pub fn prepare_native_socket_import(
        &self,
        resources: ExecutableChildSocketResources,
    ) -> Result<ExecutableChildNativeSockets, RuntimeError> {
        let mut listeners = HashMap::new();
        for (transport, socket) in resources.listeners {
            if listeners.insert(transport, socket).is_some() {
                return Err(RuntimeError::Config(
                    "child received multiple listener sockets for one transport".to_string(),
                ));
            }
        }
        let tcp_socket = listeners.remove(&TransportKind::Tcp).ok_or_else(|| {
            RuntimeError::Config("child transfer is missing the TCP listener socket".to_string())
        })?;
        let tcp_listener = import_tcp_listener(tcp_socket)?;

        let udp_socket = match listeners.remove(&TransportKind::Udp) {
            Some(socket) => Some(Arc::new(import_udp_socket(socket)?)),
            None if self.config.as_inner().topology.be_enabled => {
                return Err(RuntimeError::Config(
                    "child transfer is missing the Bedrock listener socket".to_string(),
                ));
            }
            None => None,
        };
        debug_assert!(listeners.is_empty());

        let expected_tcp_sessions = self
            .directory
            .sessions
            .iter()
            .filter(|session| session.transport == TransportKind::Tcp)
            .map(|session| session.connection_id)
            .collect::<std::collections::BTreeSet<_>>();
        let mut tcp_sessions = BTreeMap::new();
        for (connection_id, socket) in resources.tcp_sessions {
            if !expected_tcp_sessions.contains(&connection_id) {
                return Err(RuntimeError::Config(format!(
                    "child received TCP socket for unknown session {connection_id:?}"
                )));
            }
            let stream = import_tcp_stream(socket)?;
            let io = TransportSessionIo::prestaged_imported_tcp(stream);
            if tcp_sessions.insert(connection_id, io).is_some() {
                return Err(RuntimeError::Config(format!(
                    "child transfer duplicates TCP session socket {connection_id:?}"
                )));
            }
        }
        if tcp_sessions.len() != expected_tcp_sessions.len() {
            return Err(RuntimeError::Config(format!(
                "child imported {} TCP session sockets but directory requires {}",
                tcp_sessions.len(),
                expected_tcp_sessions.len()
            )));
        }

        Ok(ExecutableChildNativeSockets {
            tcp_listener,
            udp_socket,
            tcp_sessions,
        })
    }

    /// Validates an immutable child candidate update without acquiring network authority.
    ///
    /// Repeated directory projections and a final core journal can be supplied. Each call is
    /// atomic from the child's point of view: malformed resources leave the previous candidate
    /// unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a resource is ambiguous, out of arena bounds, malformed, or
    /// does not reach the revisions declared by `prestage`.
    pub fn prestage(
        &mut self,
        prestage: &PrestageV1,
        arena: &mut SharedTransferArenaReader,
    ) -> Result<(), RuntimeError> {
        prestage
            .validate()
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        arena
            .advance_published(prestage.arena_used)
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        let snapshot = optional_resource(
            &prestage.resources,
            ResourceKindV1::CoreSnapshot,
            "core snapshot",
        )?;
        let journal = optional_resource(
            &prestage.resources,
            ResourceKindV1::CoreJournal,
            "core journal",
        )?;
        let next_core = match (snapshot, journal) {
            (Some(_), Some(_)) => {
                return Err(RuntimeError::Config(
                    "core pre-stage cannot contain both a snapshot and a journal".to_string(),
                ));
            }
            (Some(resource), None) => CoreVersion::import_process_transfer(
                resource_bytes(arena, resource)?,
                super::selection::SelectionResolver::content_behavior(),
            )
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
            (None, Some(resource)) => self
                .pending_core
                .apply_process_transfer_delta(resource_bytes(arena, resource)?)
                .map_err(|error| RuntimeError::Config(error.to_string()))?,
            (None, None) => Arc::clone(&self.pending_core),
        };
        if next_core.revision() < self.pending_core.revision() {
            return Err(RuntimeError::Config(
                "core pre-stage revision moved backwards".to_string(),
            ));
        }
        if next_core.revision().value() != prestage.core_revision {
            return Err(RuntimeError::Config(format!(
                "pre-staged core revision {} does not match declared revision {}",
                next_core.revision().value(),
                prestage.core_revision
            )));
        }
        let next_directory = match optional_resource(
            &prestage.resources,
            ResourceKindV1::SessionDirectory,
            "session directory",
        )? {
            Some(resource) => Some(ExecutableSessionDirectory::decode(resource_bytes(
                arena, resource,
            )?)?),
            None => None,
        };
        let effective_directory = next_directory.as_ref().unwrap_or(&self.directory);
        if effective_directory.revision != prestage.directory_revision {
            return Err(RuntimeError::Config(format!(
                "pre-staged directory revision {} does not match declared revision {}",
                effective_directory.revision, prestage.directory_revision
            )));
        }
        let next_raknet_router_peers = match optional_resource(
            &prestage.resources,
            ResourceKindV1::RakNetState,
            "RakNet router state",
        )? {
            Some(resource) => Some(decode_raknet_router_state(resource_bytes(
                arena, resource,
            )?)?),
            None => None,
        };
        let directory_update = match next_directory {
            Some(next_directory) if next_directory != self.directory => {
                if next_directory.revision == self.directory.revision {
                    return Err(RuntimeError::Config(format!(
                        "pre-staged directory revision {} was reused for different session state",
                        next_directory.revision
                    )));
                }
                let next_play_phases = prepare_directory_play_phases(self, &next_directory)?;
                Some((next_directory, next_play_phases))
            }
            Some(_) | None => None,
        };
        self.pending_core = next_core;
        if let Some((next_directory, next_play_phases)) = directory_update {
            self.directory = next_directory;
            self.prepared_play_phases = next_play_phases;
        }
        if let Some(peers) = next_raknet_router_peers {
            self.raknet_router_peers = peers;
        }
        Ok(())
    }

    /// Resolves every session binding and imports plugin-owned handoff state before commit.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when session resources are malformed, do not correspond exactly
    /// to the acknowledged directory, or refer to unavailable plugin generations. Failure
    /// consumes the child candidate so partially imported plugin state cannot be retried.
    pub fn prepare_session_import(
        mut self,
        resources: &[ResourceDescriptorV1],
        arena: &SharedTransferArenaReader,
    ) -> Result<ExecutableChildSessionsPrepared, RuntimeError> {
        let mut encoded_states = Vec::new();
        for resource in resources {
            let kind = ResourceKindV1::try_from(resource.kind).map_err(|_| {
                RuntimeError::Config(format!(
                    "executable transfer resource {} has an invalid kind tag {}",
                    resource.resource_id, resource.kind
                ))
            })?;
            if kind == ResourceKindV1::SessionState {
                let connection_id = resource.logical_id.ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "session state resource {} has no connection identity",
                        resource.resource_id
                    ))
                })?;
                encoded_states.push((
                    ConnectionId(connection_id),
                    resource_bytes(arena, resource)?,
                ));
            }
        }
        let sessions = prepare_executable_sessions(&mut self, encoded_states)?;
        Ok(ExecutableChildSessionsPrepared {
            prepared: self,
            sessions,
        })
    }
}

impl ExecutableChildSessionsPrepared {
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Imports duplicated listener/session sockets and reconstructs frozen transport authorities.
    /// No socket is read, accepted, or written before the returned capability is activated.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when native resources do not exactly match the committed topology
    /// and session directory, or when a transferred RakNet checkpoint is invalid.
    pub async fn prepare_network_import(
        mut self,
        native: ExecutableChildNativeSockets,
    ) -> Result<ExecutableChildNetworkPrepared, RuntimeError> {
        let ExecutableChildNativeSockets {
            tcp_listener,
            udp_socket,
            mut tcp_sessions,
        } = native;
        let bedrock_listener = match udp_socket {
            Some(socket) => {
                let binding = crate::ListenerBinding {
                    transport: TransportKind::Udp,
                    local_addr: socket.local_addr()?,
                    adapter_ids: self
                        .prepared
                        .active_protocols
                        .protocols
                        .adapter_ids_for_transport(TransportKind::Udp),
                };
                let server = RakNetServer::<RakNetBound>::from_socket(
                    imported_raknet_server_config(&self.prepared, &binding)?,
                    socket,
                )?;
                Some(server.start_paused())
            }
            None => None,
        };

        enum NetworkCandidate {
            Tcp {
                actor: ExecutableSessionActorSnapshot,
                transport: ExecutableTcpTransportSnapshot,
                phase: SessionPhase,
            },
            Bedrock {
                actor: ExecutableSessionActorSnapshot,
                remote_addr: std::net::SocketAddr,
                reader_compression_threshold: Option<u16>,
                writer_compression_threshold: Option<u16>,
                phase: SessionPhase,
            },
        }

        let mut peer_snapshots = std::mem::take(&mut self.prepared.raknet_router_peers);
        let candidates = std::mem::take(&mut self.sessions)
            .into_iter()
            .map(|candidate| {
                let PreparedExecutableSession { state, phase } = candidate;
                match state {
                    ValidatedExecutableSessionState::Tcp { actor, transport } => {
                        NetworkCandidate::Tcp {
                            actor,
                            transport,
                            phase,
                        }
                    }
                    ValidatedExecutableSessionState::Bedrock {
                        actor,
                        peer,
                        reader_compression_threshold,
                        writer_compression_threshold,
                    } => {
                        let remote_addr = peer.remote_addr();
                        peer_snapshots.push(peer);
                        NetworkCandidate::Bedrock {
                            actor,
                            remote_addr,
                            reader_compression_threshold,
                            writer_compression_threshold,
                            phase,
                        }
                    }
                }
            })
            .collect::<Vec<_>>();
        let mut bedrock_listener = bedrock_listener;
        let mut imported_peers = if let Some(listener) = bedrock_listener.as_mut() {
            listener.import_peers(peer_snapshots).await?
        } else if peer_snapshots.is_empty() {
            Vec::new()
        } else {
            return Err(RuntimeError::Config(
                "RakNet peer state was transferred without a Bedrock listener".to_string(),
            ));
        };
        let mut peers_by_addr = imported_peers
            .drain(..)
            .map(|peer| (peer.remote_addr(), peer))
            .collect::<HashMap<_, _>>();

        let mut sessions = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let (actor, phase, io) = match candidate {
                NetworkCandidate::Tcp {
                    actor,
                    transport,
                    phase,
                } => {
                    let mut io = tcp_sessions.remove(&actor.connection_id).ok_or_else(|| {
                        RuntimeError::Config(format!(
                            "child transfer is missing TCP socket for session {:?}",
                            actor.connection_id
                        ))
                    })?;
                    let decrypt = transport.decrypt.map(imported_cipher_snapshot);
                    let encrypt = transport.encrypt.map(imported_cipher_snapshot);
                    io.restore_imported_tcp_state(decrypt, encrypt)?;
                    (actor, phase, io)
                }
                NetworkCandidate::Bedrock {
                    actor,
                    remote_addr,
                    reader_compression_threshold,
                    writer_compression_threshold,
                    phase,
                } => {
                    let peer = peers_by_addr.remove(&remote_addr).ok_or_else(|| {
                        RuntimeError::Config(format!(
                            "child RakNet listener did not import peer {remote_addr} for session {:?}",
                            actor.connection_id
                        ))
                    })?;
                    (
                        actor,
                        phase,
                        TransportSessionIo::imported_bedrock(
                            peer,
                            reader_compression_threshold,
                            writer_compression_threshold,
                        ),
                    )
                }
            };
            sessions.push(PreparedImportedSession { actor, phase, io });
        }
        if let Some((connection_id, _)) = tcp_sessions.into_iter().next() {
            return Err(RuntimeError::Config(format!(
                "child received an unclaimed TCP socket for session {connection_id:?}"
            )));
        }
        let queued_bedrock_peers = peers_by_addr.into_values().collect();
        Ok(ExecutableChildNetworkPrepared {
            prepared: self.prepared,
            tcp_listener,
            bedrock_listener,
            queued_bedrock_peers,
            sessions,
        })
    }
}

impl ExecutableChildCommit {
    #[must_use]
    pub const fn config_source(&self) -> &ServerConfigSource {
        &self.config_source
    }

    #[must_use]
    pub const fn config(&self) -> &crate::config::ServerConfig {
        self.config.as_inner()
    }

    /// Returns the exact loaded artifact identities whose generation leases are retained by this
    /// unactivated child candidate.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if the packaged plugin catalog can no longer prove an exact
    /// artifact identity.
    pub fn plugin_artifacts(&self) -> Result<Vec<ExecutablePluginArtifact>, RuntimeError> {
        self.plugin_host
            .exact_plugin_artifacts()?
            .into_iter()
            .map(|artifact| {
                Ok(ExecutablePluginArtifact {
                    plugin_id: artifact.plugin_id,
                    sha256: artifact.sha256,
                })
            })
            .collect()
    }

    #[must_use]
    pub fn loaded_protocol_adapter_ids(&self) -> Vec<revy_voxel_core::AdapterId> {
        let protocols = self.selection.loaded_plugins.protocols();
        let mut ids = protocols.adapter_ids_for_transport(TransportKind::Tcp);
        ids.extend(protocols.adapter_ids_for_transport(TransportKind::Udp));
        ids.sort();
        ids.dedup();
        ids
    }

    #[must_use]
    pub fn active_protocol_adapter_ids(&self) -> Vec<revy_voxel_core::AdapterId> {
        let mut ids = self
            .active_protocols
            .protocols
            .adapter_ids_for_transport(TransportKind::Tcp);
        ids.extend(
            self.active_protocols
                .protocols
                .adapter_ids_for_transport(TransportKind::Udp),
        );
        ids.sort();
        ids.dedup();
        ids
    }

    #[must_use]
    pub fn sessions(&self) -> Vec<SessionStatusSnapshot> {
        self.sessions
            .iter()
            .map(|session| session_status_snapshot(session.actor.connection_id, &session.phase))
            .collect()
    }

    #[must_use]
    pub fn core(&self) -> &Arc<CoreVersion> {
        &self.core
    }

    #[must_use]
    pub const fn persisted_revision(&self) -> CoreRevision {
        self.persisted_revision
    }

    #[must_use]
    pub const fn latest_dirty_revision(&self) -> Option<CoreRevision> {
        self.latest_dirty_revision
    }

    #[must_use]
    pub const fn directory(&self) -> &ExecutableSessionDirectory {
        &self.directory
    }

    #[must_use]
    pub const fn parent_epoch_revision(&self) -> u64 {
        self.parent_epoch_revision
    }

    pub(in crate::runtime) fn into_boot_parts(self) -> ExecutableChildBootParts {
        ExecutableChildBootParts {
            config_source: self.config_source,
            config: self.config,
            plugin_host: self.plugin_host,
            active_protocols: self.active_protocols,
            selection: self.selection,
            online_auth_keys: self.online_auth_keys,
            core: self.core,
            persisted_revision: self.persisted_revision,
            latest_dirty_revision: self.latest_dirty_revision,
            active_generation_id: self.active_generation_id,
            parent_epoch_revision: self.parent_epoch_revision,
            cutover_context: self.cutover_context,
            tcp_listener: self.tcp_listener,
            bedrock_listener: self.bedrock_listener,
            queued_bedrock_peers: self.queued_bedrock_peers,
            sessions: self.sessions,
        }
    }
}

impl ExecutableChildRuntimePrepared {
    pub(in crate::runtime) fn new(
        running: super::RunningServer,
        data_plane: FrozenDataPlane,
        activation_receivers: Vec<tokio::sync::oneshot::Receiver<Result<(), String>>>,
        queued_bedrock_peers: Vec<RakNetPeer<FrozenRakNetPeer>>,
        generation_id: GenerationId,
        cutover_context: ExecutableChildCutoverContext,
    ) -> Self {
        Self {
            running,
            data_plane: Some(data_plane),
            activation_receivers,
            queued_bedrock_peers,
            generation_id,
            cutover_context,
        }
    }

    /// Activates the imported listeners and sessions exactly once. After this transition the
    /// child is the sole data-plane authority and may acknowledge `Committed` to the parent.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] after terminating the child runtime when any imported authority
    /// cannot be activated. The parent must remain fail-closed after sending `Commit`.
    pub async fn activate(mut self) -> Result<ExecutableChildActivated, RuntimeError> {
        let resume_started = Instant::now();
        // Commit authorizes activation, but gameplay admission must stay closed until every
        // imported actor has installed its prepared state. Opening it first lets the first tick
        // and queued gameplay compete with the remaining activation acknowledgements.
        let active_revision = self.running.runtime.authority.active().revision();
        self.running
            .runtime
            .authority
            .activate_epoch(active_revision);
        for activation in std::mem::take(&mut self.activation_receivers) {
            match activation.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    return Err(self
                        .fail_closed(RuntimeError::Config(format!(
                            "imported session activation failed: {error}"
                        )))
                        .await);
                }
                Err(_) => {
                    return Err(self
                        .fail_closed(RuntimeError::Config(
                            "imported session ended before activation acknowledgement".to_string(),
                        ))
                        .await);
                }
            }
        }
        let mut resumed_peers = Vec::with_capacity(self.queued_bedrock_peers.len());
        for peer in std::mem::take(&mut self.queued_bedrock_peers) {
            match peer.resume() {
                Ok(peer) => resumed_peers.push(peer),
                Err(error) => return Err(self.fail_closed(RuntimeError::from(error)).await),
            }
        }
        if let Err(error) = self
            .running
            .runtime
            .topology_resources
            .activate_imported_listeners()
            .await
        {
            return Err(self.fail_closed(error).await);
        }
        self.data_plane
            .take()
            .expect("prepared child must retain frozen data-plane authority")
            .resume();

        for peer in resumed_peers {
            let accepted = super::AcceptedGenerationSession::new(
                self.generation_id,
                crate::transport::AcceptedTransportSession {
                    transport: TransportKind::Udp,
                    io: TransportSessionIo::bedrock(peer),
                },
                self.running
                    .runtime
                    .sessions
                    .queued_accepts()
                    .track(self.generation_id),
            );
            if self
                .running
                .runtime
                .sessions
                .accepted_sender()
                .send(accepted)
                .await
                .is_err()
            {
                return Err(self
                    .fail_closed(RuntimeError::Config(
                        "child runtime closed before queued Bedrock peers were admitted"
                            .to_string(),
                    ))
                    .await);
            }
        }

        Ok(ExecutableChildActivated {
            server: Arc::new(ServerSupervisor {
                running: self.running,
            }),
            cutover_context: self.cutover_context,
            resume_duration: resume_started.elapsed(),
        })
    }

    async fn fail_closed(mut self, cause: RuntimeError) -> RuntimeError {
        let _ = self.running.runtime.request_shutdown();
        if let Some(data_plane) = self.data_plane.take() {
            data_plane.resume();
        }
        match self.running.join_runtime().await {
            Ok(()) => cause,
            Err(shutdown) => RuntimeError::Config(format!(
                "{cause}; child fail-closed shutdown failed: {shutdown}"
            )),
        }
    }
}

impl ExecutableChildActivated {
    #[must_use]
    pub fn server(&self) -> Arc<ServerSupervisor> {
        Arc::clone(&self.server)
    }

    #[must_use]
    pub fn resume_duration_us(&self) -> u64 {
        duration_us(self.resume_duration)
    }

    /// Publishes the committed executable cutover into the child operator status.
    ///
    /// The context was validated with the child commit, so callers can provide only the measured
    /// freeze duration and cannot redefine the transferred epoch or connection population.
    #[must_use]
    pub fn publish_committed_report(&self, freeze_duration: Duration) -> CutoverReport {
        let report = CutoverReport {
            operation: CutoverOperation::ExecutableUpgrade,
            mode: None,
            connection_mix: self.cutover_context.connection_mix,
            session_count: self.cutover_context.session_count,
            stage_us: self.cutover_context.stage_us,
            prepare_us: self.cutover_context.prepare_us,
            freeze_us: duration_us(freeze_duration),
            resume_us: duration_us(self.resume_duration),
            outcome: CutoverOutcome::Committed,
            epoch_revision: self.cutover_context.child_epoch_revision,
        };
        self.server
            .running
            .runtime
            .authority
            .record_cutover(report.clone());
        report
    }
}

impl ExecutableChildNetworkPrepared {
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    #[must_use]
    pub fn queued_bedrock_peer_count(&self) -> usize {
        self.queued_bedrock_peers.len()
    }

    #[must_use]
    pub fn listener_bindings(&self) -> Vec<crate::ListenerBinding> {
        let mut bindings = vec![crate::ListenerBinding {
            transport: TransportKind::Tcp,
            local_addr: self
                .tcp_listener
                .local_addr()
                .expect("validated imported TCP listener retains its address"),
            adapter_ids: self
                .prepared
                .active_protocols
                .protocols
                .adapter_ids_for_transport(TransportKind::Tcp),
        }];
        if let Some(listener) = &self.bedrock_listener {
            bindings.push(crate::ListenerBinding {
                transport: TransportKind::Udp,
                local_addr: listener
                    .local_addr()
                    .expect("validated imported UDP listener retains its address"),
                adapter_ids: self
                    .prepared
                    .active_protocols
                    .protocols
                    .adapter_ids_for_transport(TransportKind::Udp),
            });
        }
        bindings
    }

    /// Consumes the fully imported child candidate only when final revisions match every pending
    /// session. Native sockets are pre-staged before the parent freezes; final RakNet state is
    /// reconstructed during frozen preparation. Both complete before this transition is allowed.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when commit metadata differs from the validated pending state.
    pub fn commit(mut self, commit: &CommitV1) -> Result<ExecutableChildCommit, RuntimeError> {
        commit
            .validate()
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        if self.prepared.pending_core.revision().value() != commit.final_core_revision {
            return Err(RuntimeError::Config(format!(
                "child pending core revision {} does not match commit revision {}",
                self.prepared.pending_core.revision().value(),
                commit.final_core_revision
            )));
        }
        if self.prepared.directory.revision != commit.final_directory_revision {
            return Err(RuntimeError::Config(format!(
                "child pending directory revision {} does not match commit revision {}",
                self.prepared.directory.revision, commit.final_directory_revision
            )));
        }
        if let Some(session) = self
            .sessions
            .iter()
            .find(|session| session.actor.epoch_revision != commit.parent_epoch_revision)
        {
            return Err(RuntimeError::Config(format!(
                "session {:?} belongs to parent epoch {} but commit names epoch {}",
                session.actor.connection_id,
                session.actor.epoch_revision,
                commit.parent_epoch_revision
            )));
        }
        let connection_mix = connection_mix(&self.prepared.directory);
        let session_count = u64::try_from(self.prepared.directory.sessions.len())
            .map_err(|_| RuntimeError::Config("child session count exceeds u64".to_string()))?;
        if session_count != commit.session_count
            || u64::try_from(connection_mix.java).ok() != Some(commit.java_session_count)
            || u64::try_from(connection_mix.bedrock).ok() != Some(commit.bedrock_session_count)
        {
            return Err(RuntimeError::Config(format!(
                "child session mix java={} bedrock={} total={} does not match commit java={} bedrock={} total={}",
                connection_mix.java,
                connection_mix.bedrock,
                session_count,
                commit.java_session_count,
                commit.bedrock_session_count,
                commit.session_count
            )));
        }
        let child_epoch_revision = commit
            .parent_epoch_revision
            .checked_add(1)
            .ok_or_else(|| RuntimeError::Config("child epoch revision overflow".to_string()))?;
        self.prepared.persisted_revision = CoreRevision::from_value(commit.persisted_core_revision);
        self.prepared.latest_dirty_revision = commit
            .latest_dirty_core_revision
            .map(CoreRevision::from_value);
        Ok(ExecutableChildCommit {
            config_source: self.prepared.config_source,
            config: self.prepared.config,
            plugin_host: self.prepared.plugin_host,
            active_protocols: self.prepared.active_protocols,
            selection: self.prepared.selection,
            online_auth_keys: self.prepared.online_auth_keys,
            core: self.prepared.pending_core,
            persisted_revision: self.prepared.persisted_revision,
            latest_dirty_revision: self.prepared.latest_dirty_revision,
            directory: self.prepared.directory,
            active_generation_id: self.prepared.active_generation_id,
            parent_epoch_revision: commit.parent_epoch_revision,
            cutover_context: ExecutableChildCutoverContext {
                stage_us: commit.stage_duration_us,
                prepare_us: commit.prepare_duration_us,
                connection_mix,
                session_count: usize::try_from(session_count).map_err(|_| {
                    RuntimeError::Config("child session count exceeds address space".to_string())
                })?,
                child_epoch_revision,
            },
            tcp_listener: self.tcp_listener,
            bedrock_listener: self.bedrock_listener,
            queued_bedrock_peers: self.queued_bedrock_peers,
            sessions: self.sessions,
        })
    }
}

impl ExecutableUpgradeStaged {
    /// Enters the child boot and continuous pre-stage phase.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if this upgrade authority was superseded.
    pub async fn begin_preparing(self) -> Result<ExecutableUpgradePreparing, RuntimeError> {
        if let Err(error) = self
            .status_authority
            .update(RuntimeUpgradePhase::ParentPreparing)
        {
            self.runtime.authority.active().core.end_precopy().await;
            return Err(error);
        }
        Ok(ExecutableUpgradePreparing {
            staged: self,
            started_at: Instant::now(),
        })
    }

    fn mark_frozen(&self) -> Result<(), RuntimeError> {
        self.status_authority
            .update(RuntimeUpgradePhase::ParentFrozen)
    }

    fn mark_outcome_uncertain(&self) -> Result<(), RuntimeError> {
        self.status_authority
            .update(RuntimeUpgradePhase::ParentOutcomeUncertain)
    }

    const fn epoch_revision(&self) -> u64 {
        self.epoch_revision
    }

    const fn active_generation_id(&self) -> GenerationId {
        self.active_generation_id
    }

    const fn validated_config_digest(&self) -> [u8; 32] {
        self.validated_config_digest
    }

    fn plugin_artifacts(&self) -> &[ExecutablePluginArtifact] {
        &self.plugin_artifacts
    }

    const fn core_snapshot_revision(&self) -> CoreRevision {
        self.core_snapshot_revision
    }

    const fn core_snapshot_region(&self) -> &SealedArenaRegion {
        &self.core_snapshot_region
    }

    const fn persisted_core_revision(&self) -> CoreRevision {
        self.persisted_core_revision
    }

    const fn latest_dirty_core_revision(&self) -> Option<CoreRevision> {
        self.latest_dirty_core_revision
    }

    const fn staged_directory_revision(&self) -> u64 {
        self.initial_directory.directory.revision
    }

    const fn initial_directory(&self) -> &ExecutableDirectoryPrestage {
        &self.initial_directory
    }

    const fn online_auth_keys_region(&self) -> Option<&SealedArenaRegion> {
        self.online_auth_keys_region.as_ref()
    }

    fn current_directory_revision(&self) -> u64 {
        self.runtime.sessions.directory_revision()
    }

    fn arena(&self) -> &Arc<SharedTransferArena> {
        &self.arena
    }

    /// Captures the latest stable session-directory projection into a new immutable arena region.
    ///
    /// This operation remains outside the data-plane freeze and may be repeated until the child
    /// acknowledges the current directory revision.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a stable projection cannot be observed, serialization exceeds
    /// policy, or the arena budget is exhausted.
    async fn prestage_directory(&self) -> Result<ExecutableDirectoryPrestage, RuntimeError> {
        let (revision, sessions) = self
            .runtime
            .sessions
            .executable_directory_snapshot()
            .await?;
        write_directory_to_arena(
            &self.arena,
            ExecutableSessionDirectory { revision, sessions },
        )
    }

    /// Freezes every live session transport while retaining rollback authority in its actor.
    ///
    /// The executable coordinator must call this only after the global data-plane gate and ingress
    /// are closed. A failure can be recovered with [`Self::rollback_session_transports`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if any actor cannot seal its TCP or RakNet transport state.
    async fn freeze_session_transports(
        &self,
        prepared_directory_revision: u64,
    ) -> Result<FrozenProcessSessionStates, RuntimeError> {
        self.runtime
            .sessions
            .freeze_for_process_transfer(prepared_directory_revision)
            .await
    }

    /// Restores all session transport authorities retained after a pre-commit failure.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if a retained transport cannot resume. Such a failure is isolated
    /// to that session by the actor.
    async fn rollback_session_transports(&self) -> Result<(), RuntimeError> {
        self.runtime.sessions.rollback_process_transfer().await
    }

    /// Irreversibly consumes the parent session authorities after child commit is confirmed.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if a session actor cannot finalize its transferred authority.
    async fn commit_session_transports(&self) -> Result<(), RuntimeError> {
        self.runtime.sessions.commit_process_transfer().await
    }

    async fn seal_raknet_router_state(
        &self,
        peers: &[PeerSnapshot],
    ) -> Result<Option<SealedArenaRegion>, RuntimeError> {
        let Some(mut slot) = self.raknet_router_state_slot.lock().await.take() else {
            if peers.is_empty() {
                return Ok(None);
            }
            return Err(RuntimeError::Config(
                "RakNet router state exists without a prepared transfer slot".to_string(),
            ));
        };
        ciborium::ser::into_writer(peers, &mut slot)
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        Ok(Some(slot.seal_written()))
    }

    /// Materializes only the bounded mutations committed after the pre-copy revision.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if another runtime epoch superseded this stage or the bounded
    /// semantic journal cannot be encoded.
    async fn seal_final_core_delta(
        &self,
        base_revision: CoreRevision,
    ) -> Result<ExecutableCoreDelta, RuntimeError> {
        let active = self.runtime.authority.active();
        if active.revision() != self.epoch_revision {
            return Err(RuntimeError::Config(format!(
                "executable upgrade staged epoch {} but active epoch is now {}",
                self.epoch_revision,
                active.revision()
            )));
        }
        let mut slot = self
            .final_core_delta_slot
            .lock()
            .await
            .take()
            .ok_or_else(|| {
                RuntimeError::Config(
                    "executable final core delta was already sealed or invalidated".to_string(),
                )
            })?;
        match active
            .core
            .write_process_delta_since(base_revision, &mut slot)
            .await
            .map_err(RuntimeError::CoreTransfer)?
        {
            CoreProcessDeltaSeal::Ready {
                descriptor,
                persisted_revision,
                latest_dirty_revision,
            } => Ok(ExecutableCoreDelta::Ready {
                descriptor,
                region: slot.seal_written(),
                persisted_revision,
                latest_dirty_revision,
            }),
            CoreProcessDeltaSeal::Outpaced {
                requested_revision,
                earliest_revision,
            } => {
                *self.final_core_delta_slot.lock().await = Some(slot);
                Ok(ExecutableCoreDelta::Outpaced {
                    requested_revision,
                    earliest_revision,
                })
            }
        }
    }
}

impl ExecutableUpgradePreparing {
    /// Copies an incremental core update without freezing gameplay. If the finite journal no
    /// longer covers the child's revision, starts a fresh snapshot and journal before freeze.
    /// Arena exhaustion remains a preparation failure; it cannot trigger a large frozen copy.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] for an invalid base revision, encoding/allocation failure, or an
    /// exhausted transfer arena. The caller must abort preparation when the child rejects it.
    pub async fn prestage_core(
        &self,
        base_revision: u64,
    ) -> Result<ExecutableCorePrestage, RuntimeError> {
        let core = &self.staged.runtime.authority.active().core;
        // This is outside freeze. Materialize the already bounded journal into temporary
        // owned storage so the monotonic arena retains only published bytes, not a maximum-size
        // reservation (including an unused reservation when a fresh snapshot is required).
        let mut encoded_delta = Vec::new();
        encoded_delta
            .try_reserve_exact(CORE_PROCESS_DELTA_SLOT_BYTES)
            .map_err(|_error| RuntimeError::Allocation {
                resource: "core pre-stage delta",
                requested: CORE_PROCESS_DELTA_SLOT_BYTES,
            })?;
        let delta = core
            .write_process_delta_since(CoreRevision::from_value(base_revision), &mut encoded_delta)
            .await
            .map_err(RuntimeError::CoreTransfer)?;
        let (revision, kind, region) = match delta {
            CoreProcessDeltaSeal::Ready { descriptor, .. } => {
                let mut slot = self
                    .staged
                    .arena
                    .reserve(encoded_delta.len())
                    .map_err(map_arena_error)?;
                slot.write_all(&encoded_delta).map_err(RuntimeError::Io)?;
                (
                    descriptor.final_revision(),
                    ResourceKindV1::CoreJournal,
                    slot.seal().map_err(map_arena_error)?,
                )
            }
            CoreProcessDeltaSeal::Outpaced { .. } => {
                let precopy = core.process_precopy().await;
                let snapshot =
                    tokio::task::spawn_blocking(move || precopy.version.prepare_process_transfer())
                        .await
                        .map_err(RuntimeError::from)?
                        .map_err(|error| RuntimeError::Config(error.to_string()))?;
                let mut snapshot_slot = self
                    .staged
                    .arena
                    .reserve(snapshot.bytes().len())
                    .map_err(map_arena_error)?;
                snapshot_slot
                    .write_all(snapshot.bytes())
                    .map_err(RuntimeError::Io)?;
                (
                    snapshot.revision(),
                    ResourceKindV1::CoreSnapshot,
                    snapshot_slot.seal().map_err(map_arena_error)?,
                )
            }
        };
        Ok(ExecutableCorePrestage {
            epoch_revision: self.staged.epoch_revision,
            revision,
            kind,
            region,
        })
    }

    /// Counts commits made since a prepared update, for pre-freeze catch-up admission. This is
    /// not a freeze barrier: any mutation admitted before gate closure remains in the final delta.
    #[must_use]
    pub fn core_prestage_lag(&self, update: &ExecutableCorePrestage) -> u64 {
        self.staged
            .runtime
            .authority
            .active()
            .core
            .version()
            .revision()
            .value()
            .saturating_sub(update.revision())
    }
    #[must_use]
    pub const fn epoch_revision(&self) -> u64 {
        self.staged.epoch_revision()
    }

    #[must_use]
    pub const fn validated_config_digest(&self) -> [u8; 32] {
        self.staged.validated_config_digest()
    }

    #[must_use]
    pub fn plugin_artifacts(&self) -> &[ExecutablePluginArtifact] {
        self.staged.plugin_artifacts()
    }

    #[must_use]
    pub const fn core_snapshot_revision_value(&self) -> u64 {
        self.staged.core_snapshot_revision().value()
    }

    #[must_use]
    pub const fn core_snapshot_region(&self) -> &SealedArenaRegion {
        self.staged.core_snapshot_region()
    }

    #[must_use]
    pub const fn persisted_core_revision_value(&self) -> u64 {
        self.staged.persisted_core_revision().value()
    }

    #[must_use]
    pub fn latest_dirty_core_revision_value(&self) -> Option<u64> {
        self.staged
            .latest_dirty_core_revision()
            .map(CoreRevision::value)
    }

    #[must_use]
    pub const fn staged_directory_revision(&self) -> u64 {
        self.staged.staged_directory_revision()
    }

    #[must_use]
    pub const fn active_generation_id_value(&self) -> u64 {
        self.staged.active_generation_id().0
    }

    #[must_use]
    pub const fn initial_directory(&self) -> &ExecutableDirectoryPrestage {
        self.staged.initial_directory()
    }

    #[must_use]
    pub const fn online_auth_keys_region(&self) -> Option<&SealedArenaRegion> {
        self.staged.online_auth_keys_region()
    }

    #[must_use]
    pub fn current_directory_revision(&self) -> u64 {
        self.staged.current_directory_revision()
    }

    #[must_use]
    pub fn arena(&self) -> &Arc<SharedTransferArena> {
        self.staged.arena()
    }

    /// Captures a stable directory revision while the child has no network authority.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the projection is unstable, exceeds policy, or cannot be
    /// written into the bounded transfer arena.
    pub async fn prestage_directory(&self) -> Result<ExecutableDirectoryPrestage, RuntimeError> {
        self.staged.prestage_directory().await
    }

    /// Duplicates all OS sockets for the already-running child while the parent remains the only
    /// read/write authority.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a listener or TCP session cannot produce a target-bound
    /// transfer capability, or when the session directory changes during preparation.
    pub async fn prepare_network_transfer(
        &self,
        target: SocketTransferTarget,
    ) -> Result<ExecutableNetworkPrestage, RuntimeError> {
        let (listeners, sessions) = tokio::join!(
            self.staged
                .runtime
                .topology_resources
                .duplicate_for_process_transfer(target),
            self.staged
                .runtime
                .sessions
                .prepare_process_transfer_sockets(target, Arc::clone(&self.staged.arena)),
        );
        let listeners = match listeners {
            Ok(listeners) => listeners,
            Err(error) => {
                return match self.staged.rollback_session_transports().await {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(RuntimeError::Config(format!(
                        "{error}; process-transfer preparation rollback failed: {rollback}"
                    ))),
                };
            }
        };
        let PreparedProcessSessionSockets {
            directory_revision,
            sessions,
        } = sessions?;
        Ok(ExecutableNetworkPrestage {
            target,
            directory_revision,
            listeners,
            sessions,
        })
    }

    /// Abandons preparation before ingress or the data-plane gate is frozen.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a session cannot discard its preallocated transfer slot.
    pub async fn abort_preparation(mut self) -> Result<(), RuntimeError> {
        let rollback = self.staged.rollback_session_transports().await;
        self.staged
            .runtime
            .authority
            .active()
            .core
            .end_precopy()
            .await;
        self.staged.status_authority.finish();
        rollback
    }

    /// Closes parent ingress and the data-plane gate, then seals the acknowledged transfer state.
    ///
    /// The acknowledged directory capability is consumed so a raw or stale revision cannot be
    /// mistaken for the exact projection already validated by the child.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] after restoring the parent runtime when the directory changed,
    /// session transport sealing failed, or the bounded core journal was outpaced.
    pub async fn freeze(
        mut self,
        network: ExecutableNetworkPrepared,
        acknowledged_directory: ExecutableDirectoryPrestage,
        acknowledged_core: ExecutableCorePrestage,
    ) -> Result<ExecutableUpgradeFrozen, RuntimeError> {
        if acknowledged_core.epoch_revision != self.staged.epoch_revision {
            let cause =
                RuntimeError::Config("core pre-stage belongs to another runtime epoch".to_string());
            let rollback = self.abort_preparation().await;
            return Err(combine_abort_errors(cause, rollback, Ok(())));
        }
        let acknowledged_revision = acknowledged_directory.directory.revision;
        if network.directory_revision != acknowledged_revision {
            let rollback = self.staged.rollback_session_transports().await;
            self.staged
                .runtime
                .authority
                .active()
                .core
                .end_precopy()
                .await;
            let cause = RuntimeError::CutoverDirectoryChanged {
                prepared_revision: network.directory_revision,
                sealed_revision: acknowledged_revision,
            };
            return Err(combine_abort_errors(cause, rollback, Ok(())));
        }
        let current_revision = self.staged.current_directory_revision();
        if current_revision != acknowledged_revision {
            let rollback = self.staged.rollback_session_transports().await;
            self.staged
                .runtime
                .authority
                .active()
                .core
                .end_precopy()
                .await;
            let cause = RuntimeError::CutoverDirectoryChanged {
                prepared_revision: acknowledged_revision,
                sealed_revision: current_revision,
            };
            return Err(combine_abort_errors(cause, rollback, Ok(())));
        }
        let connection_mix = connection_mix(&acknowledged_directory.directory);
        let session_count = acknowledged_directory.directory.sessions.len();
        let prepare_duration = self.started_at.elapsed();
        let data_plane = self.staged.runtime.authority.freeze().await;
        let ingress = match self
            .staged
            .runtime
            .topology_resources
            .freeze_ingress()
            .await
        {
            Ok(ingress) => ingress,
            Err(error) => {
                let rollback = self.staged.rollback_session_transports().await;
                self.staged
                    .runtime
                    .authority
                    .active()
                    .core
                    .end_precopy()
                    .await;
                let resumed = data_plane.resume();
                self.staged.runtime.authority.record_cutover(CutoverReport {
                    operation: CutoverOperation::ExecutableUpgrade,
                    mode: None,
                    connection_mix,
                    session_count,
                    stage_us: duration_us(self.staged.stage_duration),
                    prepare_us: duration_us(prepare_duration),
                    freeze_us: duration_us(resumed.elapsed()),
                    resume_us: 0,
                    outcome: CutoverOutcome::Aborted,
                    epoch_revision: self.staged.epoch_revision,
                });
                return Err(combine_abort_errors(error, rollback, Ok(())));
            }
        };

        let sealed = async {
            let sealed_revision = self.staged.current_directory_revision();
            if sealed_revision != acknowledged_revision {
                return Err(RuntimeError::CutoverDirectoryChanged {
                    prepared_revision: acknowledged_revision,
                    sealed_revision,
                });
            }
            // Closed ingress fixes peer ownership: the router seals only unclaimed peers,
            // session actors seal their claimed peers, and the closed gate fixes core state.
            // Complete all three branches even on failure before rollback can reopen authority.
            let (raknet_router_state, session_states, core_delta) = tokio::join!(
                async {
                    let peers = ingress.seal_process_transfer().await?;
                    self.staged.seal_raknet_router_state(&peers).await
                },
                self.staged.freeze_session_transports(acknowledged_revision),
                self.staged
                    .seal_final_core_delta(acknowledged_core.revision),
            );
            let raknet_router_state = raknet_router_state?;
            let session_states = session_states?;
            let network = seal_frozen_network(network, session_states, raknet_router_state)?;
            let sealed_revision = self.staged.current_directory_revision();
            if sealed_revision != acknowledged_revision {
                return Err(RuntimeError::CutoverDirectoryChanged {
                    prepared_revision: acknowledged_revision,
                    sealed_revision,
                });
            }
            let core_delta = match core_delta? {
                ExecutableCoreDelta::Ready {
                    descriptor,
                    region,
                    persisted_revision,
                    latest_dirty_revision,
                } => ExecutableSealedCoreDelta {
                    descriptor,
                    region,
                    persisted_revision,
                    latest_dirty_revision,
                },
                ExecutableCoreDelta::Outpaced {
                    requested_revision,
                    earliest_revision,
                } => {
                    return Err(RuntimeError::CutoverOutpaced {
                        resource: "core journal",
                        staged_revision: requested_revision.value(),
                        earliest_revision: earliest_revision.value(),
                    });
                }
            };
            self.staged.mark_frozen()?;
            Ok((core_delta, network))
        }
        .await;

        let (core_delta, network) = match sealed {
            Ok(sealed) => sealed,
            Err(error) => {
                return Err(self
                    .abort_freeze(ingress, data_plane, error, connection_mix, session_count)
                    .await);
            }
        };
        self.staged.status_authority.preserve_on_drop();
        Ok(ExecutableUpgradeFrozen {
            preparing: self,
            ingress,
            data_plane,
            final_directory: acknowledged_directory,
            core_delta,
            network,
            connection_mix,
            session_count,
            prepare_duration,
        })
    }

    async fn abort_freeze(
        mut self,
        ingress: FrozenTopologyIngress,
        data_plane: FrozenDataPlane,
        cause: RuntimeError,
        connection_mix: CutoverConnectionMix,
        session_count: usize,
    ) -> RuntimeError {
        let rollback_result = self.staged.rollback_session_transports().await;
        self.staged
            .runtime
            .authority
            .active()
            .core
            .end_precopy()
            .await;
        let ingress_result = ingress.resume().await;
        let resumed = data_plane.resume();
        let freeze_duration = resumed.elapsed();
        self.staged.runtime.authority.record_cutover(CutoverReport {
            operation: CutoverOperation::ExecutableUpgrade,
            mode: None,
            connection_mix,
            session_count,
            stage_us: duration_us(self.staged.stage_duration),
            prepare_us: duration_us(self.started_at.elapsed()),
            freeze_us: duration_us(freeze_duration),
            resume_us: 0,
            outcome: CutoverOutcome::Aborted,
            epoch_revision: self.staged.epoch_revision,
        });
        self.staged.status_authority.finish();
        combine_abort_errors(cause, rollback_result, ingress_result)
    }
}

impl ExecutableSealedCoreDelta {
    #[must_use]
    pub const fn base_revision_value(&self) -> u64 {
        self.descriptor.base_revision().value()
    }

    #[must_use]
    pub const fn final_revision_value(&self) -> u64 {
        self.descriptor.final_revision().value()
    }

    #[must_use]
    pub const fn encoded_len(&self) -> usize {
        self.descriptor.encoded_len()
    }

    #[must_use]
    pub const fn region(&self) -> &SealedArenaRegion {
        &self.region
    }

    #[must_use]
    pub const fn persisted_revision_value(&self) -> u64 {
        self.persisted_revision.value()
    }

    #[must_use]
    pub fn latest_dirty_revision_value(&self) -> Option<u64> {
        self.latest_dirty_revision.map(CoreRevision::value)
    }
}

impl ExecutableUpgradeFrozen {
    #[must_use]
    pub const fn final_directory(&self) -> &ExecutableDirectoryPrestage {
        &self.final_directory
    }

    #[must_use]
    pub const fn core_delta(&self) -> &ExecutableSealedCoreDelta {
        &self.core_delta
    }

    #[must_use]
    pub const fn network(&self) -> &ExecutableFrozenNetwork {
        &self.network
    }

    #[must_use]
    pub const fn epoch_revision(&self) -> u64 {
        self.preparing.staged.epoch_revision
    }

    #[must_use]
    pub fn connection_mix(&self) -> CutoverConnectionMix {
        self.connection_mix
    }

    #[must_use]
    pub const fn session_count(&self) -> usize {
        self.session_count
    }

    #[must_use]
    pub fn stage_duration_us(&self) -> u64 {
        duration_us(self.preparing.staged.stage_duration)
    }

    #[must_use]
    pub fn prepare_duration_us(&self) -> u64 {
        duration_us(self.prepare_duration)
    }

    /// Returns the monotonic interval since the parent data-plane gate closed.
    #[must_use]
    pub fn freeze_duration(&self) -> Duration {
        self.data_plane.elapsed()
    }

    /// Rolls back a transfer before the child receives commit authority.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if one or more session transports or listeners cannot resume.
    pub async fn abort(self) -> Result<(), RuntimeError> {
        let ExecutableUpgradeFrozen {
            mut preparing,
            ingress,
            data_plane,
            connection_mix,
            session_count,
            ..
        } = self;
        let rollback_result = preparing.staged.rollback_session_transports().await;
        preparing
            .staged
            .runtime
            .authority
            .active()
            .core
            .end_precopy()
            .await;
        let ingress_result = ingress.resume().await;
        let resumed = data_plane.resume();
        let freeze_duration = resumed.elapsed();
        preparing
            .staged
            .runtime
            .authority
            .record_cutover(CutoverReport {
                operation: CutoverOperation::ExecutableUpgrade,
                mode: None,
                connection_mix,
                session_count,
                stage_us: duration_us(preparing.staged.stage_duration),
                prepare_us: duration_us(preparing.started_at.elapsed()),
                freeze_us: duration_us(freeze_duration),
                resume_us: 0,
                outcome: CutoverOutcome::Aborted,
                epoch_revision: preparing.staged.epoch_revision,
            });
        preparing.staged.status_authority.finish();
        combine_resume_results(rollback_result, ingress_result)
    }

    /// Consumes rollback authority immediately before the V1 `Commit` envelope is sent.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if the operator-visible status authority was superseded. The
    /// parent remains frozen and must terminate rather than resume in that case.
    pub fn commit_sent(self) -> Result<ExecutableUpgradeOutcomePending, RuntimeError> {
        self.preparing.staged.mark_outcome_uncertain()?;
        Ok(ExecutableUpgradeOutcomePending { frozen: self })
    }
}

impl ExecutableUpgradeOutcomePending {
    /// Returns the monotonic interval since the parent data-plane gate closed.
    #[must_use]
    pub fn freeze_duration(&self) -> Duration {
        self.frozen.data_plane.elapsed()
    }

    /// Finalizes parent authority only after an authenticated child `Committed` acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if parent-side transport authority cannot be retired. The child is
    /// already authoritative, so the parent stays fail-closed and must terminate.
    pub async fn committed(
        self,
        freeze_duration: Duration,
        resume_duration: Duration,
    ) -> Result<(ExecutableParentCommitHold, CutoverReport), RuntimeError> {
        let child_epoch_revision = self
            .frozen
            .preparing
            .staged
            .epoch_revision
            .checked_add(1)
            .ok_or_else(|| RuntimeError::Config("child epoch revision overflow".to_string()))?;
        self.frozen
            .preparing
            .staged
            .status_authority
            .update(RuntimeUpgradePhase::ParentCommitted)?;
        let report = CutoverReport {
            operation: CutoverOperation::ExecutableUpgrade,
            mode: None,
            connection_mix: self.frozen.connection_mix,
            session_count: self.frozen.session_count,
            stage_us: duration_us(self.frozen.preparing.staged.stage_duration),
            prepare_us: duration_us(self.frozen.prepare_duration),
            freeze_us: duration_us(freeze_duration),
            resume_us: duration_us(resume_duration),
            outcome: CutoverOutcome::Committed,
            epoch_revision: child_epoch_revision,
        };
        self.frozen
            .preparing
            .staged
            .runtime
            .authority
            .record_cutover(report.clone());
        warn_executable_freeze(freeze_duration);
        self.frozen
            .preparing
            .staged
            .commit_session_transports()
            .await?;
        let ExecutableUpgradeFrozen {
            preparing,
            ingress,
            data_plane,
            ..
        } = self.frozen;
        Ok((
            ExecutableParentCommitHold {
                _preparing: preparing,
                _ingress: ingress,
                _data_plane: data_plane,
            },
            report,
        ))
    }
}

fn prepare_executable_sessions(
    child: &mut ExecutableChildPrepared,
    encoded_states: Vec<(ConnectionId, &[u8])>,
) -> Result<Vec<PreparedExecutableSession>, RuntimeError> {
    if encoded_states.len() != child.directory.sessions.len() {
        return Err(RuntimeError::Config(format!(
            "child received {} session states for a {} entry directory",
            encoded_states.len(),
            child.directory.sessions.len()
        )));
    }
    let mut directory = child
        .directory
        .sessions
        .iter()
        .map(|entry| (entry.connection_id, entry))
        .collect::<BTreeMap<_, _>>();
    if directory.len() != child.directory.sessions.len() {
        return Err(RuntimeError::Config(
            "child session directory contains duplicate connection ids".to_string(),
        ));
    }
    let mut generations = BTreeMap::new();
    let mut candidates = Vec::with_capacity(encoded_states.len());
    for (connection_id, bytes) in encoded_states {
        let directory_entry = directory.remove(&connection_id).ok_or_else(|| {
            RuntimeError::Config(format!(
                "session state {connection_id:?} has no acknowledged directory entry"
            ))
        })?;
        let binding = if directory_entry.phase == ConnectionPhase::Play {
            let phase = child
                .prepared_play_phases
                .remove(&connection_id)
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "session {connection_id:?} has no pre-staged Play binding"
                    ))
                })?;
            ExecutableSessionImportBinding::Prepared(phase)
        } else {
            let generation = Arc::clone(
                generations
                    .entry(directory_entry.generation_id.0)
                    .or_insert_with(|| executable_generation(child, directory_entry.generation_id)),
            );
            ExecutableSessionImportBinding::Pending(generation)
        };
        candidates.push((directory_entry, bytes, binding));
    }
    if let Some(connection_id) = directory.keys().next() {
        return Err(RuntimeError::Config(format!(
            "directory session {connection_id:?} has no transferred actor state"
        )));
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    candidates.sort_by_key(|(entry, _, _)| entry.connection_id);
    let session_count = candidates.len();
    let worker_count = executable_session_prepare_worker_count(session_count);
    let chunk_size = session_count.div_ceil(worker_count);
    let mut candidates = candidates.into_iter();
    let child = &*child;
    // Each worker owns decode, semantic validation and plugin/transport import for its
    // chunk. No second worker wave or central collection of large decoded states is needed.
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(worker_count);
        loop {
            let chunk = candidates.by_ref().take(chunk_size).collect::<Vec<_>>();
            if chunk.is_empty() {
                break;
            }
            workers.push(scope.spawn(move || {
                chunk
                    .into_iter()
                    .map(|(entry, bytes, binding)| {
                        let state = ExecutableSessionState::decode(bytes)?;
                        validate_executable_actor(state.actor())?;
                        if state.transport() != entry.transport {
                            return Err(RuntimeError::Config(format!(
                                "session {:?} state transport does not match its directory entry",
                                entry.connection_id
                            )));
                        }
                        validate_actor_directory_entry(entry, state.actor())?;
                        let phase = match binding {
                            ExecutableSessionImportBinding::Prepared(phase) => phase,
                            ExecutableSessionImportBinding::Pending(generation) => {
                                prepare_executable_phase(child, &generation, state.actor())?
                            }
                        };
                        let state = match state {
                            ExecutableSessionState::Tcp { actor, transport } => {
                                import_executable_plugin_state(&actor, &phase)?;
                                ValidatedExecutableSessionState::Tcp { actor, transport }
                            }
                            ExecutableSessionState::Bedrock { actor, transport } => {
                                let peer =
                                    transport.peer.into_validated(RakNetBudgets::default())?;
                                import_executable_plugin_state(&actor, &phase)?;
                                ValidatedExecutableSessionState::Bedrock {
                                    actor,
                                    peer,
                                    reader_compression_threshold: transport
                                        .reader_compression_threshold,
                                    writer_compression_threshold: transport
                                        .writer_compression_threshold,
                                }
                            }
                        };
                        Ok(PreparedExecutableSession { state, phase })
                    })
                    .collect::<Result<Vec<_>, RuntimeError>>()
            }));
        }
        let mut prepared = Vec::with_capacity(session_count);
        for worker in workers {
            let mut imported = worker.join().map_err(|_| {
                RuntimeError::Config("executable session import worker panicked".to_string())
            })??;
            prepared.append(&mut imported);
        }
        Ok(prepared)
    })
}

fn executable_session_prepare_worker_count(session_count: usize) -> usize {
    std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(EXECUTABLE_SESSION_PREPARE_WORKER_LIMIT)
        .min(session_count)
}

fn prepare_directory_play_phases(
    child: &ExecutableChildPrepared,
    directory: &ExecutableSessionDirectory,
) -> Result<BTreeMap<ConnectionId, SessionPhase>, RuntimeError> {
    let mut generations = BTreeMap::new();
    let mut phases = BTreeMap::new();
    for entry in &directory.sessions {
        if entry.phase != ConnectionPhase::Play {
            continue;
        }
        let adapter_id = entry.adapter_id.as_ref().ok_or_else(|| {
            RuntimeError::Config(format!(
                "Play directory entry {:?} has no protocol adapter",
                entry.connection_id
            ))
        })?;
        let gameplay_profile = entry.gameplay_profile.as_ref().ok_or_else(|| {
            RuntimeError::Config(format!(
                "Play directory entry {:?} has no gameplay profile",
                entry.connection_id
            ))
        })?;
        let player_id = entry.player_id.ok_or_else(|| {
            RuntimeError::Config(format!(
                "Play directory entry {:?} has no player identity",
                entry.connection_id
            ))
        })?;
        let entity_id = entry.entity_id.ok_or_else(|| {
            RuntimeError::Config(format!(
                "Play directory entry {:?} has no entity identity",
                entry.connection_id
            ))
        })?;
        let generation = Arc::clone(
            generations
                .entry(entry.generation_id.0)
                .or_insert_with(|| executable_generation(child, entry.generation_id)),
        );
        let binding = ExecutableSessionBinding {
            transport: entry.transport,
            adapter_id: adapter_id.clone(),
            gameplay_profile: GameplayProfileId::new(gameplay_profile.clone()),
            protocol_generation: entry.protocol_generation,
            gameplay_generation: entry.gameplay_generation,
        };
        let phase = SessionPhase::Play(PlaySession {
            binding: prepare_executable_binding(
                child,
                &generation,
                entry.connection_id,
                &binding,
                Some(entity_id),
            )?,
            player_id,
            entity_id,
        });
        validate_directory_entry(entry, &phase)?;
        if phases.insert(entry.connection_id, phase).is_some() {
            return Err(RuntimeError::Config(format!(
                "child session directory duplicates Play session {:?}",
                entry.connection_id
            )));
        }
    }
    Ok(phases)
}

fn executable_generation(
    child: &ExecutableChildPrepared,
    generation_id: GenerationId,
) -> Arc<ActiveGeneration> {
    Arc::new(ActiveGeneration {
        generation_id,
        config: child.config.as_inner().clone(),
        protocol_registry: child.active_protocols.protocols.clone(),
        default_adapter: Arc::clone(&child.active_protocols.default_adapter),
        default_bedrock_adapter: child.active_protocols.default_bedrock_adapter.clone(),
        listener_bindings: Vec::new(),
    })
}

fn validate_actor_directory_entry(
    entry: &SessionStatusSnapshot,
    actor: &ExecutableSessionActorSnapshot,
) -> Result<(), RuntimeError> {
    let (phase, binding, player_id, entity_id) = match &actor.phase {
        ExecutableSessionPhase::Handshaking => (ConnectionPhase::Handshaking, None, None, None),
        ExecutableSessionPhase::Status(binding) => {
            (ConnectionPhase::Status, Some(binding), None, None)
        }
        ExecutableSessionPhase::Login(login) => {
            let binding = match login {
                ExecutableLoginSession::Negotiating { binding }
                | ExecutableLoginSession::Authenticating { binding, .. }
                | ExecutableLoginSession::AcceptedWritePending { binding, .. } => binding,
            };
            (ConnectionPhase::Login, Some(binding), None, None)
        }
        ExecutableSessionPhase::Play {
            binding,
            player_id,
            entity_id,
        } => (
            ConnectionPhase::Play,
            Some(binding),
            Some(*player_id),
            Some(*entity_id),
        ),
        ExecutableSessionPhase::Closing { phase, .. } => (*phase, None, None, None),
    };
    let binding_matches = match (phase, binding) {
        (ConnectionPhase::Play, Some(binding)) => {
            entry.adapter_id.as_deref() == Some(binding.adapter_id.as_str())
                && entry.gameplay_profile.as_deref() == Some(binding.gameplay_profile.as_str())
                && entry.protocol_generation == binding.protocol_generation
                && entry.gameplay_generation == binding.gameplay_generation
        }
        (ConnectionPhase::Status | ConnectionPhase::Login, Some(binding)) => {
            entry.adapter_id.as_deref() == Some(binding.adapter_id.as_str())
                && entry.gameplay_profile.is_none()
                && entry.protocol_generation.is_none()
                && entry.gameplay_generation.is_none()
        }
        (_, None) => {
            entry.adapter_id.is_none()
                && entry.gameplay_profile.is_none()
                && entry.protocol_generation.is_none()
                && entry.gameplay_generation.is_none()
        }
        _ => false,
    };
    if entry.connection_id == actor.connection_id
        && entry.generation_id == actor.generation_id
        && entry.transport == actor_transport(actor)
        && entry.phase == phase
        && entry.player_id == player_id
        && entry.entity_id == entity_id
        && binding_matches
    {
        Ok(())
    } else {
        Err(RuntimeError::Config(format!(
            "session {:?} actor state does not match its acknowledged directory entry",
            entry.connection_id
        )))
    }
}

fn validate_executable_actor(actor: &ExecutableSessionActorSnapshot) -> Result<(), RuntimeError> {
    ensure_session_blob_budget(
        "executable session read buffer bytes",
        actor.read_buffer.len(),
        MAX_EXECUTABLE_SESSION_READ_BUFFER_BYTES,
    )?;
    if actor.queued_messages.len() > super::SESSION_OUTBOUND_QUEUE_CAPACITY {
        return Err(RuntimeError::BudgetExceeded {
            resource: "executable queued session messages",
            requested: actor.queued_messages.len(),
            limit: super::SESSION_OUTBOUND_QUEUE_CAPACITY,
        });
    }
    for message in &actor.queued_messages {
        if let ExecutableQueuedSessionMessage::Terminate { reason } = message {
            ensure_session_blob_budget(
                "executable session termination reason bytes",
                reason.len(),
                MAX_EXECUTABLE_SESSION_DIAGNOSTIC_BYTES,
            )?;
        }
    }
    if let Some(reason) = &actor.pending_terminate {
        ensure_session_blob_budget(
            "executable pending termination reason bytes",
            reason.len(),
            MAX_EXECUTABLE_SESSION_DIAGNOSTIC_BYTES,
        )?;
    }
    Ok(())
}

fn prepare_executable_phase(
    child: &ExecutableChildPrepared,
    generation: &Arc<ActiveGeneration>,
    actor: &ExecutableSessionActorSnapshot,
) -> Result<SessionPhase, RuntimeError> {
    let phase = match &actor.phase {
        ExecutableSessionPhase::Handshaking => SessionPhase::Handshaking {
            generation: Arc::clone(generation),
        },
        ExecutableSessionPhase::Status(snapshot) => SessionPhase::Status(
            prepare_executable_binding(child, generation, actor.connection_id, snapshot, None)?,
        ),
        ExecutableSessionPhase::Login(ExecutableLoginSession::Negotiating {
            binding: snapshot,
        }) => SessionPhase::Login(LoginSession::Negotiating(prepare_executable_binding(
            child,
            generation,
            actor.connection_id,
            snapshot,
            None,
        )?)),
        ExecutableSessionPhase::Login(ExecutableLoginSession::Authenticating {
            binding: snapshot,
            username,
            verify_token,
            auth_generation,
        }) => {
            if snapshot.transport != TransportKind::Tcp {
                return Err(RuntimeError::Config(format!(
                    "session {:?} has an online authentication challenge on a non-TCP transport",
                    actor.connection_id
                )));
            }
            let captured = child.selection.auth_profile.capture_generation()?;
            if captured.generation_id() != *auth_generation {
                return Err(RuntimeError::Config(format!(
                    "session {:?} authentication generation is unavailable in the child",
                    actor.connection_id
                )));
            }
            SessionPhase::Login(LoginSession::Authenticating {
                binding: prepare_executable_binding(
                    child,
                    generation,
                    actor.connection_id,
                    snapshot,
                    None,
                )?,
                challenge: LoginChallenge {
                    username: username.clone(),
                    verify_token: *verify_token,
                    auth_generation: captured,
                },
            })
        }
        ExecutableSessionPhase::Login(ExecutableLoginSession::AcceptedWritePending {
            binding: snapshot,
            player_id,
            entity_id,
        }) => SessionPhase::Login(LoginSession::AcceptedWritePending {
            binding: prepare_executable_binding(
                child,
                generation,
                actor.connection_id,
                snapshot,
                Some(*entity_id),
            )?,
            player_id: *player_id,
            entity_id: *entity_id,
        }),
        ExecutableSessionPhase::Play {
            binding: snapshot,
            player_id,
            entity_id,
        } => SessionPhase::Play(PlaySession {
            binding: prepare_executable_binding(
                child,
                generation,
                actor.connection_id,
                snapshot,
                Some(*entity_id),
            )?,
            player_id: *player_id,
            entity_id: *entity_id,
        }),
        ExecutableSessionPhase::Closing { transport, phase } => SessionPhase::Closing {
            generation_id: actor.generation_id,
            transport: *transport,
            phase: *phase,
        },
    };
    if phase.generation_id() != actor.generation_id || phase.transport() != actor_transport(actor) {
        return Err(RuntimeError::Config(format!(
            "session {:?} phase metadata is internally inconsistent",
            actor.connection_id
        )));
    }
    Ok(phase)
}

fn prepare_executable_binding(
    child: &ExecutableChildPrepared,
    generation: &Arc<ActiveGeneration>,
    connection_id: ConnectionId,
    snapshot: &ExecutableSessionBinding,
    entity_id: Option<EntityId>,
) -> Result<BoundSession, RuntimeError> {
    let adapter = child
        .active_protocols
        .protocols
        .resolve_adapter(&snapshot.adapter_id)
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "session {connection_id:?} references unavailable adapter `{}`",
                snapshot.adapter_id
            ))
        })?;
    if adapter.descriptor().transport != snapshot.transport {
        return Err(RuntimeError::Config(format!(
            "session {connection_id:?} adapter `{}` changed transport",
            snapshot.adapter_id
        )));
    }
    if adapter.plugin_generation_id() != snapshot.protocol_generation {
        return Err(RuntimeError::Config(format!(
            "session {connection_id:?} protocol generation does not match adapter `{}`",
            snapshot.adapter_id
        )));
    }
    let gameplay = child
        .selection
        .loaded_plugins
        .resolve_gameplay_profile(snapshot.gameplay_profile.as_str())
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "session {connection_id:?} references unavailable gameplay profile `{}`",
                snapshot.gameplay_profile.as_str()
            ))
        })?;
    if gameplay.plugin_generation_id() != snapshot.gameplay_generation {
        return Err(RuntimeError::Config(format!(
            "session {connection_id:?} gameplay generation does not match profile `{}`",
            snapshot.gameplay_profile.as_str()
        )));
    }
    Ok(BoundSession::new(
        Arc::clone(generation),
        snapshot.transport,
        adapter,
        gameplay,
        entity_id,
    ))
}

fn actor_transport(actor: &ExecutableSessionActorSnapshot) -> TransportKind {
    match &actor.phase {
        ExecutableSessionPhase::Handshaking => TransportKind::Tcp,
        ExecutableSessionPhase::Status(binding)
        | ExecutableSessionPhase::Login(ExecutableLoginSession::Negotiating { binding })
        | ExecutableSessionPhase::Login(ExecutableLoginSession::Authenticating {
            binding, ..
        })
        | ExecutableSessionPhase::Login(ExecutableLoginSession::AcceptedWritePending {
            binding,
            ..
        })
        | ExecutableSessionPhase::Play { binding, .. } => binding.transport,
        ExecutableSessionPhase::Closing { transport, .. } => *transport,
    }
}

fn validate_directory_entry(
    entry: &SessionStatusSnapshot,
    phase: &SessionPhase,
) -> Result<(), RuntimeError> {
    let actual = session_status_snapshot(entry.connection_id, phase);
    if *entry == actual {
        Ok(())
    } else {
        Err(RuntimeError::Config(format!(
            "session {:?} actor state does not match its acknowledged directory entry",
            entry.connection_id
        )))
    }
}

fn session_status_snapshot(
    connection_id: ConnectionId,
    phase: &SessionPhase,
) -> SessionStatusSnapshot {
    let view = phase.directory_entry();
    let (player_id, entity_id) = match &view.phase {
        super::SessionDirectoryPhase::Play {
            player_id,
            entity_id,
            ..
        } => (Some(*player_id), Some(*entity_id)),
        _ => (None, None),
    };
    SessionStatusSnapshot {
        connection_id,
        generation_id: view.generation_id,
        transport: view.transport,
        phase: view.phase(),
        adapter_id: view.adapter_id().map(str::to_string),
        gameplay_profile: view
            .gameplay_profile()
            .map(|profile| profile.as_str().to_string()),
        player_id,
        entity_id,
        protocol_generation: view.protocol_generation(),
        gameplay_generation: view.gameplay_generation(),
    }
}

fn import_executable_plugin_state(
    actor: &ExecutableSessionActorSnapshot,
    phase: &SessionPhase,
) -> Result<(), RuntimeError> {
    let Some(binding) = phase.binding() else {
        if !actor.protocol_session_blob.is_empty() || !actor.gameplay_session_blob.is_empty() {
            return Err(RuntimeError::Config(format!(
                "session {:?} has plugin handoff data without a plugin binding",
                actor.connection_id
            )));
        }
        return Ok(());
    };
    let protocol = phase.protocol_snapshot(actor.connection_id);
    binding
        .adapter
        .import_session_state(&protocol, &actor.protocol_session_blob)?;
    match phase.gameplay_snapshot() {
        Some(gameplay) => binding
            .gameplay
            .import_session_state(&gameplay, &actor.gameplay_session_blob)?,
        None if actor.gameplay_session_blob.is_empty() => {}
        None => {
            return Err(RuntimeError::Config(format!(
                "session {:?} has gameplay handoff data before gameplay admission",
                actor.connection_id
            )));
        }
    }
    Ok(())
}

pub(crate) fn executable_session_slot_capacity(
    phase: &SessionPhase,
) -> Result<usize, RuntimeError> {
    let (protocol, gameplay) = phase.binding().map_or((0, 0), |binding| {
        (
            binding.adapter.max_session_handoff_bytes(),
            binding.gameplay.max_session_handoff_bytes(),
        )
    });
    let capacity = EXECUTABLE_SESSION_STATE_FIXED_SLOT_BYTES
        .checked_add(protocol)
        .and_then(|capacity| capacity.checked_add(gameplay))
        .ok_or(RuntimeError::BudgetExceeded {
            resource: "executable session state slot bytes",
            requested: usize::MAX,
            limit: MAX_EXECUTABLE_SESSION_STATE_BYTES,
        })?;
    ensure_session_blob_budget(
        "executable session state slot bytes",
        capacity,
        MAX_EXECUTABLE_SESSION_STATE_BYTES,
    )?;
    Ok(capacity)
}

pub(crate) fn executable_session_state(
    actor: ExecutableSessionActorSnapshot,
    transport: FrozenProcessTransportState,
) -> Result<ExecutableSessionState, RuntimeError> {
    match transport {
        FrozenProcessTransportState::Tcp { decrypt, encrypt } => {
            if actor_transport(&actor) != TransportKind::Tcp {
                return Err(RuntimeError::Config(format!(
                    "session {:?} actor phase changed away from its TCP transport",
                    actor.connection_id
                )));
            }
            Ok(ExecutableSessionState::Tcp {
                actor,
                transport: ExecutableTcpTransportSnapshot {
                    decrypt: decrypt.map(executable_cipher_snapshot),
                    encrypt: encrypt.map(executable_cipher_snapshot),
                },
            })
        }
        FrozenProcessTransportState::Bedrock {
            peer,
            reader_compression_threshold,
            writer_compression_threshold,
        } => {
            if actor_transport(&actor) != TransportKind::Udp {
                return Err(RuntimeError::Config(format!(
                    "session {:?} actor phase changed away from its Bedrock transport",
                    actor.connection_id
                )));
            }
            Ok(ExecutableSessionState::Bedrock {
                actor,
                transport: ExecutableBedrockTransportSnapshot {
                    peer,
                    reader_compression_threshold,
                    writer_compression_threshold,
                },
            })
        }
    }
}

pub(crate) fn capture_executable_session(
    connection_id: ConnectionId,
    epoch_revision: u64,
    phase: &SessionPhase,
    read_buffer: &BytesMut,
    queued_messages: &VecDeque<SessionMessage>,
    pending_terminate: Option<&str>,
) -> Result<ExecutableSessionActorSnapshot, RuntimeError> {
    let protocol_snapshot = phase.protocol_snapshot(connection_id);
    let protocol_session_blob = match phase.binding() {
        Some(binding) => {
            let blob = binding.adapter.export_session_state(&protocol_snapshot)?;
            ensure_session_blob_budget(
                "protocol session handoff bytes",
                blob.len(),
                binding.adapter.max_session_handoff_bytes(),
            )?;
            blob
        }
        None => Vec::new(),
    };
    let gameplay_session = phase.gameplay_snapshot();
    let gameplay_session_blob = match (phase.binding(), gameplay_session.as_ref()) {
        (Some(binding), Some(session)) => {
            let blob = binding.gameplay.export_session_state(session)?;
            ensure_session_blob_budget(
                "gameplay session handoff bytes",
                blob.len(),
                binding.gameplay.max_session_handoff_bytes(),
            )?;
            blob
        }
        _ => Vec::new(),
    };
    let phase_snapshot = match phase {
        SessionPhase::Handshaking { .. } => ExecutableSessionPhase::Handshaking,
        SessionPhase::Status(binding) => {
            ExecutableSessionPhase::Status(executable_binding(binding))
        }
        SessionPhase::Login(LoginSession::Negotiating(binding)) => {
            ExecutableSessionPhase::Login(ExecutableLoginSession::Negotiating {
                binding: executable_binding(binding),
            })
        }
        SessionPhase::Login(LoginSession::Authenticating { binding, challenge }) => {
            ExecutableSessionPhase::Login(ExecutableLoginSession::Authenticating {
                binding: executable_binding(binding),
                username: challenge.username.clone(),
                verify_token: challenge.verify_token,
                auth_generation: challenge.auth_generation.generation_id(),
            })
        }
        SessionPhase::Login(LoginSession::AcceptedWritePending {
            binding,
            player_id,
            entity_id,
        }) => ExecutableSessionPhase::Login(ExecutableLoginSession::AcceptedWritePending {
            binding: executable_binding(binding),
            player_id: *player_id,
            entity_id: *entity_id,
        }),
        SessionPhase::Play(play) => ExecutableSessionPhase::Play {
            binding: executable_binding(&play.binding),
            player_id: play.player_id,
            entity_id: play.entity_id,
        },
        SessionPhase::Closing {
            transport, phase, ..
        } => ExecutableSessionPhase::Closing {
            transport: *transport,
            phase: *phase,
        },
    };
    Ok(ExecutableSessionActorSnapshot {
        connection_id,
        epoch_revision,
        generation_id: phase.generation_id(),
        phase: phase_snapshot,
        read_buffer: read_buffer.to_vec(),
        queued_messages: queued_messages
            .iter()
            .map(|message| match message {
                SessionMessage::Events(events) => ExecutableQueuedSessionMessage::Events(
                    events.iter().map(|event| event.as_ref().clone()).collect(),
                ),
                SessionMessage::Terminate { reason } => ExecutableQueuedSessionMessage::Terminate {
                    reason: reason.clone(),
                },
            })
            .collect(),
        pending_terminate: pending_terminate.map(str::to_string),
        protocol_session_blob,
        gameplay_session_blob,
    })
}

fn executable_binding(binding: &super::BoundSession) -> ExecutableSessionBinding {
    ExecutableSessionBinding {
        transport: binding.transport,
        adapter_id: binding.adapter.descriptor().adapter_id,
        gameplay_profile: binding.gameplay.profile_id(),
        protocol_generation: binding.adapter.plugin_generation_id(),
        gameplay_generation: binding.gameplay.plugin_generation_id(),
    }
}

fn ensure_session_blob_budget(
    resource: &'static str,
    requested: usize,
    limit: usize,
) -> Result<(), RuntimeError> {
    if requested > limit {
        Err(RuntimeError::BudgetExceeded {
            resource,
            requested,
            limit,
        })
    } else {
        Ok(())
    }
}

fn decode_raknet_router_state(bytes: &[u8]) -> Result<Vec<ValidatedPeerSnapshot>, RuntimeError> {
    ensure_session_blob_budget(
        "executable RakNet router state bytes",
        bytes.len(),
        EXECUTABLE_RAKNET_ROUTER_STATE_BYTES,
    )?;
    let mut reader = Cursor::new(bytes);
    let peers: Vec<PeerSnapshot> = ciborium::de::from_reader(&mut reader)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    if usize::try_from(reader.position()).ok() != Some(bytes.len()) {
        return Err(RuntimeError::Config(
            "executable RakNet router state contains trailing bytes".to_string(),
        ));
    }
    if peers.len() > RakNetBudgets::default().max_peers {
        return Err(RuntimeError::BudgetExceeded {
            resource: "executable RakNet peer snapshots",
            requested: peers.len(),
            limit: RakNetBudgets::default().max_peers,
        });
    }
    let peers = validate_peer_snapshots(peers, RakNetBudgets::default())?;
    let mut addresses = std::collections::HashSet::with_capacity(peers.len());
    for peer in &peers {
        if !addresses.insert(peer.remote_addr()) {
            return Err(RuntimeError::Config(format!(
                "executable RakNet router state duplicates peer {}",
                peer.remote_addr()
            )));
        }
    }
    Ok(peers)
}

fn seal_frozen_network(
    network: ExecutableNetworkPrepared,
    frozen: FrozenProcessSessionStates,
    raknet_router_state: Option<SealedArenaRegion>,
) -> Result<ExecutableFrozenNetwork, RuntimeError> {
    let ExecutableNetworkPrepared {
        listeners,
        mut prepared_sessions,
        mut frozen_sessions,
        mut frozen_states,
        ..
    } = network;
    for frozen_session in frozen.sessions {
        let connection_id = frozen_session.state.actor().connection_id;
        let transport = prepared_sessions.remove(&connection_id).ok_or_else(|| {
            RuntimeError::Config(format!(
                "frozen session {connection_id:?} has no prepared network capability"
            ))
        })?;
        frozen_states.push(ExecutableSessionStateResource {
            connection_id,
            region: frozen_session.region,
        });
        let resource = match (transport, frozen_session.state) {
            (TransportKind::Tcp, ExecutableSessionState::Tcp { actor, transport }) => {
                ExecutableSessionTransportResource::Tcp(ExecutableTcpSessionTransport {
                    actor,
                    snapshot: transport,
                })
            }
            (TransportKind::Udp, ExecutableSessionState::Bedrock { actor, transport }) => {
                ExecutableSessionTransportResource::Bedrock(ExecutableBedrockSessionTransport {
                    actor,
                    snapshot: transport,
                })
            }
            _ => {
                return Err(RuntimeError::Config(format!(
                    "session {connection_id:?} changed transport during executable freeze"
                )));
            }
        };
        frozen_sessions.push(resource);
    }
    if let Some((connection_id, _)) = prepared_sessions.into_iter().next() {
        return Err(RuntimeError::Config(format!(
            "prepared network capability for session {connection_id:?} was not frozen"
        )));
    }
    frozen_sessions.sort_by_key(ExecutableSessionTransportResource::connection_id);
    frozen_states.sort_by_key(ExecutableSessionStateResource::connection_id);
    Ok(ExecutableFrozenNetwork {
        listeners,
        sessions: frozen_sessions,
        session_states: frozen_states,
        raknet_router_state,
    })
}

fn executable_cipher_snapshot(
    snapshot: MinecraftStreamCipherSnapshot,
) -> ExecutableMinecraftCipherSnapshot {
    ExecutableMinecraftCipherSnapshot {
        shared_secret: snapshot.shared_secret(),
        shift_register: snapshot.shift_register(),
    }
}

fn imported_cipher_snapshot(
    snapshot: ExecutableMinecraftCipherSnapshot,
) -> MinecraftStreamCipherSnapshot {
    MinecraftStreamCipherSnapshot::from_parts(snapshot.shared_secret, snapshot.shift_register)
}

fn imported_raknet_server_config(
    child: &ExecutableChildPrepared,
    binding: &crate::ListenerBinding,
) -> Result<RakNetServerConfig, RuntimeError> {
    let adapter = child
        .active_protocols
        .default_bedrock_adapter
        .as_ref()
        .ok_or_else(|| {
            RuntimeError::Config(
                "Bedrock listener was transferred without a default Bedrock adapter".to_string(),
            )
        })?;
    let descriptor = adapter.descriptor();
    let listener = adapter.bedrock_listener_descriptor().ok_or_else(|| {
        RuntimeError::Config("default Bedrock adapter has no listener descriptor".to_string())
    })?;
    Ok(RakNetServerConfig {
        bind_addr: binding.local_addr,
        motd: child.config.as_inner().network.motd.clone(),
        server_name: "RevyCraft".to_string(),
        game_version: listener.game_version,
        protocol_number: u32::try_from(descriptor.protocol_number).map_err(|_| {
            RuntimeError::Config(format!(
                "bedrock protocol number {} must be non-negative",
                descriptor.protocol_number
            ))
        })?,
        raknet_version: listener.raknet_version,
        max_players: child.config.as_inner().network.max_players,
        online_players: 0,
        server_guid: 0,
        budgets: RakNetBudgets::default(),
    })
}

fn import_tcp_listener(socket: ExportedSocket) -> Result<tokio::net::TcpListener, RuntimeError> {
    let listener = std::net::TcpListener::from(import_native_socket(socket)?);
    listener.set_nonblocking(true)?;
    Ok(tokio::net::TcpListener::from_std(listener)?)
}

fn import_tcp_stream(socket: ExportedSocket) -> Result<tokio::net::TcpStream, RuntimeError> {
    let stream = std::net::TcpStream::from(import_native_socket(socket)?);
    stream.set_nonblocking(true)?;
    Ok(tokio::net::TcpStream::from_std(stream)?)
}

fn import_udp_socket(socket: ExportedSocket) -> Result<tokio::net::UdpSocket, RuntimeError> {
    let socket = std::net::UdpSocket::from(import_native_socket(socket)?);
    socket.set_nonblocking(true)?;
    Ok(tokio::net::UdpSocket::from_std(socket)?)
}

#[cfg(unix)]
fn import_native_socket(socket: ExportedSocket) -> Result<std::os::fd::OwnedFd, RuntimeError> {
    Ok(socket.into_owned())
}

#[cfg(windows)]
fn import_native_socket(
    socket: ExportedSocket,
) -> Result<std::os::windows::io::OwnedSocket, RuntimeError> {
    socket.import().map_err(RuntimeError::from)
}

fn connection_mix(directory: &ExecutableSessionDirectory) -> CutoverConnectionMix {
    directory
        .sessions
        .iter()
        .fold(CutoverConnectionMix::default(), |mut mix, session| {
            match session.transport {
                TransportKind::Tcp => mix.java += 1,
                TransportKind::Udp => mix.bedrock += 1,
            }
            mix
        })
}

fn validated_config_digest(config: &crate::config::ServerConfig) -> Result<[u8; 32], RuntimeError> {
    let config_value = serde_json::to_value(config).map_err(|error| {
        RuntimeError::Config(format!(
            "validated runtime config could not be represented for executable upgrade: {error}"
        ))
    })?;
    let mut digest = Sha256::new();
    digest.update(b"RevyCraft validated runtime config v1\0");
    update_canonical_json_digest(&config_value, &mut digest);
    Ok(digest.finalize().into())
}

fn update_canonical_json_digest(value: &serde_json::Value, digest: &mut Sha256) {
    match value {
        serde_json::Value::Null => digest.update([0]),
        serde_json::Value::Bool(value) => digest.update([1, u8::from(*value)]),
        serde_json::Value::Number(value) => {
            digest.update([2]);
            update_digest_bytes(value.to_string().as_bytes(), digest);
        }
        serde_json::Value::String(value) => {
            digest.update([3]);
            update_digest_bytes(value.as_bytes(), digest);
        }
        serde_json::Value::Array(values) => {
            digest.update([4]);
            update_digest_length(values.len(), digest);
            for value in values {
                update_canonical_json_digest(value, digest);
            }
        }
        serde_json::Value::Object(values) => {
            digest.update([5]);
            update_digest_length(values.len(), digest);
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for key in keys {
                update_digest_bytes(key.as_bytes(), digest);
                update_canonical_json_digest(
                    values
                        .get(key)
                        .expect("canonical config key came from the same object"),
                    digest,
                );
            }
        }
    }
}

fn update_digest_bytes(bytes: &[u8], digest: &mut Sha256) {
    update_digest_length(bytes.len(), digest);
    digest.update(bytes);
}

fn update_digest_length(length: usize, digest: &mut Sha256) {
    digest.update(u64::try_from(length).unwrap_or(u64::MAX).to_be_bytes());
}

fn required_resource<'a>(
    resources: &'a [ResourceDescriptorV1],
    kind: ResourceKindV1,
    name: &'static str,
) -> Result<&'a ResourceDescriptorV1, RuntimeError> {
    optional_resource(resources, kind, name)?.ok_or_else(|| {
        RuntimeError::Config(format!(
            "executable transfer is missing its {name} resource"
        ))
    })
}

fn optional_resource<'a>(
    resources: &'a [ResourceDescriptorV1],
    kind: ResourceKindV1,
    name: &'static str,
) -> Result<Option<&'a ResourceDescriptorV1>, RuntimeError> {
    let mut matches = resources
        .iter()
        .filter(|resource| resource.kind == kind as i32);
    let resource = matches.next();
    if matches.next().is_some() {
        return Err(RuntimeError::Config(format!(
            "executable transfer contains more than one {name} resource"
        )));
    }
    Ok(resource)
}

fn resource_bytes<'a>(
    arena: &'a SharedTransferArenaReader,
    resource: &ResourceDescriptorV1,
) -> Result<&'a [u8], RuntimeError> {
    let offset = usize::try_from(resource.arena_offset).map_err(|_| {
        RuntimeError::Config(format!(
            "resource {} offset does not fit the child address space",
            resource.resource_id
        ))
    })?;
    let length = usize::try_from(resource.arena_length).map_err(|_| {
        RuntimeError::Config(format!(
            "resource {} length does not fit the child address space",
            resource.resource_id
        ))
    })?;
    arena
        .region(offset, length)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

fn combine_abort_errors(
    cause: RuntimeError,
    rollback: Result<(), RuntimeError>,
    ingress: Result<(), RuntimeError>,
) -> RuntimeError {
    match (rollback, ingress) {
        (Ok(()), Ok(())) => cause,
        (rollback, ingress) => RuntimeError::Config(format!(
            "{cause}; process-transfer rollback: {}; listener ingress resume: {}",
            result_summary(rollback),
            result_summary(ingress)
        )),
    }
}

fn combine_resume_results(
    rollback: Result<(), RuntimeError>,
    ingress: Result<(), RuntimeError>,
) -> Result<(), RuntimeError> {
    match (rollback, ingress) {
        (Ok(()), Ok(())) => Ok(()),
        (rollback, ingress) => Err(RuntimeError::Config(format!(
            "process-transfer rollback: {}; listener ingress resume: {}",
            result_summary(rollback),
            result_summary(ingress)
        ))),
    }
}

fn result_summary(result: Result<(), RuntimeError>) -> String {
    match result {
        Ok(()) => "ok".to_string(),
        Err(error) => error.to_string(),
    }
}

fn warn_executable_freeze(duration: Duration) {
    let freeze_us = duration_us(duration);
    if freeze_us > 100_000 {
        eprintln!(
            "executable upgrade freeze exceeded target: {freeze_us}us (target=100000us acceptance=250000us hard-limit=500000us)"
        );
    }
}

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn write_directory_to_arena(
    arena: &SharedTransferArena,
    directory: ExecutableSessionDirectory,
) -> Result<ExecutableDirectoryPrestage, RuntimeError> {
    let mut counter = BoundedEncodedLength::new(MAX_EXECUTABLE_DIRECTORY_BYTES);
    ciborium::ser::into_writer(&directory, &mut counter).map_err(|error| {
        if counter.exceeded {
            RuntimeError::BudgetExceeded {
                resource: "executable session directory bytes",
                requested: counter.length.saturating_add(1),
                limit: MAX_EXECUTABLE_DIRECTORY_BYTES,
            }
        } else {
            RuntimeError::Config(error.to_string())
        }
    })?;
    let mut reservation = arena.reserve(counter.length).map_err(map_arena_error)?;
    ciborium::ser::into_writer(&directory, &mut reservation)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let region = reservation.seal().map_err(map_arena_error)?;
    Ok(ExecutableDirectoryPrestage { directory, region })
}

struct BoundedEncodedLength {
    length: usize,
    limit: usize,
    exceeded: bool,
}

impl BoundedEncodedLength {
    const fn new(limit: usize) -> Self {
        Self {
            length: 0,
            limit,
            exceeded: false,
        }
    }
}

impl Write for BoundedEncodedLength {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let Some(next) = self.length.checked_add(bytes.len()) else {
            self.exceeded = true;
            return Err(std::io::Error::other("encoded length overflow"));
        };
        if next > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::other("encoded length exceeds policy"));
        }
        self.length = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn map_arena_error(error: ArenaError) -> RuntimeError {
    match error {
        ArenaError::CapacityExhausted {
            requested,
            remaining,
        } => RuntimeError::BudgetExceeded {
            resource: "executable transfer arena bytes",
            requested,
            limit: remaining,
        },
        ArenaError::CapacityLimitExceeded { capacity, limit } => RuntimeError::BudgetExceeded {
            resource: "executable transfer arena bytes",
            requested: usize::try_from(capacity).unwrap_or(usize::MAX),
            limit: usize::try_from(limit).unwrap_or(usize::MAX),
        },
        ArenaError::Mapping(error) => RuntimeError::Io(error),
        other => RuntimeError::Config(other.to_string()),
    }
}

impl ExecutableUpgradeStatusAuthority {
    fn update(&self, phase: RuntimeUpgradePhase) -> Result<(), RuntimeError> {
        self.runtime
            .authority
            .update_executable_upgrade(self.id, phase)
    }

    fn preserve_on_drop(&mut self) {
        self.clear_on_drop = false;
    }

    fn finish(&mut self) {
        self.runtime.authority.finish_executable_upgrade(self.id);
        self.clear_on_drop = false;
    }
}

impl Drop for ExecutableUpgradeStatusAuthority {
    fn drop(&mut self) {
        if self.clear_on_drop {
            self.runtime.authority.finish_executable_upgrade(self.id);
        }
    }
}
