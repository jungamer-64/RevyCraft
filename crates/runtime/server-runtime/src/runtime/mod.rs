mod admin;
mod bootstrap;
mod core_loop;
mod session;
mod status;
#[cfg(test)]
mod tests;

use self::admin::RemoteAdminPrincipal;
use crate::RuntimeError;
use crate::config::{ServerConfig, ServerConfigSource, StaticConfig};
use crate::transport::AcceptedTransportSession;
pub use crate::{
    AdminConfigReloadView, AdminGenerationCountView, AdminGenerationReloadView,
    AdminListenerBindingView, AdminNamedCountView, AdminPermission, AdminPhaseCountView,
    AdminPluginHostView, AdminPluginsReloadView, AdminPrincipal, AdminRequest, AdminResponse,
    AdminSessionSummaryView, AdminSessionView, AdminSessionsView, AdminStatusView,
    AdminTransportCountView, ListenerBinding, PluginFailureAction, PluginFailureMatrix,
    PluginHostStatusSnapshot,
};
use mc_core::{
    ConnectionId, CoreEvent, EntityId, GameplayProfileId, InventoryContainer,
    InventoryTransactionContext, PlayerId, ServerCore, SessionCapabilitySet,
};
use mc_plugin_host::registry::{LoadedPluginSet, ProtocolRegistry};
use mc_plugin_host::runtime::{
    AdminUiProfileHandle, AuthGenerationHandle, AuthProfileHandle, GameplayProfileHandle,
    RuntimePluginHost, RuntimeReloadContext, StorageProfileHandle,
};
use mc_proto_common::{ConnectionPhase, ProtocolAdapter, TransportKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock as AsyncRwLock, mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};

pub use self::admin::{AdminAuthError, AdminCommandError, AdminControlPlaneHandle, AdminSubject};
pub use self::status::{
    GenerationCountSnapshot, GenerationStatusSnapshot, GenerationStatusState,
    OptionalNamedCountSnapshot, PhaseCountSnapshot, RuntimeStatusSnapshot, SessionStatusSnapshot,
    SessionSummarySnapshot, TransportCountSnapshot, format_runtime_status_summary,
};

pub(crate) const LOGIN_SERVER_ID: &str = "";
pub(crate) const LOGIN_VERIFY_TOKEN_LEN: usize = 4;
pub(crate) const ACCEPT_QUEUE_CAPACITY: usize = 256;
pub(crate) const SESSION_OUTBOUND_QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReloadScope {
    Plugins,
    Config,
    Generation,
}

pub struct ServerSupervisor {
    running: RunningServer,
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

    #[must_use]
    pub fn admin_grpc_bind_addr(&self) -> Option<SocketAddr> {
        self.running.admin_grpc_bind_addr()
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the requested reload scope cannot be applied.
    pub async fn reload(&self, scope: ReloadScope) -> Result<ReloadResult, RuntimeError> {
        self.running.reload(scope).await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a loaded plugin cannot be reloaded successfully.
    pub async fn reload_plugins(&self) -> Result<Vec<String>, RuntimeError> {
        match self.reload(ReloadScope::Plugins).await? {
            ReloadResult::Plugins(reloaded) => Ok(reloaded),
            ReloadResult::Config(_) | ReloadResult::Generation(_) => {
                unreachable!("plugin reload should only produce a plugin-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot reconcile live config state or apply a
    /// candidate topology.
    pub async fn reload_config(&self) -> Result<ConfigReloadResult, RuntimeError> {
        match self.reload(ReloadScope::Config).await? {
            ReloadResult::Config(result) => Ok(result),
            ReloadResult::Plugins(_) | ReloadResult::Generation(_) => {
                unreachable!("config reload should only produce a config-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot materialize a candidate topology.
    pub async fn reload_generation(&self) -> Result<GenerationReloadResult, RuntimeError> {
        match self.reload(ReloadScope::Generation).await? {
            ReloadResult::Generation(result) => Ok(result),
            ReloadResult::Plugins(_) | ReloadResult::Config(_) => {
                unreachable!("generation reload should only produce a generation-scoped result")
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
    /// Returns [`RuntimeError`] when the server task fails while shutting down.
    pub async fn shutdown(self) -> Result<(), RuntimeError> {
        self.running.shutdown().await
    }
}

pub(crate) struct RunningServer {
    pub(crate) runtime: Arc<RuntimeServer>,
    pub(crate) join_handle: JoinHandle<Result<(), RuntimeError>>,
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

    #[must_use]
    pub fn admin_grpc_bind_addr(&self) -> Option<SocketAddr> {
        self.runtime.admin_grpc_bind_addr()
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
    /// Returns [`RuntimeError`] when the requested reload scope cannot be applied.
    pub async fn reload(&self, scope: ReloadScope) -> Result<ReloadResult, RuntimeError> {
        let reload_host = self.runtime.reload_host.as_ref().ok_or_else(|| {
            RuntimeError::Config(
                "reload is unavailable without a reload-capable supervisor boot".into(),
            )
        })?;
        self.runtime.reload(reload_host.as_ref(), scope).await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a loaded plugin cannot be reloaded successfully.
    #[allow(dead_code)]
    pub async fn reload_plugins(&self) -> Result<Vec<String>, RuntimeError> {
        match self.reload(ReloadScope::Plugins).await? {
            ReloadResult::Plugins(reloaded) => Ok(reloaded),
            ReloadResult::Config(_) | ReloadResult::Generation(_) => {
                unreachable!("plugin reload should only produce a plugin-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot reconcile live config state or apply a
    /// candidate topology.
    #[allow(dead_code)]
    pub async fn reload_config(&self) -> Result<ConfigReloadResult, RuntimeError> {
        match self.reload(ReloadScope::Config).await? {
            ReloadResult::Config(result) => Ok(result),
            ReloadResult::Plugins(_) | ReloadResult::Generation(_) => {
                unreachable!("config reload should only produce a config-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot materialize a candidate topology.
    #[allow(dead_code)]
    pub async fn reload_generation(&self) -> Result<GenerationReloadResult, RuntimeError> {
        match self.reload(ReloadScope::Generation).await? {
            ReloadResult::Generation(result) => Ok(result),
            ReloadResult::Plugins(_) | ReloadResult::Config(_) => {
                unreachable!("generation reload should only produce a generation-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the server task fails while shutting down.
    pub async fn shutdown(self) -> Result<(), RuntimeError> {
        let _ = self.runtime.request_shutdown();
        self.join_handle.await?
    }
}

impl RuntimeServer {
    #[must_use]
    pub(crate) fn admin_grpc_bind_addr(&self) -> Option<SocketAddr> {
        self.static_config
            .admin_grpc
            .enabled
            .then_some(self.static_config.admin_grpc.bind_addr)
    }

    pub(crate) async fn selection_state(&self) -> RuntimeSelectionState {
        self.selection_state.read().await.clone()
    }

    pub(crate) async fn update_generation_config(&self, candidate_config: &ServerConfig) {
        let mut selection_state = self.selection_state.write().await;
        selection_state.config.network = candidate_config.network.clone();
        selection_state.config.topology = candidate_config.topology.clone();
    }
}

#[derive(Clone)]
pub(crate) struct SessionHandle {
    pub(crate) tx: mpsc::Sender<SessionMessage>,
    pub(crate) control_tx: watch::Sender<Option<String>>,
    pub(crate) generation: Arc<ActiveGeneration>,
    pub(crate) transport: TransportKind,
    pub(crate) phase: ConnectionPhase,
    pub(crate) adapter_id: Option<String>,
    pub(crate) player_id: Option<PlayerId>,
    pub(crate) entity_id: Option<EntityId>,
    pub(crate) gameplay_profile: Option<GameplayProfileId>,
    pub(crate) gameplay: Option<Arc<dyn GameplayProfileHandle>>,
    pub(crate) session_capabilities: Option<SessionCapabilitySet>,
}

#[derive(Clone, Debug)]
pub(crate) enum SessionMessage {
    Event(Arc<CoreEvent>),
    Terminate { reason: String },
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
    pub(crate) active_non_player_window: Option<(u8, InventoryContainer)>,
    pub(crate) pending_rejected_inventory_transaction: Option<InventoryTransactionContext>,
}

pub(crate) struct RuntimeState {
    pub(crate) core: ServerCore,
    pub(crate) dirty: bool,
}

pub(crate) struct AcceptedGenerationSession {
    pub(crate) generation_id: GenerationId,
    pub(crate) session: AcceptedTransportSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GenerationId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationReloadResult {
    pub activated_generation_id: GenerationId,
    pub retired_generation_ids: Vec<GenerationId>,
    pub applied_config_change: bool,
    pub reconfigured_adapter_ids: Vec<String>,
}

impl GenerationReloadResult {
    #[must_use]
    pub fn changed(&self, previous_generation_id: GenerationId) -> bool {
        self.activated_generation_id != previous_generation_id
            || self.applied_config_change
            || !self.retired_generation_ids.is_empty()
            || !self.reconfigured_adapter_ids.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigReloadResult {
    pub reloaded_plugins: Vec<String>,
    pub generation: GenerationReloadResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReloadResult {
    Plugins(Vec<String>),
    Config(ConfigReloadResult),
    Generation(GenerationReloadResult),
}

#[derive(Clone)]
pub(crate) struct RuntimeSelectionState {
    pub(crate) config: ServerConfig,
    pub(crate) loaded_plugins: LoadedPluginSet,
    pub(crate) auth_profile: Arc<dyn AuthProfileHandle>,
    pub(crate) bedrock_auth_profile: Option<Arc<dyn AuthProfileHandle>>,
    pub(crate) admin_ui: Option<Arc<dyn AdminUiProfileHandle>>,
    pub(crate) remote_admin_subjects: HashMap<String, RemoteAdminPrincipal>,
}

pub(crate) struct ActiveGeneration {
    pub(crate) generation_id: GenerationId,
    pub(crate) config: ServerConfig,
    pub(crate) protocol_registry: ProtocolRegistry,
    pub(crate) default_adapter: Arc<dyn ProtocolAdapter>,
    pub(crate) default_bedrock_adapter: Option<Arc<dyn ProtocolAdapter>>,
    pub(crate) listener_bindings: Vec<ListenerBinding>,
}

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
    pub(crate) shutdown_tx: Option<oneshot::Sender<()>>,
    pub(crate) join_handle: Option<JoinHandle<()>>,
}

pub(crate) struct RuntimeGenerationState {
    pub(crate) active: Arc<ActiveGeneration>,
    pub(crate) draining: Vec<DrainingGeneration>,
    pub(crate) listener_workers: HashMap<TransportKind, TopologyListenerWorker>,
    pub(crate) next_generation_id: u64,
}

pub(crate) struct RuntimeServer {
    pub(crate) static_config: StaticConfig,
    pub(crate) config_source: ServerConfigSource,
    pub(crate) reload_host: Option<Arc<dyn RuntimePluginHost>>,
    pub(crate) selection_state: AsyncRwLock<RuntimeSelectionState>,
    pub(crate) consistency_gate: AsyncRwLock<()>,
    pub(crate) generation_state: RwLock<RuntimeGenerationState>,
    pub(crate) online_auth_keys: Option<Arc<OnlineAuthKeys>>,
    pub(crate) storage_profile: Arc<dyn StorageProfileHandle>,
    pub(crate) state: Mutex<RuntimeState>,
    pub(crate) sessions: Mutex<HashMap<ConnectionId, SessionHandle>>,
    pub(crate) session_tasks: Mutex<JoinSet<(ConnectionId, Result<(), RuntimeError>)>>,
    pub(crate) next_connection_id: Mutex<u64>,
    pub(crate) accepted_tx: mpsc::Sender<AcceptedGenerationSession>,
    pub(crate) queued_accepts: QueuedAcceptTracker,
    pub(crate) shutting_down: AtomicBool,
    pub(crate) shutdown_tx: std::sync::Mutex<Option<oneshot::Sender<()>>>,
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

impl QueuedAcceptTracker {
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
