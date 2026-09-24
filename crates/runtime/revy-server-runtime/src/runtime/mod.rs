mod admin;
mod authority;
mod bootstrap;
mod core_loop;
mod core_store;
mod cutover;
mod executable;
mod generation;
mod reload_coordinator;
mod selection;
mod session;
mod session_registry;
mod status;
mod supervisor;
#[cfg(test)]
mod tests;
mod topology_resources;

pub use self::admin::{
    AdminAuthError, AdminCommandError, AdminControlPlaneHandle, AdminSubject,
    RuntimeUpgradeCallback, RuntimeUpgradeFuture,
};
pub(crate) use self::authority::RuntimeEpoch;
use self::core_store::{
    CoreCandidatePlan, CoreCommandOutcome, CoreDeltaSeal, CoreInvocation, CoreProcessDeltaSeal,
    CoreProcessPrecopy, CoreStore, PreparedGameplayTick, SharedCoreEvent,
};
pub use self::executable::{
    ExecutableBedrockSessionTransport, ExecutableBedrockTransportSnapshot,
    ExecutableChildActivated, ExecutableChildCommit, ExecutableChildNativeSockets,
    ExecutableChildNetworkPrepared, ExecutableChildPrepared, ExecutableChildRuntimePrepared,
    ExecutableChildSessionsPrepared, ExecutableChildSocketResources, ExecutableCorePrestage,
    ExecutableDirectoryPrestage, ExecutableFrozenNetwork, ExecutableLoginSession,
    ExecutableMinecraftCipherSnapshot, ExecutableNetworkPrepared, ExecutableNetworkPrestage,
    ExecutableParentCommitHold, ExecutablePluginArtifact, ExecutableQueuedSessionMessage,
    ExecutableSealedCoreDelta, ExecutableSessionActorSnapshot, ExecutableSessionBinding,
    ExecutableSessionDirectory, ExecutableSessionPhase, ExecutableSessionState,
    ExecutableSessionStateResource, ExecutableSessionTransportResource,
    ExecutableTcpSessionTransport, ExecutableTcpTransportSnapshot, ExecutableUpgradeFrozen,
    ExecutableUpgradeOutcomePending, ExecutableUpgradePreparing, ExecutableUpgradeStaged,
};
pub use self::generation::ExecutableListenerResource;
pub(crate) use self::generation::{
    AcceptedGenerationSession, ActiveGeneration, DrainingGeneration, GenerationAdmission,
    GenerationId, ListenerIngressCommand, QueuedAcceptGuard, QueuedAcceptTracker,
    TopologyListenerWorker, TopologyResourceState, now_ms,
};
use self::reload_coordinator::ReloadCoordinator;
pub(crate) use self::session::*;
use self::session_registry::SessionRegistry;
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
use self::topology_resources::TopologyResources;
pub use crate::{
    AdminArtifactsReloadView, AdminCoreReloadView, AdminFullReloadView, AdminGenerationCountView,
    AdminListenerBindingView, AdminNamedCountView, AdminPermission, AdminPhaseCountView,
    AdminPluginHostView, AdminRequest, AdminResponse, AdminRuntimeReloadDetail,
    AdminRuntimeReloadView, AdminSessionSummaryView, AdminSessionTransportCountView,
    AdminSessionView, AdminSessionsView, AdminStatusView, AdminTopologyReloadView,
    AdminUpgradeRuntimeView, CutoverConnectionMix, CutoverOperation, CutoverOutcome, CutoverReport,
    ListenerBinding, PluginFailureAction, PluginFailureMatrix, PluginHostStatusSnapshot,
    RuntimeReloadMode, RuntimeUpgradePhase, RuntimeUpgradeRole, RuntimeUpgradeStateView,
};
pub(crate) const LOGIN_SERVER_ID: &str = "";
pub(crate) const LOGIN_VERIFY_TOKEN_LEN: usize = 4;
pub(crate) const ACCEPT_QUEUE_CAPACITY: usize = 256;
pub(crate) const SESSION_OUTBOUND_QUEUE_CAPACITY: usize = 256;

pub(crate) struct RuntimeServer {
    pub(crate) reload: ReloadCoordinator,
    pub(crate) authority: authority::RuntimeAuthority,
    pub(crate) topology_resources: TopologyResources,
    pub(crate) sessions: SessionRegistry,
}
