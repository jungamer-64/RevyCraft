use crate::RuntimeError;
use crate::runtime::{
    ActiveGeneration, GenerationId, LOGIN_VERIFY_TOKEN_LEN, RuntimeUpgradeSessionHandle,
};
use mc_plugin_host::runtime::{AuthGenerationHandle, GameplayProfileHandle};
use mc_proto_common::{ConnectionPhase, ProtocolAdapter, TransportKind};
use revy_voxel_core::{
    ConnectionId, CoreEvent, EntityId, GameplayProfileId, PlayerId, PluginGenerationId,
    SessionCapabilitySet,
};
use std::sync::Arc;
use tokio::sync::{RwLock as AsyncRwLock, mpsc, oneshot};

#[derive(Clone)]
pub(crate) struct SessionHandle {
    pub(crate) tx: mpsc::Sender<SessionMessage>,
    pub(crate) control_tx: mpsc::Sender<SessionControl>,
    pub(crate) shared_state: SharedSessionState,
}

#[derive(Clone)]
pub(crate) struct SessionRecipient {
    pub(crate) tx: mpsc::Sender<SessionMessage>,
    pub(crate) control_tx: mpsc::Sender<SessionControl>,
}

pub(crate) type SharedSessionState = Arc<AsyncRwLock<SessionState>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionView {
    pub(crate) generation_id: GenerationId,
    pub(crate) transport: TransportKind,
    pub(crate) phase: ConnectionPhase,
    pub(crate) adapter_id: Option<String>,
    pub(crate) player_id: Option<PlayerId>,
    pub(crate) entity_id: Option<EntityId>,
    pub(crate) gameplay_profile: Option<GameplayProfileId>,
    pub(crate) protocol_generation: Option<PluginGenerationId>,
    pub(crate) gameplay_generation: Option<PluginGenerationId>,
}

#[derive(Clone)]
pub(crate) struct SessionRuntimeContext {
    pub(crate) player_id: Option<PlayerId>,
    pub(crate) gameplay: Option<Arc<dyn GameplayProfileHandle>>,
    pub(crate) session_capabilities: Option<SessionCapabilitySet>,
}

#[derive(Clone, Debug)]
pub(crate) enum SessionMessage {
    Event(Arc<CoreEvent>),
    Terminate { reason: String },
}

#[derive(Clone)]
pub(crate) struct SessionReattachInstruction {
    pub(crate) generation: Arc<ActiveGeneration>,
    pub(crate) adapter: Option<Arc<dyn ProtocolAdapter>>,
    pub(crate) gameplay: Option<Arc<dyn GameplayProfileHandle>>,
    pub(crate) phase: ConnectionPhase,
    pub(crate) player_id: Option<PlayerId>,
    pub(crate) entity_id: Option<EntityId>,
    pub(crate) resync_events: Vec<Arc<CoreEvent>>,
}

pub(crate) enum SessionControl {
    Terminate {
        reason: String,
    },
    FreezeForUpgrade {
        ack_tx: oneshot::Sender<Result<(), RuntimeError>>,
    },
    ResumeAfterUpgradeRollback {
        ack_tx: oneshot::Sender<Result<(), RuntimeError>>,
    },
    Reattach {
        instruction: SessionReattachInstruction,
        ack_tx: oneshot::Sender<Result<(), RuntimeError>>,
    },
    Export {
        ack_tx: oneshot::Sender<Result<RuntimeUpgradeSessionHandle, RuntimeError>>,
    },
}

#[derive(Clone)]
pub(crate) struct SessionReattachRecord {
    pub(crate) connection_id: ConnectionId,
    pub(crate) control_tx: mpsc::Sender<SessionControl>,
    pub(crate) transport: TransportKind,
    pub(crate) phase: ConnectionPhase,
    pub(crate) adapter_id: Option<String>,
    pub(crate) player_id: Option<PlayerId>,
    pub(crate) entity_id: Option<EntityId>,
    pub(crate) gameplay_profile: Option<GameplayProfileId>,
    pub(crate) protocol_generation: Option<PluginGenerationId>,
    pub(crate) gameplay_generation: Option<PluginGenerationId>,
}

pub(crate) struct SessionState {
    pub(crate) generation: Arc<ActiveGeneration>,
    pub(crate) transport: TransportKind,
    pub(crate) phase: ConnectionPhase,
    pub(crate) adapter: Option<Arc<dyn ProtocolAdapter>>,
    pub(crate) gameplay: Option<Arc<dyn GameplayProfileHandle>>,
    pub(crate) login_challenge: Option<LoginChallengeState>,
    pub(crate) player_id: Option<PlayerId>,
    pub(crate) entity_id: Option<EntityId>,
    pub(crate) session_capabilities: Option<SessionCapabilitySet>,
}

#[derive(Clone)]
pub(crate) struct LoginChallengeState {
    pub(crate) username: String,
    pub(crate) verify_token: [u8; LOGIN_VERIFY_TOKEN_LEN],
    pub(crate) auth_generation: Arc<dyn AuthGenerationHandle>,
}
