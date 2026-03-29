use crate::PluginHostError;
use crate::config::RuntimeSelectionConfig;
use crate::host::PreparedProtocolTopology;
use crate::host::{PluginFailureAction, PluginHostStatusSnapshot};
use crate::registry::{LoadedPluginSet, ProtocolRegistry};
use mc_plugin_api::abi::PluginKind;
use mc_plugin_api::codec::admin_surface::{
    AdminSurfaceInstanceDeclaration, AdminSurfacePauseView, AdminSurfaceStatusView,
};
use mc_plugin_api::codec::auth::{AuthMode, BedrockAuthResult};
use mc_plugin_api::codec::gameplay::GameplaySessionSnapshot;
use mc_plugin_api::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_api::host_api::AdminSurfaceHostApiV1;
use mc_proto_common::StorageError;
use revy_voxel_core::{
    AdminSurfaceCapabilitySet, AdminSurfaceProfileId, AuthCapabilitySet, ConnectionId,
    GameplayCapabilitySet, GameplayCommand, GameplayJournal, GameplayJournalApplyResult,
    GameplayProfileId, PlayerId, PluginGenerationId, ServerCore, SessionCapabilitySet,
    StorageCapabilitySet, TargetedEvent, WorldSnapshot,
};
use std::any::Any;
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

pub trait GameplayProfileHandle: Send + Sync {
    fn profile_id(&self) -> GameplayProfileId;

    fn capability_set(&self) -> GameplayCapabilitySet;

    fn plugin_generation_id(&self) -> Option<PluginGenerationId>;

    fn prepare_player_join(
        &self,
        snapshot: ServerCore,
        session: &SessionCapabilitySet,
        connection_id: ConnectionId,
        username: String,
        player_id: PlayerId,
        now_ms: u64,
    ) -> Result<GameplayJournal, PluginHostError>;

    fn prepare_command(
        &self,
        snapshot: ServerCore,
        session: &SessionCapabilitySet,
        command: &GameplayCommand,
        now_ms: u64,
    ) -> Result<GameplayJournal, PluginHostError>;

    fn prepare_tick(
        &self,
        snapshot: ServerCore,
        session: &SessionCapabilitySet,
        player_id: PlayerId,
        now_ms: u64,
    ) -> Result<GameplayJournal, PluginHostError>;

    fn handle_player_join(
        &self,
        core: &mut ServerCore,
        session: &SessionCapabilitySet,
        connection_id: ConnectionId,
        username: String,
        player_id: PlayerId,
        now_ms: u64,
    ) -> Result<Vec<TargetedEvent>, PluginHostError> {
        let journal = self.prepare_player_join(
            core.clone(),
            session,
            connection_id,
            username,
            player_id,
            now_ms,
        )?;
        match core.validate_and_apply_gameplay_journal(journal) {
            GameplayJournalApplyResult::Applied(events) => Ok(events),
            GameplayJournalApplyResult::Conflict => Err(PluginHostError::Config(
                "prepared gameplay join journal conflicted against the live core".to_string(),
            )),
        }
    }

    fn handle_command(
        &self,
        core: &mut ServerCore,
        session: &SessionCapabilitySet,
        command: &GameplayCommand,
        now_ms: u64,
    ) -> Result<Vec<TargetedEvent>, PluginHostError> {
        let journal = self.prepare_command(core.clone(), session, command, now_ms)?;
        match core.validate_and_apply_gameplay_journal(journal) {
            GameplayJournalApplyResult::Applied(events) => Ok(events),
            GameplayJournalApplyResult::Conflict => Err(PluginHostError::Config(
                "prepared gameplay command journal conflicted against the live core".to_string(),
            )),
        }
    }

    fn handle_tick(
        &self,
        core: &mut ServerCore,
        session: &SessionCapabilitySet,
        player_id: PlayerId,
        now_ms: u64,
    ) -> Result<Vec<TargetedEvent>, PluginHostError> {
        let journal = self.prepare_tick(core.clone(), session, player_id, now_ms)?;
        match core.validate_and_apply_gameplay_journal(journal) {
            GameplayJournalApplyResult::Applied(events) => Ok(events),
            GameplayJournalApplyResult::Conflict => Err(PluginHostError::Config(
                "prepared gameplay tick journal conflicted against the live core".to_string(),
            )),
        }
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the gameplay plugin rejects the session
    /// close notification.
    fn session_closed(&self, session: &GameplaySessionSnapshot) -> Result<(), PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the gameplay plugin cannot export its
    /// session-owned runtime state for process handoff.
    fn export_session_state(
        &self,
        _session: &GameplaySessionSnapshot,
    ) -> Result<Vec<u8>, PluginHostError> {
        Ok(Vec::new())
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the gameplay plugin cannot import a
    /// previously exported session-owned runtime state blob.
    fn import_session_state(
        &self,
        _session: &GameplaySessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), PluginHostError> {
        Ok(())
    }
}

pub trait StorageProfileHandle: Send + Sync {
    fn plugin_id(&self) -> &str;

    fn capability_set(&self) -> StorageCapabilitySet;

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
    fn capability_set(&self) -> AuthCapabilitySet;

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

pub trait AdminSurfaceProfileHandle: Send + Sync {
    fn profile_id(&self) -> &AdminSurfaceProfileId;

    fn capability_set(&self) -> AdminSurfaceCapabilitySet;

    fn plugin_generation_id(&self) -> Option<PluginGenerationId>;

    fn declare_instance(
        &self,
        instance_id: &str,
        surface_config_path: Option<&Path>,
    ) -> Result<AdminSurfaceInstanceDeclaration, PluginHostError>;

    fn start(
        &self,
        instance_id: &str,
        surface_config_path: Option<&Path>,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, PluginHostError>;

    fn pause_for_upgrade(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfacePauseView, PluginHostError>;

    fn resume_from_upgrade(
        &self,
        instance_id: &str,
        surface_config_path: Option<&Path>,
        resume_payload: &[u8],
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, PluginHostError>;

    fn activate_after_upgrade_commit(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<(), PluginHostError>;

    fn resume_after_upgrade_rollback(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, PluginHostError>;

    fn shutdown(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<(), PluginHostError>;
}

pub struct RuntimeProtocolTopologyCandidate {
    prepared: PreparedProtocolTopology,
    requires_protocol_swap: bool,
}

impl RuntimeProtocolTopologyCandidate {
    #[must_use]
    pub(crate) const fn new(
        prepared: PreparedProtocolTopology,
        requires_protocol_swap: bool,
    ) -> Self {
        Self {
            prepared,
            requires_protocol_swap,
        }
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
    pub const fn requires_protocol_swap(&self) -> bool {
        self.requires_protocol_swap
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

pub struct StagedRuntimeSelection {
    loaded_plugins: LoadedPluginSet,
    reloaded_plugin_ids: Vec<String>,
    protocol_topology: RuntimeProtocolTopologyCandidate,
    staged: Option<Box<dyn Any + Send>>,
}

impl StagedRuntimeSelection {
    #[must_use]
    pub(crate) fn new<T: Any + Send>(
        loaded_plugins: LoadedPluginSet,
        reloaded_plugin_ids: Vec<String>,
        protocol_topology: RuntimeProtocolTopologyCandidate,
        staged: T,
    ) -> Self {
        Self {
            loaded_plugins,
            reloaded_plugin_ids,
            protocol_topology,
            staged: Some(Box::new(staged)),
        }
    }

    #[must_use]
    pub const fn loaded_plugins(&self) -> &LoadedPluginSet {
        &self.loaded_plugins
    }

    #[must_use]
    pub fn reloaded_plugin_ids(&self) -> &[String] {
        &self.reloaded_plugin_ids
    }

    #[must_use]
    pub const fn protocol_topology(&self) -> &RuntimeProtocolTopologyCandidate {
        &self.protocol_topology
    }

    pub(crate) fn into_parts(
        mut self,
    ) -> (
        LoadedPluginSet,
        Vec<String>,
        RuntimeProtocolTopologyCandidate,
        Box<dyn Any + Send>,
    ) {
        (
            self.loaded_plugins,
            self.reloaded_plugin_ids,
            self.protocol_topology,
            self.staged
                .take()
                .expect("staged runtime selection should contain staged state"),
        )
    }
}

pub struct PreparedRuntimeSelection {
    loaded_plugins: LoadedPluginSet,
    reloaded_plugin_ids: Vec<String>,
    protocol_topology: RuntimeProtocolTopologyCandidate,
    staged: Option<Box<dyn Any + Send>>,
}

impl PreparedRuntimeSelection {
    #[must_use]
    pub(crate) fn new<T: Any + Send>(
        loaded_plugins: LoadedPluginSet,
        reloaded_plugin_ids: Vec<String>,
        protocol_topology: RuntimeProtocolTopologyCandidate,
        staged: T,
    ) -> Self {
        Self {
            loaded_plugins,
            reloaded_plugin_ids,
            protocol_topology,
            staged: Some(Box::new(staged)),
        }
    }

    #[must_use]
    pub const fn loaded_plugins(&self) -> &LoadedPluginSet {
        &self.loaded_plugins
    }

    #[must_use]
    pub fn reloaded_plugin_ids(&self) -> &[String] {
        &self.reloaded_plugin_ids
    }

    #[must_use]
    pub const fn protocol_topology(&self) -> &RuntimeProtocolTopologyCandidate {
        &self.protocol_topology
    }

    pub(crate) fn take_staged<T: Any + Send>(mut self) -> T {
        *self
            .staged
            .take()
            .expect("prepared runtime selection should contain staged state")
            .downcast::<T>()
            .expect("prepared runtime selection staged payload type should match")
    }
}

pub trait RuntimePluginHost: Send + Sync {
    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the host cannot stage its
    /// runtime-selected gameplay/auth/plugin state with the provided config.
    fn stage_runtime_selection(
        &self,
        config: &RuntimeSelectionConfig,
    ) -> Result<StagedRuntimeSelection, PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when a modified plugin cannot be staged.
    fn stage_runtime_artifacts(&self) -> Result<StagedRuntimeSelection, PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the staged candidate cannot be
    /// validated against the live runtime snapshot.
    fn finalize_staged_runtime_selection(
        &self,
        staged: StagedRuntimeSelection,
        runtime: &RuntimeReloadContext,
    ) -> Result<PreparedRuntimeSelection, PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the host cannot stage its
    /// runtime-selected gameplay/auth/plugin state with the provided config.
    fn prepare_runtime_selection(
        &self,
        config: &RuntimeSelectionConfig,
        runtime: &RuntimeReloadContext,
    ) -> Result<PreparedRuntimeSelection, PluginHostError>;

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when a modified plugin cannot be staged.
    fn prepare_runtime_artifacts(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<PreparedRuntimeSelection, PluginHostError>;

    fn commit_runtime_selection(&self, prepared: PreparedRuntimeSelection);

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
