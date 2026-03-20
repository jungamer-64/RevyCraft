mod bootstrap;
mod core_loop;
mod session;
#[cfg(test)]
mod tests;

use crate::RuntimeError;
use crate::config::{ServerConfig, ServerConfigSource};
use crate::host::PluginHost;
use crate::plugin_host::{
    AuthGeneration, HotSwappableAuthProfile, HotSwappableGameplayProfile,
    HotSwappableStorageProfile,
};
use crate::registry::{ListenerBinding, ProtocolRegistry};
use crate::transport::AcceptedTransportSession;
use mc_core::{
    ConnectionId, CoreEvent, EntityId, GameplayProfileId, PlayerId, ServerCore,
    SessionCapabilitySet, WorldSnapshot,
};
use mc_plugin_api::{GameplaySessionSnapshot, ProtocolSessionSnapshot};
use mc_proto_common::{ConnectionPhase, ProtocolAdapter, TransportKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

pub use self::bootstrap::spawn_server;

pub(crate) const LOGIN_SERVER_ID: &str = "";
pub(crate) const LOGIN_VERIFY_TOKEN_LEN: usize = 4;

pub struct RunningServer {
    plugin_host: Option<Arc<PluginHost>>,
    runtime: Arc<RuntimeServer>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<Result<(), RuntimeError>>,
}

impl RunningServer {
    #[must_use]
    pub fn listener_bindings(&self) -> Vec<ListenerBinding> {
        self.runtime.listener_bindings()
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a loaded protocol plugin cannot be
    /// reloaded successfully.
    pub async fn reload_plugins(&self) -> Result<Vec<String>, RuntimeError> {
        match &self.plugin_host {
            Some(plugin_host) => self.runtime.reload_plugins(plugin_host).await,
            None => Ok(Vec::new()),
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot materialize a candidate
    /// topology or bind the required listeners.
    pub async fn reload_topology(&self) -> Result<TopologyReloadResult, RuntimeError> {
        match &self.plugin_host {
            Some(plugin_host) => self.runtime.reload_topology(plugin_host).await,
            None => Ok(self.runtime.noop_topology_reload_result()),
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

#[derive(Clone)]
pub(crate) struct SessionHandle {
    pub(crate) tx: mpsc::UnboundedSender<SessionMessage>,
    pub(crate) topology_generation_id: TopologyGenerationId,
    pub(crate) phase: ConnectionPhase,
    pub(crate) adapter_id: Option<String>,
    pub(crate) player_id: Option<PlayerId>,
    pub(crate) entity_id: Option<EntityId>,
    pub(crate) gameplay_profile: Option<GameplayProfileId>,
    pub(crate) session_capabilities: Option<SessionCapabilitySet>,
}

#[derive(Clone, Debug)]
pub(crate) enum SessionMessage {
    Event(Arc<CoreEvent>),
    Terminate { reason: String },
}

pub(crate) struct SessionState {
    pub(crate) topology_generation_id: TopologyGenerationId,
    pub(crate) transport: TransportKind,
    pub(crate) phase: ConnectionPhase,
    pub(crate) adapter: Option<Arc<dyn ProtocolAdapter>>,
    pub(crate) gameplay: Option<Arc<HotSwappableGameplayProfile>>,
    pub(crate) login_challenge: Option<LoginChallengeState>,
    pub(crate) player_id: Option<PlayerId>,
    pub(crate) entity_id: Option<EntityId>,
    pub(crate) session_capabilities: Option<SessionCapabilitySet>,
}

pub(crate) struct RuntimeState {
    pub(crate) core: ServerCore,
    pub(crate) dirty: bool,
}

pub(crate) struct AcceptedTopologySession {
    pub(crate) topology_generation_id: TopologyGenerationId,
    pub(crate) session: AcceptedTransportSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TopologyGenerationId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopologyReloadResult {
    pub activated_generation_id: TopologyGenerationId,
    pub retired_generation_ids: Vec<TopologyGenerationId>,
    pub applied_config_change: bool,
    pub reconfigured_adapter_ids: Vec<String>,
}

pub(crate) struct RuntimeTopologyGeneration {
    pub(crate) generation_id: TopologyGenerationId,
    pub(crate) config: ServerConfig,
    pub(crate) protocol_registry: ProtocolRegistry,
    pub(crate) default_adapter: Arc<dyn ProtocolAdapter>,
    pub(crate) default_bedrock_adapter: Option<Arc<dyn ProtocolAdapter>>,
    pub(crate) listener_bindings: Vec<ListenerBinding>,
}

pub(crate) struct DrainingTopologyGeneration {
    pub(crate) generation: Arc<RuntimeTopologyGeneration>,
    pub(crate) drain_deadline_ms: u64,
}

pub(crate) struct TopologyListenerWorker {
    pub(crate) transport: TransportKind,
    pub(crate) generation_tx: watch::Sender<TopologyGenerationId>,
    pub(crate) shutdown_tx: Option<oneshot::Sender<()>>,
    pub(crate) join_handle: Option<JoinHandle<()>>,
}

pub(crate) struct RuntimeTopologyState {
    pub(crate) active: Arc<RuntimeTopologyGeneration>,
    pub(crate) draining: Vec<DrainingTopologyGeneration>,
    pub(crate) listener_workers: HashMap<TransportKind, TopologyListenerWorker>,
    pub(crate) next_generation_id: u64,
}

pub(crate) struct RuntimeServer {
    pub(crate) config: ServerConfig,
    pub(crate) config_source: ServerConfigSource,
    pub(crate) plugin_host: Option<Arc<PluginHost>>,
    pub(crate) topology: RwLock<RuntimeTopologyState>,
    pub(crate) auth_profile: Arc<HotSwappableAuthProfile>,
    pub(crate) bedrock_auth_profile: Option<Arc<HotSwappableAuthProfile>>,
    pub(crate) online_auth_keys: Option<Arc<OnlineAuthKeys>>,
    pub(crate) storage_profile: Arc<HotSwappableStorageProfile>,
    pub(crate) state: Mutex<RuntimeState>,
    pub(crate) sessions: Mutex<HashMap<ConnectionId, SessionHandle>>,
    pub(crate) next_connection_id: Mutex<u64>,
    pub(crate) accepted_tx: mpsc::UnboundedSender<AcceptedTopologySession>,
}

pub(crate) struct OnlineAuthKeys {
    pub(crate) private_key: rsa::RsaPrivateKey,
    pub(crate) public_key_der: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct LoginChallengeState {
    pub(crate) username: String,
    pub(crate) verify_token: [u8; LOGIN_VERIFY_TOKEN_LEN],
    pub(crate) auth_generation: Arc<AuthGeneration>,
    #[allow(dead_code)]
    pub(crate) challenge_started_at: u64,
}

pub(crate) struct ProtocolReloadSession {
    pub(crate) adapter_id: String,
    pub(crate) session: ProtocolSessionSnapshot,
}

pub(crate) struct RuntimeReloadContext {
    pub(crate) protocol_sessions: Vec<ProtocolReloadSession>,
    pub(crate) gameplay_sessions: Vec<GameplaySessionSnapshot>,
    pub(crate) snapshot: WorldSnapshot,
    pub(crate) world_dir: PathBuf,
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
