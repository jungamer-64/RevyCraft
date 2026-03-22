use crate::PluginHostError;
use crate::config::RuntimeSelectionConfig;
use crate::host::{PluginFailureAction, PluginHostStatusSnapshot};
use crate::plugin_host::PreparedProtocolTopology;
use crate::registry::{LoadedPluginSet, ProtocolRegistry};
use mc_core::{
    CapabilitySet, GameplayPolicyResolver, GameplayProfileId, PlayerId, PluginGenerationId,
    WorldSnapshot,
};
use mc_plugin_api::abi::PluginKind;
use mc_plugin_api::codec::auth::{AuthMode, BedrockAuthResult};
use mc_plugin_api::codec::gameplay::GameplaySessionSnapshot;
use mc_plugin_api::codec::protocol::ProtocolSessionSnapshot;
use mc_proto_common::StorageError;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolReloadSession {
    pub adapter_id: String,
    pub session: ProtocolSessionSnapshot,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeReloadContext {
    pub protocol_sessions: Vec<ProtocolReloadSession>,
    pub gameplay_sessions: Vec<GameplaySessionSnapshot>,
    pub snapshot: WorldSnapshot,
    pub world_dir: PathBuf,
}

pub trait GameplayProfileHandle: GameplayPolicyResolver + Send + Sync {
    fn profile_id(&self) -> GameplayProfileId;

    fn capability_set(&self) -> CapabilitySet;

    fn plugin_generation_id(&self) -> Option<PluginGenerationId>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the gameplay plugin rejects the session
    /// close notification.
    fn session_closed(&self, session: &GameplaySessionSnapshot) -> Result<(), PluginHostError>;
}

pub trait StorageProfileHandle: Send + Sync {
    fn plugin_id(&self) -> &str;

    fn capability_set(&self) -> CapabilitySet;

    fn plugin_generation_id(&self) -> Option<PluginGenerationId>;

    /// # Errors
    ///
    /// Returns [`StorageError`] when the storage plugin cannot materialize the
    /// requested world snapshot.
    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError>;

    /// # Errors
    ///
    /// Returns [`StorageError`] when the storage plugin cannot persist the
    /// provided world snapshot.
    fn save_snapshot(&self, world_dir: &Path, snapshot: &WorldSnapshot)
    -> Result<(), StorageError>;
}

pub trait AuthGenerationHandle: Send + Sync {
    fn generation_id(&self) -> PluginGenerationId;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when online authentication fails for the
    /// captured generation.
    fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, PluginHostError>;
}

pub trait AuthProfileHandle: Send + Sync {
    fn capability_set(&self) -> CapabilitySet;

    fn plugin_generation_id(&self) -> Option<PluginGenerationId>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the plugin cannot report its current
    /// authentication mode.
    fn mode(&self) -> Result<AuthMode, PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the current generation cannot be
    /// captured for an in-flight login challenge.
    fn capture_generation(&self) -> Result<Arc<dyn AuthGenerationHandle>, PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when offline authentication fails.
    fn authenticate_offline(&self, username: &str) -> Result<PlayerId, PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when online authentication fails.
    fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when bedrock offline authentication fails.
    fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when bedrock XBL authentication fails.
    fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, PluginHostError>;
}

pub struct RuntimeProtocolTopologyCandidate {
    prepared: PreparedProtocolTopology,
}

impl RuntimeProtocolTopologyCandidate {
    #[must_use]
    pub(crate) const fn new(prepared: PreparedProtocolTopology) -> Self {
        Self { prepared }
    }

    #[must_use]
    pub fn registry(&self) -> &ProtocolRegistry {
        &self.prepared.registry
    }

    #[must_use]
    pub fn managed_protocol_ids(&self) -> &[String] {
        &self.prepared.adapter_ids
    }

    #[must_use]
    pub(crate) fn into_prepared(self) -> PreparedProtocolTopology {
        self.prepared
    }
}

pub struct RuntimeSelectionResult {
    pub loaded_plugins: LoadedPluginSet,
    pub reloaded: Vec<String>,
}

pub trait RuntimePluginHost: Send + Sync {
    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the host cannot reconcile its
    /// runtime-selected gameplay/auth/plugin state with the provided config.
    fn reconcile_runtime_selection(
        &self,
        config: &RuntimeSelectionConfig,
        runtime: &RuntimeReloadContext,
    ) -> Result<RuntimeSelectionResult, PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when a modified plugin cannot be reloaded.
    fn reload_modified_with_context(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<String>, PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when a candidate protocol topology cannot be
    /// prepared for activation.
    fn prepare_protocol_topology_for_reload(
        &self,
    ) -> Result<RuntimeProtocolTopologyCandidate, PluginHostError>;

    fn activate_protocol_topology(&self, candidate: RuntimeProtocolTopologyCandidate);

    fn take_pending_fatal_error(&self) -> Option<PluginHostError>;

    fn handle_runtime_failure(
        &self,
        kind: PluginKind,
        plugin_id: &str,
        reason: &str,
    ) -> PluginFailureAction;

    fn managed_protocol_ids(&self) -> Vec<String>;

    fn status(&self) -> PluginHostStatusSnapshot;
}
