use crate::RuntimeError;
use crate::runtime::executable::FrozenProcessSessionState;
use crate::runtime::{ActiveGeneration, GenerationId, LOGIN_VERIFY_TOKEN_LEN, RuntimeEpoch};
use crate::transport::PreparedProcessTransportSocket;
use mc_plugin_host::runtime::{AuthGenerationHandle, GameplayProfileHandle, ProtocolReloadSession};
use mc_proto_common::{ConnectionPhase, ProtocolAdapter, ProtocolSessionSnapshot, TransportKind};
use revy_runtime_transfer::{SharedTransferArena, SocketTransferTarget};
use revy_voxel_core::{
    ConnectionId, CoreEvent, EntityId, GameplayProfileId, PlayerId, PluginGenerationId,
    SessionCapabilitySet,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub(crate) struct SessionHandle {
    pub(crate) tx: mpsc::Sender<SessionMessage>,
    pub(crate) control_tx: mpsc::Sender<SessionControl>,
    pub(crate) directory: Arc<SessionDirectoryProjection>,
}

#[derive(Clone)]
pub(crate) struct SessionRecipient {
    pub(crate) connection_id: ConnectionId,
    pub(crate) tx: mpsc::Sender<SessionMessage>,
    pub(crate) control_tx: mpsc::Sender<SessionControl>,
}

#[derive(Clone, Debug)]
pub(crate) enum SessionMessage {
    Events(Vec<Arc<CoreEvent>>),
    Terminate { reason: String },
}

pub(crate) enum SessionControl {
    Terminate {
        reason: String,
    },
    /// A coalescible housekeeping notification, not authority to disconnect the
    /// generation observed by the registry. The actor rechecks after epoch activation.
    EnforceGenerationDrain,
    PrepareCutover {
        candidate: Arc<RuntimeEpoch>,
        resync_events: Vec<Arc<CoreEvent>>,
        force_resync: bool,
        ack_tx: oneshot::Sender<Result<(), RuntimeError>>,
    },
    SealCutover {
        epoch_revision: u64,
        final_events: Vec<Arc<CoreEvent>>,
        ack_tx: oneshot::Sender<Result<(), RuntimeError>>,
    },
    AbortCutover {
        epoch_revision: u64,
        ack_tx: oneshot::Sender<Result<(), RuntimeError>>,
    },
    PrepareProcessTransferSocket {
        target: SocketTransferTarget,
        arena: Arc<SharedTransferArena>,
        ack_tx: oneshot::Sender<Result<PreparedProcessTransportSocket, RuntimeError>>,
    },
    FreezeForProcessTransfer {
        ack_tx: oneshot::Sender<Result<FrozenProcessSessionState, RuntimeError>>,
    },
    RollbackProcessTransfer {
        ack_tx: oneshot::Sender<Result<(), RuntimeError>>,
    },
    CommitProcessTransfer {
        ack_tx: oneshot::Sender<Result<(), RuntimeError>>,
    },
}

#[derive(Clone)]
pub(crate) struct BoundSession {
    pub(crate) generation: Arc<ActiveGeneration>,
    pub(crate) transport: TransportKind,
    pub(crate) adapter: Arc<dyn ProtocolAdapter>,
    pub(crate) gameplay: Arc<dyn GameplayProfileHandle>,
    pub(crate) capabilities: SessionCapabilitySet,
}

impl BoundSession {
    pub(crate) fn new(
        generation: Arc<ActiveGeneration>,
        transport: TransportKind,
        adapter: Arc<dyn ProtocolAdapter>,
        gameplay: Arc<dyn GameplayProfileHandle>,
        entity_id: Option<EntityId>,
    ) -> Self {
        let capabilities = SessionCapabilitySet {
            protocol: adapter.capability_set(),
            gameplay: gameplay.capability_set(),
            gameplay_profile: gameplay.profile_id(),
            entity_id,
            protocol_generation: adapter.plugin_generation_id(),
            gameplay_generation: gameplay.plugin_generation_id(),
        };
        Self {
            generation,
            transport,
            adapter,
            gameplay,
            capabilities,
        }
    }

    pub(crate) fn with_entity_id(&self, entity_id: EntityId) -> Self {
        Self::new(
            Arc::clone(&self.generation),
            self.transport,
            Arc::clone(&self.adapter),
            Arc::clone(&self.gameplay),
            Some(entity_id),
        )
    }
}

#[derive(Clone)]
pub(crate) struct LoginChallenge {
    pub(crate) username: String,
    pub(crate) verify_token: [u8; LOGIN_VERIFY_TOKEN_LEN],
    pub(crate) auth_generation: Arc<dyn AuthGenerationHandle>,
}

#[derive(Clone)]
pub(crate) enum LoginSession {
    Negotiating(BoundSession),
    Authenticating {
        binding: BoundSession,
        challenge: LoginChallenge,
    },
    AcceptedWritePending {
        binding: BoundSession,
        player_id: PlayerId,
        entity_id: EntityId,
    },
}

impl LoginSession {
    pub(crate) fn binding(&self) -> &BoundSession {
        match self {
            Self::Negotiating(binding)
            | Self::Authenticating { binding, .. }
            | Self::AcceptedWritePending { binding, .. } => binding,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PlaySession {
    pub(crate) binding: BoundSession,
    pub(crate) player_id: PlayerId,
    pub(crate) entity_id: EntityId,
}

#[derive(Clone)]
pub(crate) enum SessionPhase {
    Handshaking {
        generation: Arc<ActiveGeneration>,
    },
    Status(BoundSession),
    Login(LoginSession),
    Play(PlaySession),
    Closing {
        generation_id: GenerationId,
        transport: TransportKind,
        phase: ConnectionPhase,
    },
}

impl SessionPhase {
    pub(crate) fn phase(&self) -> ConnectionPhase {
        match self {
            Self::Handshaking { .. } => ConnectionPhase::Handshaking,
            Self::Status(_) => ConnectionPhase::Status,
            Self::Login(_) => ConnectionPhase::Login,
            Self::Play(_) => ConnectionPhase::Play,
            Self::Closing { phase, .. } => *phase,
        }
    }

    pub(crate) fn generation_id(&self) -> GenerationId {
        match self {
            Self::Handshaking { generation } => generation.generation_id,
            Self::Status(binding) => binding.generation.generation_id,
            Self::Login(login) => login.binding().generation.generation_id,
            Self::Play(play) => play.binding.generation.generation_id,
            Self::Closing { generation_id, .. } => *generation_id,
        }
    }

    pub(crate) fn transport(&self) -> TransportKind {
        match self {
            Self::Handshaking { .. } => TransportKind::Tcp,
            Self::Status(binding) => binding.transport,
            Self::Login(login) => login.binding().transport,
            Self::Play(play) => play.binding.transport,
            Self::Closing { transport, .. } => *transport,
        }
    }

    pub(crate) fn binding(&self) -> Option<&BoundSession> {
        match self {
            Self::Status(binding) => Some(binding),
            Self::Login(login) => Some(login.binding()),
            Self::Play(play) => Some(&play.binding),
            Self::Handshaking { .. } | Self::Closing { .. } => None,
        }
    }

    pub(crate) fn command_context(&self) -> Option<SessionCommandContext> {
        match self {
            Self::Login(login) => Some(SessionCommandContext::Login {
                gameplay: Arc::clone(&login.binding().gameplay),
                capabilities: login.binding().capabilities.clone(),
            }),
            Self::Play(play) => Some(SessionCommandContext::Play {
                player_id: play.player_id,
                gameplay: Arc::clone(&play.binding.gameplay),
                capabilities: play.binding.capabilities.clone(),
            }),
            Self::Handshaking { .. } | Self::Status(_) | Self::Closing { .. } => None,
        }
    }

    pub(crate) fn protocol_snapshot(&self, connection_id: ConnectionId) -> ProtocolSessionSnapshot {
        let (player_id, entity_id) = match self {
            Self::Login(LoginSession::AcceptedWritePending {
                player_id,
                entity_id,
                ..
            })
            | Self::Play(PlaySession {
                player_id,
                entity_id,
                ..
            }) => (Some(*player_id), Some(*entity_id)),
            Self::Handshaking { .. } | Self::Status(_) | Self::Login(_) | Self::Closing { .. } => {
                (None, None)
            }
        };
        ProtocolSessionSnapshot {
            connection_id,
            phase: self.phase(),
            player_id,
            entity_id,
        }
    }

    pub(crate) fn gameplay_snapshot(
        &self,
    ) -> Option<mc_plugin_contract::codec::gameplay::GameplaySessionSnapshot> {
        let (binding, player_id, entity_id) = match self {
            Self::Login(LoginSession::AcceptedWritePending {
                binding,
                player_id,
                entity_id,
            }) => (binding, *player_id, *entity_id),
            Self::Play(play) => (&play.binding, play.player_id, play.entity_id),
            _ => return None,
        };
        Some(
            mc_plugin_contract::codec::gameplay::GameplaySessionSnapshot {
                phase: self.phase(),
                player_id: Some(player_id),
                entity_id: Some(entity_id),
                protocol: binding.capabilities.protocol.clone(),
                gameplay_profile: binding.capabilities.gameplay_profile.clone(),
                protocol_generation: binding.capabilities.protocol_generation,
                gameplay_generation: binding.capabilities.gameplay_generation,
            },
        )
    }

    pub(crate) fn directory_entry(&self) -> SessionDirectoryEntry {
        let phase = match self {
            Self::Handshaking { .. } => SessionDirectoryPhase::Handshaking,
            Self::Status(binding) => SessionDirectoryPhase::Status {
                adapter_id: binding.adapter.descriptor().adapter_id,
            },
            Self::Login(login) => SessionDirectoryPhase::Login {
                adapter_id: login.binding().adapter.descriptor().adapter_id,
            },
            Self::Play(play) => SessionDirectoryPhase::Play {
                adapter_id: play.binding.adapter.descriptor().adapter_id,
                player_id: play.player_id,
                entity_id: play.entity_id,
                gameplay: Arc::clone(&play.binding.gameplay),
                capabilities: play.binding.capabilities.clone(),
            },
            Self::Closing { phase, .. } => SessionDirectoryPhase::Closing { phase: *phase },
        };
        SessionDirectoryEntry {
            generation_id: self.generation_id(),
            transport: self.transport(),
            phase,
        }
    }
}

pub(crate) enum SessionLifecycle {
    Running {
        epoch_revision: u64,
        phase: SessionPhase,
    },
    CutoverPrepared {
        active_epoch_revision: u64,
        active: SessionPhase,
        pending: PendingSessionState,
    },
    TransferFrozen {
        active_epoch_revision: u64,
        active: SessionPhase,
        pending: PendingSessionState,
    },
    Transferred,
}

pub(crate) struct PendingSessionState {
    pub(crate) epoch_revision: u64,
    pub(crate) phase: SessionPhase,
    pub(crate) resync_frames: Vec<Vec<u8>>,
}

#[derive(Clone)]
pub(crate) enum SessionCommandContext {
    Login {
        gameplay: Arc<dyn GameplayProfileHandle>,
        capabilities: SessionCapabilitySet,
    },
    Play {
        player_id: PlayerId,
        gameplay: Arc<dyn GameplayProfileHandle>,
        capabilities: SessionCapabilitySet,
    },
}

#[derive(Clone)]
pub(crate) struct SessionDirectoryEntry {
    pub(crate) generation_id: GenerationId,
    pub(crate) transport: TransportKind,
    pub(crate) phase: SessionDirectoryPhase,
}

pub(crate) struct SessionDirectoryProjection {
    connection_id: ConnectionId,
    entry: RwLock<SessionDirectoryEntry>,
    revision: Arc<AtomicU64>,
    players: Arc<SessionPlayerProjection>,
}

pub(crate) struct SessionPlayerProjection {
    connections: RwLock<HashMap<PlayerId, ConnectionId>>,
}

impl SessionPlayerProjection {
    pub(crate) fn new() -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn connection(&self, player_id: PlayerId) -> Option<ConnectionId> {
        self.connections
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&player_id)
            .copied()
    }

    fn publish(
        &self,
        connection_id: ConnectionId,
        previous: Option<PlayerId>,
        next: Option<PlayerId>,
    ) {
        if previous == next {
            return;
        }
        let mut connections = self
            .connections
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(previous) = previous
            && connections.get(&previous) == Some(&connection_id)
        {
            connections.remove(&previous);
        }
        if let Some(next) = next {
            connections.insert(next, connection_id);
        }
    }
}

impl SessionDirectoryProjection {
    pub(crate) fn new(
        connection_id: ConnectionId,
        entry: SessionDirectoryEntry,
        revision: Arc<AtomicU64>,
        players: Arc<SessionPlayerProjection>,
    ) -> Self {
        Self {
            connection_id,
            entry: RwLock::new(entry),
            revision,
            players,
        }
    }

    pub(crate) fn read(&self) -> SessionDirectoryEntry {
        self.entry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn publish(&self, entry: SessionDirectoryEntry) {
        let mut active = self
            .entry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active.same_projection(&entry) {
            return;
        }
        self.players
            .publish(self.connection_id, active.player_id(), entry.player_id());
        *active = entry;
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn attach(&self) {
        let player_id = self
            .entry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .player_id();
        self.players.publish(self.connection_id, None, player_id);
    }

    pub(crate) fn detach(&self) {
        let player_id = self
            .entry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .player_id();
        self.players.publish(self.connection_id, player_id, None);
    }
}

#[derive(Clone)]
pub(crate) enum SessionDirectoryPhase {
    Handshaking,
    Status {
        adapter_id: String,
    },
    Login {
        adapter_id: String,
    },
    Play {
        adapter_id: String,
        player_id: PlayerId,
        entity_id: EntityId,
        gameplay: Arc<dyn GameplayProfileHandle>,
        capabilities: SessionCapabilitySet,
    },
    Closing {
        phase: ConnectionPhase,
    },
}

impl SessionDirectoryEntry {
    fn same_projection(&self, other: &Self) -> bool {
        self.generation_id == other.generation_id
            && self.transport == other.transport
            && self.phase.same_projection(&other.phase)
    }

    pub(crate) fn phase(&self) -> ConnectionPhase {
        match &self.phase {
            SessionDirectoryPhase::Handshaking => ConnectionPhase::Handshaking,
            SessionDirectoryPhase::Status { .. } => ConnectionPhase::Status,
            SessionDirectoryPhase::Login { .. } => ConnectionPhase::Login,
            SessionDirectoryPhase::Play { .. } => ConnectionPhase::Play,
            SessionDirectoryPhase::Closing { phase } => *phase,
        }
    }

    pub(crate) fn player_id(&self) -> Option<PlayerId> {
        match self.phase {
            SessionDirectoryPhase::Play { player_id, .. } => Some(player_id),
            _ => None,
        }
    }

    pub(crate) fn protocol_reload_session(
        &self,
        connection_id: ConnectionId,
    ) -> Option<ProtocolReloadSession> {
        let adapter_id = match &self.phase {
            SessionDirectoryPhase::Status { adapter_id }
            | SessionDirectoryPhase::Login { adapter_id }
            | SessionDirectoryPhase::Play { adapter_id, .. } => adapter_id.clone(),
            SessionDirectoryPhase::Handshaking | SessionDirectoryPhase::Closing { .. } => {
                return None;
            }
        };
        let (player_id, entity_id) = match self.phase {
            SessionDirectoryPhase::Play {
                player_id,
                entity_id,
                ..
            } => (Some(player_id), Some(entity_id)),
            _ => (None, None),
        };
        Some(ProtocolReloadSession {
            adapter_id,
            session: ProtocolSessionSnapshot {
                connection_id,
                phase: self.phase(),
                player_id,
                entity_id,
            },
        })
    }

    pub(crate) fn adapter_id(&self) -> Option<&str> {
        match &self.phase {
            SessionDirectoryPhase::Status { adapter_id }
            | SessionDirectoryPhase::Login { adapter_id }
            | SessionDirectoryPhase::Play { adapter_id, .. } => Some(adapter_id),
            SessionDirectoryPhase::Handshaking | SessionDirectoryPhase::Closing { .. } => None,
        }
    }

    pub(crate) fn gameplay_profile(&self) -> Option<GameplayProfileId> {
        match &self.phase {
            SessionDirectoryPhase::Play { capabilities, .. } => {
                Some(capabilities.gameplay_profile.clone())
            }
            _ => None,
        }
    }

    pub(crate) fn protocol_generation(&self) -> Option<PluginGenerationId> {
        match &self.phase {
            SessionDirectoryPhase::Play { capabilities, .. } => capabilities.protocol_generation,
            _ => None,
        }
    }

    pub(crate) fn gameplay_generation(&self) -> Option<PluginGenerationId> {
        match &self.phase {
            SessionDirectoryPhase::Play { capabilities, .. } => capabilities.gameplay_generation,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SessionPlayerProjection;
    use revy_voxel_core::{ConnectionId, PlayerId};
    use uuid::Uuid;

    #[test]
    fn stale_session_detach_does_not_remove_a_newer_player_projection() {
        let players = SessionPlayerProjection::new();
        let player_id = PlayerId(Uuid::from_u128(7));
        let previous_connection = ConnectionId(11);
        let current_connection = ConnectionId(12);

        players.publish(previous_connection, None, Some(player_id));
        players.publish(current_connection, None, Some(player_id));
        players.publish(previous_connection, Some(player_id), None);

        assert_eq!(players.connection(player_id), Some(current_connection));
        players.publish(current_connection, Some(player_id), None);
        assert_eq!(players.connection(player_id), None);
    }
}

impl SessionDirectoryPhase {
    fn same_projection(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Handshaking, Self::Handshaking) => true,
            (Self::Status { adapter_id: left }, Self::Status { adapter_id: right })
            | (Self::Login { adapter_id: left }, Self::Login { adapter_id: right }) => {
                left == right
            }
            (
                Self::Play {
                    adapter_id: left_adapter,
                    player_id: left_player,
                    entity_id: left_entity,
                    gameplay: left_gameplay,
                    capabilities: left_capabilities,
                },
                Self::Play {
                    adapter_id: right_adapter,
                    player_id: right_player,
                    entity_id: right_entity,
                    gameplay: right_gameplay,
                    capabilities: right_capabilities,
                },
            ) => {
                left_adapter == right_adapter
                    && left_player == right_player
                    && left_entity == right_entity
                    && Arc::ptr_eq(left_gameplay, right_gameplay)
                    && left_capabilities == right_capabilities
            }
            (Self::Closing { phase: left }, Self::Closing { phase: right }) => left == right,
            _ => false,
        }
    }
}
