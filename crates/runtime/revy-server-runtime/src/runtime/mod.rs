mod admin;
mod bootstrap;
mod core_loop;
mod kernel;
mod reload_coordinator;
mod selection;
mod session;
mod session_registry;
mod status;
#[cfg(test)]
mod tests;
mod topology_manager;
mod upgrade;

use self::kernel::{KernelCommandOutcome, RuntimeKernel};
use self::reload_coordinator::ReloadCoordinator;
use self::selection::{ResolvedRuntimeSelection, SelectionManager};
use self::session_registry::SessionRegistry;
use self::topology_manager::TopologyManager;
use crate::RuntimeError;
use crate::config::{ServerConfig, ServerConfigSource};
use crate::transport::AcceptedTransportSession;
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
use mc_plugin_host::registry::ProtocolRegistry;
use mc_plugin_host::runtime::{
    AdminSurfaceProfileHandle, AuthGenerationHandle, GameplayProfileHandle, RuntimeReloadContext,
};
use mc_proto_common::{ConnectionPhase, ProtocolAdapter, TransportKind};
use revy_voxel_core::{
    CoreEvent, EntityId, GameplayProfileId, PlayerId, PluginGenerationId, SessionCapabilitySet,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(test)]
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::{RwLock as AsyncRwLock, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

pub use self::admin::{
    AdminAuthError, AdminCommandError, AdminControlPlaneHandle, AdminSubject,
    RuntimeUpgradeCallback, RuntimeUpgradeFuture,
};
pub use self::status::{
    GenerationCountSnapshot, GenerationStatusSnapshot, GenerationStatusState,
    OptionalNamedCountSnapshot, PhaseCountSnapshot, RuntimeStatusSnapshot, SessionStatusSnapshot,
    SessionSummarySnapshot, TransportCountSnapshot, format_runtime_status_summary,
};
pub use self::upgrade::{
    RuntimeUpgradeCommitHold, RuntimeUpgradeGuard, RuntimeUpgradeImport,
    RuntimeUpgradeLoginChallenge, RuntimeUpgradePayload, RuntimeUpgradeQueuedMessage,
    RuntimeUpgradeSessionHandle, RuntimeUpgradeSessionState,
};

pub(crate) const LOGIN_SERVER_ID: &str = "";
pub(crate) const LOGIN_VERIFY_TOKEN_LEN: usize = 4;
pub(crate) const ACCEPT_QUEUE_CAPACITY: usize = 256;
pub(crate) const SESSION_OUTBOUND_QUEUE_CAPACITY: usize = 256;

pub struct ServerSupervisor {
    running: RunningServer,
}

#[derive(Clone)]
pub struct AdminSurfaceSelection {
    pub instance_id: String,
    pub surface_config_path: Option<PathBuf>,
    pub profile: Arc<dyn AdminSurfaceProfileHandle>,
}

impl ServerSupervisor {
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when config loading, plugin resolution, or runtime boot fails.
    pub async fn boot(config_source: ServerConfigSource) -> Result<Self, RuntimeError> {
        let config = config_source.load()?;
        let plugin_host =
            mc_plugin_host::host::plugin_host_from_config(&config.plugin_host_bootstrap_config())?
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "no packaged plugins discovered under `{}`",
                        config.bootstrap.plugins_dir.display()
                    ))
                })?;
        let loaded_plugins =
            plugin_host.load_plugin_set(&config.plugin_host_runtime_selection_config())?;
        let running =
            self::bootstrap::boot_server(config_source, config, loaded_plugins, Some(plugin_host))
                .await?;
        Ok(Self { running })
    }

    #[must_use]
    pub fn listener_bindings(&self) -> Vec<ListenerBinding> {
        self.running.listener_bindings()
    }

    #[must_use]
    pub fn admin_control_plane(&self) -> AdminControlPlaneHandle {
        self.running.admin_control_plane()
    }

    pub async fn current_admin_surfaces(&self) -> Vec<AdminSurfaceSelection> {
        self.running.current_admin_surfaces().await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the requested reload mode cannot be applied.
    pub async fn reload_runtime(
        &self,
        mode: RuntimeReloadMode,
    ) -> Result<RuntimeReloadResult, RuntimeError> {
        self.running.reload_runtime(mode).await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a loaded plugin cannot be reloaded successfully.
    pub async fn reload_runtime_artifacts(&self) -> Result<ArtifactsReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Artifacts).await? {
            RuntimeReloadResult::Artifacts(result) => Ok(result),
            RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Core(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("artifacts reload should only produce an artifacts-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot materialize a candidate topology.
    pub async fn reload_runtime_topology(&self) -> Result<TopologyReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Topology).await? {
            RuntimeReloadResult::Topology(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Core(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("topology reload should only produce a topology-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot reload the live core.
    pub async fn reload_runtime_core(&self) -> Result<CoreReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Core).await? {
            RuntimeReloadResult::Core(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("core reload should only produce a core-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot reconcile live config state or apply a
    /// candidate topology.
    pub async fn reload_runtime_full(&self) -> Result<FullReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Full).await? {
            RuntimeReloadResult::Full(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Core(_) => {
                unreachable!("full reload should only produce a full-scoped result")
            }
        }
    }

    #[must_use]
    pub async fn status(&self) -> RuntimeStatusSnapshot {
        self.running.status().await
    }

    #[must_use]
    pub async fn session_status(&self) -> Vec<SessionStatusSnapshot> {
        self.running.session_status().await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime completion watcher closes unexpectedly.
    pub async fn wait_for_runtime_completion(&self) -> Result<(), RuntimeError> {
        self.running.wait_for_runtime_completion().await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime task exits with an error or the join fails.
    pub async fn join_runtime(&self) -> Result<(), RuntimeError> {
        self.running.join_runtime().await
    }

    pub fn request_shutdown(&self) -> bool {
        self.running.request_shutdown()
    }

    pub fn clear_runtime_upgrade_state(&self) {
        self.running.clear_runtime_upgrade_state();
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the server task fails while shutting down.
    pub async fn shutdown(self) -> Result<(), RuntimeError> {
        self.running.shutdown().await
    }
}

pub(crate) struct RunningServer {
    pub(crate) runtime: Arc<RuntimeServer>,
    pub(crate) join_handle: tokio::sync::Mutex<Option<JoinHandle<Result<(), RuntimeError>>>>,
    pub(crate) runtime_completion_rx: watch::Receiver<bool>,
}

impl RunningServer {
    #[must_use]
    pub fn listener_bindings(&self) -> Vec<ListenerBinding> {
        self.runtime.listener_bindings()
    }

    #[must_use]
    pub fn admin_control_plane(&self) -> AdminControlPlaneHandle {
        AdminControlPlaneHandle::new(Arc::clone(&self.runtime))
    }

    pub async fn current_admin_surfaces(&self) -> Vec<AdminSurfaceSelection> {
        self.runtime.current_admin_surfaces().await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime completion watcher closes unexpectedly.
    pub async fn wait_for_runtime_completion(&self) -> Result<(), RuntimeError> {
        let mut completion_rx = self.runtime_completion_rx.clone();
        if *completion_rx.borrow() {
            return Ok(());
        }
        completion_rx.changed().await.map_err(|error| {
            RuntimeError::Config(format!(
                "runtime completion watcher closed unexpectedly: {error}"
            ))
        })?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the requested reload mode cannot be applied.
    pub async fn reload_runtime(
        &self,
        mode: RuntimeReloadMode,
    ) -> Result<RuntimeReloadResult, RuntimeError> {
        let reload_host = self.runtime.reload.reload_host().ok_or_else(|| {
            RuntimeError::Config(
                "reload is unavailable without a reload-capable supervisor boot".into(),
            )
        })?;
        self.runtime
            .reload_runtime(reload_host.as_ref(), mode)
            .await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a loaded plugin cannot be reloaded successfully.
    #[allow(dead_code)]
    pub async fn reload_runtime_artifacts(&self) -> Result<ArtifactsReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Artifacts).await? {
            RuntimeReloadResult::Artifacts(result) => Ok(result),
            RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Core(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("artifacts reload should only produce an artifacts-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot materialize a candidate topology.
    #[allow(dead_code)]
    pub async fn reload_runtime_topology(&self) -> Result<TopologyReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Topology).await? {
            RuntimeReloadResult::Topology(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Core(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("topology reload should only produce a topology-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot reload the live core.
    #[allow(dead_code)]
    pub async fn reload_runtime_core(&self) -> Result<CoreReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Core).await? {
            RuntimeReloadResult::Core(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("core reload should only produce a core-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot reconcile live config state or apply a
    /// candidate topology.
    #[allow(dead_code)]
    pub async fn reload_runtime_full(&self) -> Result<FullReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Full).await? {
            RuntimeReloadResult::Full(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Core(_) => {
                unreachable!("full reload should only produce a full-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the server task fails while shutting down.
    pub async fn shutdown(self) -> Result<(), RuntimeError> {
        let _ = self.runtime.request_shutdown();
        self.join_runtime().await
    }

    pub fn request_shutdown(&self) -> bool {
        self.runtime.request_shutdown()
    }

    pub fn clear_runtime_upgrade_state(&self) {
        self.runtime.clear_runtime_upgrade_state();
    }

    pub async fn join_runtime(&self) -> Result<(), RuntimeError> {
        let join_handle = self.join_handle.lock().await.take();
        match join_handle {
            Some(join_handle) => join_handle.await?,
            None => self.wait_for_runtime_completion().await,
        }
    }
}

impl RuntimeServer {
    pub(crate) async fn current_admin_surfaces(&self) -> Vec<AdminSurfaceSelection> {
        self.selection
            .current_admin_surfaces()
            .await
            .into_iter()
            .map(|selection| AdminSurfaceSelection {
                instance_id: selection.instance_id,
                surface_config_path: selection.surface_config_path,
                profile: selection.profile,
            })
            .collect()
    }

    pub(crate) async fn selection_state(&self) -> ResolvedRuntimeSelection {
        self.selection.current().await
    }

    pub(crate) async fn replace_active_config(&self, next_active_config: ServerConfig) {
        self.selection.replace_config(next_active_config).await;
    }

    #[cfg(test)]
    pub(crate) async fn arm_reload_stage_pause_for_test(&self) -> ReloadStagePauseHandle {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self.reload_stage_pause_hook.lock().await = Some(ReloadStagePauseHook {
            reached_tx: Some(reached_tx),
            release_rx,
        });
        ReloadStagePauseHandle {
            reached_rx,
            release_tx: Some(release_tx),
        }
    }

    #[cfg(test)]
    pub(crate) async fn maybe_pause_after_reload_stage_for_test(&self) {
        let hook = self.reload_stage_pause_hook.lock().await.take();
        let Some(mut hook) = hook else {
            return;
        };
        if let Some(reached_tx) = hook.reached_tx.take() {
            let _ = reached_tx.send(());
        }
        let _ = hook.release_rx.await;
    }

    #[cfg(test)]
    pub(crate) async fn arm_login_accept_commit_pause_for_test(
        &self,
    ) -> LoginAcceptCommitPauseHandle {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self.login_accept_commit_pause_hook.lock().await = Some(LoginAcceptCommitPauseHook {
            reached_tx: Some(reached_tx),
            release_rx,
        });
        LoginAcceptCommitPauseHandle {
            reached_rx,
            release_tx: Some(release_tx),
        }
    }

    #[cfg(test)]
    pub(crate) async fn maybe_pause_before_login_accept_commit_for_test(&self) {
        let hook = self.login_accept_commit_pause_hook.lock().await.take();
        let Some(mut hook) = hook else {
            return;
        };
        if let Some(reached_tx) = hook.reached_tx.take() {
            let _ = reached_tx.send(());
        }
        let _ = hook.release_rx.await;
    }
}

#[cfg(test)]
impl ReloadStagePauseHandle {
    pub(crate) async fn wait_until_reached(&mut self) {
        let _ = (&mut self.reached_rx).await;
    }

    pub(crate) fn release(mut self) {
        if let Some(release_tx) = self.release_tx.take() {
            let _ = release_tx.send(());
        }
    }
}

#[cfg(test)]
impl LoginAcceptCommitPauseHandle {
    pub(crate) async fn wait_until_reached(&mut self) {
        let _ = (&mut self.reached_rx).await;
    }

    pub(crate) fn release(mut self) {
        if let Some(release_tx) = self.release_tx.take() {
            let _ = release_tx.send(());
        }
    }
}

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
    pub(crate) connection_id: revy_voxel_core::ConnectionId,
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

pub(crate) struct AcceptedGenerationSession {
    pub(crate) generation_id: GenerationId,
    pub(crate) session: AcceptedTransportSession,
    queued_accept: QueuedAcceptGuard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GenerationId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactsReloadResult {
    pub reloaded_plugin_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopologyReloadResult {
    pub activated_generation_id: GenerationId,
    pub retired_generation_ids: Vec<GenerationId>,
    pub applied_config_change: bool,
    pub reconfigured_adapter_ids: Vec<String>,
}

impl TopologyReloadResult {
    #[must_use]
    pub fn changed(&self, previous_generation_id: GenerationId) -> bool {
        self.activated_generation_id != previous_generation_id
            || self.applied_config_change
            || !self.retired_generation_ids.is_empty()
            || !self.reconfigured_adapter_ids.is_empty()
    }
}

impl AcceptedGenerationSession {
    pub(crate) fn new(
        generation_id: GenerationId,
        session: AcceptedTransportSession,
        queued_accept: QueuedAcceptGuard,
    ) -> Self {
        Self {
            generation_id,
            session,
            queued_accept,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreReloadResult {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullReloadResult {
    pub reloaded_plugin_ids: Vec<String>,
    pub topology: TopologyReloadResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeReloadResult {
    Artifacts(ArtifactsReloadResult),
    Topology(TopologyReloadResult),
    Core(CoreReloadResult),
    Full(FullReloadResult),
}

#[derive(Clone)]
pub(crate) struct ActiveGeneration {
    pub(crate) generation_id: GenerationId,
    pub(crate) config: ServerConfig,
    pub(crate) protocol_registry: ProtocolRegistry,
    pub(crate) default_adapter: Arc<dyn ProtocolAdapter>,
    pub(crate) default_bedrock_adapter: Option<Arc<dyn ProtocolAdapter>>,
    pub(crate) listener_bindings: Vec<ListenerBinding>,
}

#[derive(Clone)]
pub(crate) struct DrainingGeneration {
    pub(crate) generation: Arc<ActiveGeneration>,
    pub(crate) drain_deadline_ms: u64,
}

pub(crate) enum GenerationAdmission {
    Active(Arc<ActiveGeneration>),
    Draining(Arc<ActiveGeneration>),
    ExpiredDraining,
    Missing,
}

pub(crate) struct TopologyListenerWorker {
    pub(crate) transport: TransportKind,
    pub(crate) generation_tx: watch::Sender<GenerationId>,
    pub(crate) control_tx: mpsc::Sender<ListenerWorkerControl>,
    pub(crate) shutdown_tx: Option<oneshot::Sender<()>>,
    pub(crate) join_handle: Option<JoinHandle<()>>,
}

pub(crate) enum ListenerWorkerControl {
    Export {
        ack_tx: oneshot::Sender<Result<std::net::TcpListener, RuntimeError>>,
    },
}

pub(crate) struct RuntimeGenerationState {
    pub(crate) active: Arc<ActiveGeneration>,
    pub(crate) draining: Vec<DrainingGeneration>,
    pub(crate) listener_workers: HashMap<TransportKind, TopologyListenerWorker>,
    pub(crate) next_generation_id: u64,
}

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

#[cfg(test)]
struct ReloadStagePauseHook {
    reached_tx: Option<oneshot::Sender<()>>,
    release_rx: oneshot::Receiver<()>,
}

#[cfg(test)]
pub(crate) struct ReloadStagePauseHandle {
    reached_rx: oneshot::Receiver<()>,
    release_tx: Option<oneshot::Sender<()>>,
}

#[cfg(test)]
struct LoginAcceptCommitPauseHook {
    reached_tx: Option<oneshot::Sender<()>>,
    release_rx: oneshot::Receiver<()>,
}

#[cfg(test)]
pub(crate) struct LoginAcceptCommitPauseHandle {
    reached_rx: oneshot::Receiver<()>,
    release_tx: Option<oneshot::Sender<()>>,
}

pub(crate) struct OnlineAuthKeys {
    pub(crate) private_key: rsa::RsaPrivateKey,
    pub(crate) public_key_der: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct LoginChallengeState {
    pub(crate) username: String,
    pub(crate) verify_token: [u8; LOGIN_VERIFY_TOKEN_LEN],
    pub(crate) auth_generation: Arc<dyn AuthGenerationHandle>,
}

#[derive(Clone, Default)]
pub(crate) struct QueuedAcceptTracker {
    counts: Arc<StdMutex<HashMap<GenerationId, usize>>>,
}

pub(crate) struct QueuedAcceptGuard {
    tracker: QueuedAcceptTracker,
    generation_id: Option<GenerationId>,
}

impl QueuedAcceptTracker {
    pub(crate) fn track(&self, generation_id: GenerationId) -> QueuedAcceptGuard {
        self.increment(generation_id);
        QueuedAcceptGuard {
            tracker: self.clone(),
            generation_id: Some(generation_id),
        }
    }

    pub(crate) fn increment(&self, generation_id: GenerationId) {
        let mut counts = self
            .counts
            .lock()
            .expect("queued accept tracker should not be poisoned");
        let entry = counts.entry(generation_id).or_insert(0);
        *entry = entry.saturating_add(1);
    }

    pub(crate) fn decrement(&self, generation_id: GenerationId) {
        let mut counts = self
            .counts
            .lock()
            .expect("queued accept tracker should not be poisoned");
        let Some(entry) = counts.get_mut(&generation_id) else {
            return;
        };
        *entry = entry.saturating_sub(1);
        if *entry == 0 {
            counts.remove(&generation_id);
        }
    }

    pub(crate) fn generation_ids(&self) -> HashSet<GenerationId> {
        self.counts
            .lock()
            .expect("queued accept tracker should not be poisoned")
            .keys()
            .copied()
            .collect()
    }

    pub(crate) fn total_count(&self) -> usize {
        self.counts
            .lock()
            .expect("queued accept tracker should not be poisoned")
            .values()
            .copied()
            .sum()
    }
}

impl Drop for QueuedAcceptGuard {
    fn drop(&mut self) {
        if let Some(generation_id) = self.generation_id.take() {
            self.tracker.decrement(generation_id);
        }
    }
}

pub(crate) fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .expect("current unix time in milliseconds should fit into u64")
}
