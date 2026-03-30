mod admin;
mod bootstrap;
mod core_loop;
mod generation;
mod kernel;
mod reload_coordinator;
mod selection;
mod session;
mod session_registry;
mod status;
mod supervisor;
#[cfg(test)]
mod test_hooks;
#[cfg(test)]
mod tests;
mod topology_manager;
mod upgrade;

use self::kernel::{KernelCommandOutcome, RuntimeKernel};
use self::reload_coordinator::ReloadCoordinator;
use self::selection::SelectionManager;
use self::session_registry::SessionRegistry;
use self::topology_manager::TopologyManager;
pub use crate::{
    AdminArtifactsReloadView, AdminCoreReloadView, AdminFullReloadView, AdminGenerationCountView,
    AdminListenerBindingView, AdminNamedCountView, AdminPermission, AdminPhaseCountView,
    AdminPluginHostView, AdminRequest, AdminResponse, AdminRuntimeReloadDetail,
    AdminRuntimeReloadView, AdminSessionSummaryView, AdminSessionTransportCountView,
    AdminSessionView, AdminSessionsView, AdminStatusView, AdminTopologyReloadView,
    AdminUpgradeRuntimeView, ListenerBinding, PluginFailureAction, PluginFailureMatrix,
    PluginHostStatusSnapshot, RuntimeReloadMode, RuntimeUpgradePhase, RuntimeUpgradeRole,
    RuntimeUpgradeStateView,
};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
#[cfg(test)]
use tokio::sync::Mutex as AsyncMutex;

pub use self::admin::{
    AdminAuthError, AdminCommandError, AdminControlPlaneHandle, AdminSubject,
    RuntimeUpgradeCallback, RuntimeUpgradeFuture,
};
pub(crate) use self::generation::{
    AcceptedGenerationSession, ActiveGeneration, DrainingGeneration, GenerationAdmission,
    GenerationId, ListenerWorkerControl, QueuedAcceptGuard, QueuedAcceptTracker,
    RuntimeGenerationState, TopologyListenerWorker, now_ms,
};
pub(crate) use self::session::{
    LoginChallengeState, OnlineAuthKeys, SessionControl, SessionHandle, SessionMessage,
    SessionReattachInstruction, SessionReattachRecord, SessionRecipient, SessionRuntimeContext,
    SessionState, SessionView, SharedSessionState,
};
pub use self::status::{
    GenerationCountSnapshot, GenerationStatusSnapshot, GenerationStatusState,
    OptionalNamedCountSnapshot, PhaseCountSnapshot, RuntimeStatusSnapshot, SessionStatusSnapshot,
    SessionSummarySnapshot, TransportCountSnapshot, format_runtime_status_summary,
};
pub(crate) use self::supervisor::RunningServer;
pub use self::supervisor::{
    AdminSurfaceSelection, ArtifactsReloadResult, CoreReloadResult, FullReloadResult,
    RuntimeReloadResult, ServerSupervisor, TopologyReloadResult,
};
#[cfg(test)]
use self::test_hooks::{LoginAcceptCommitPauseHook, ReloadStagePauseHook};
pub use self::upgrade::{
    RuntimeUpgradeCommitHold, RuntimeUpgradeGuard, RuntimeUpgradeImport,
    RuntimeUpgradeLoginChallenge, RuntimeUpgradePayload, RuntimeUpgradeQueuedMessage,
    RuntimeUpgradeSessionHandle, RuntimeUpgradeSessionState,
};

pub(crate) const LOGIN_SERVER_ID: &str = "";
pub(crate) const LOGIN_VERIFY_TOKEN_LEN: usize = 4;
pub(crate) const ACCEPT_QUEUE_CAPACITY: usize = 256;
pub(crate) const SESSION_OUTBOUND_QUEUE_CAPACITY: usize = 256;

pub(crate) struct RuntimeServer {
    pub(crate) reload: ReloadCoordinator,
    pub(crate) selection: SelectionManager,
    pub(crate) topology: TopologyManager,
    pub(crate) kernel: RuntimeKernel,
    pub(crate) sessions: SessionRegistry,
    #[cfg(test)]
    pub(crate) fail_nth_reattach_send: AtomicUsize,
    #[cfg(test)]
    reload_stage_pause_hook: AsyncMutex<Option<ReloadStagePauseHook>>,
    #[cfg(test)]
    login_accept_commit_pause_hook: AsyncMutex<Option<LoginAcceptCommitPauseHook>>,
}
