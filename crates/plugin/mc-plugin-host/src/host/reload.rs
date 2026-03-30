use super::{
    AdminSurfaceGeneration, Arc, ArtifactIdentity, AuthGeneration, GameplayGeneration,
    ManagedAdminSurfacePlugin, ManagedAuthPlugin, ManagedGameplayPlugin, ManagedStoragePlugin,
    PluginFailureStage, PluginHost, PluginKind, PreparedProtocolTopology, RuntimeError,
    RuntimeReloadContext, RuntimeSelectionConfig, StorageGeneration, SystemTime,
    import_storage_runtime_state, protocol_reload_compatible, validate_gameplay_session_migration,
    validate_protocol_session_migration,
};
use crate::runtime::ProtocolReloadSession;
use crate::runtime::{
    PreparedRuntimeSelection, RuntimeProtocolTopologyCandidate, StagedRuntimeSelection,
};
use mc_plugin_api::{AdminSurfaceProfileId, AuthProfileId, GameplayProfileId, StorageProfileId};
use std::collections::HashMap;
use std::hash::Hash;

struct PreparedFreshRuntimeSelection {
    candidate_config: RuntimeSelectionConfig,
    protocols: PreparedProtocolTopology,
    gameplay: HashMap<GameplayProfileId, ManagedGameplayPlugin>,
    storage: HashMap<StorageProfileId, ManagedStoragePlugin>,
    auth: HashMap<AuthProfileId, ManagedAuthPlugin>,
    admin_surface: HashMap<AdminSurfaceProfileId, ManagedAdminSurfacePlugin>,
}

struct PreparedProtocolArtifactUpdate {
    plugin_id: String,
    identity: ArtifactIdentity,
    loaded_at: SystemTime,
    generation: Arc<super::ProtocolGeneration>,
}

struct PreparedProfileArtifactUpdate<ProfileId, Generation> {
    plugin_id: String,
    profile_id: ProfileId,
    identity: ArtifactIdentity,
    loaded_at: SystemTime,
    generation: Arc<Generation>,
}

type PreparedGameplayArtifactUpdate =
    PreparedProfileArtifactUpdate<GameplayProfileId, GameplayGeneration>;
type PreparedStorageArtifactUpdate =
    PreparedProfileArtifactUpdate<StorageProfileId, StorageGeneration>;
type PreparedAuthArtifactUpdate = PreparedProfileArtifactUpdate<AuthProfileId, AuthGeneration>;
type PreparedAdminSurfaceArtifactUpdate =
    PreparedProfileArtifactUpdate<AdminSurfaceProfileId, AdminSurfaceGeneration>;

struct PreparedArtifactRuntimeSelection {
    protocol_updates: Vec<PreparedProtocolArtifactUpdate>,
    gameplay_updates: Vec<PreparedGameplayArtifactUpdate>,
    storage_updates: Vec<PreparedStorageArtifactUpdate>,
    auth_updates: Vec<PreparedAuthArtifactUpdate>,
    admin_surface_updates: Vec<PreparedAdminSurfaceArtifactUpdate>,
}

enum PreparedRuntimeSelectionState {
    Fresh(PreparedFreshRuntimeSelection),
    Artifacts(PreparedArtifactRuntimeSelection),
}

trait ProfileArtifactKindOps {
    type ProfileId: Clone + Eq + Hash;
    type Managed;
    type Generation;

    const KIND: PluginKind;
    const PROFILE_LABEL: &'static str;

    fn registry(host: &PluginHost) -> &std::sync::Mutex<HashMap<Self::ProfileId, Self::Managed>>;

    fn package(managed: &Self::Managed) -> &super::PluginPackage;

    fn package_mut(managed: &mut Self::Managed) -> &mut super::PluginPackage;

    fn profile_id(managed: &Self::Managed) -> &Self::ProfileId;

    fn loaded_at(managed: &Self::Managed) -> SystemTime;

    fn set_loaded_at(managed: &mut Self::Managed, loaded_at: SystemTime);

    fn set_active_loaded_at(managed: &mut Self::Managed, loaded_at: SystemTime);

    fn load_generation(
        host: &PluginHost,
        package: &super::PluginPackage,
    ) -> Result<Arc<Self::Generation>, RuntimeError>;

    fn generation_profile_id(generation: &Self::Generation) -> &Self::ProfileId;

    fn profile_change_message(
        plugin_id: &str,
        current_profile_id: &Self::ProfileId,
        next_profile_id: &Self::ProfileId,
    ) -> String;

    fn validate_update(
        host: &PluginHost,
        managed: &Self::Managed,
        update: &PreparedProfileArtifactUpdate<Self::ProfileId, Self::Generation>,
        runtime: &RuntimeReloadContext,
    ) -> Result<bool, RuntimeError>;

    fn validation_failure_message(
        managed: &Self::Managed,
        update: &PreparedProfileArtifactUpdate<Self::ProfileId, Self::Generation>,
    ) -> String;

    fn apply_generation(managed: &mut Self::Managed, generation: Arc<Self::Generation>);
}

struct GameplayArtifactOps;
struct StorageArtifactOps;
struct AuthArtifactOps;
struct AdminSurfaceArtifactOps;

impl ProfileArtifactKindOps for GameplayArtifactOps {
    type ProfileId = GameplayProfileId;
    type Managed = ManagedGameplayPlugin;
    type Generation = GameplayGeneration;

    const KIND: PluginKind = PluginKind::Gameplay;
    const PROFILE_LABEL: &'static str = "gameplay";

    fn registry(host: &PluginHost) -> &std::sync::Mutex<HashMap<Self::ProfileId, Self::Managed>> {
        &host.gameplay
    }

    fn package(managed: &Self::Managed) -> &super::PluginPackage {
        &managed.package
    }

    fn package_mut(managed: &mut Self::Managed) -> &mut super::PluginPackage {
        &mut managed.package
    }

    fn profile_id(managed: &Self::Managed) -> &Self::ProfileId {
        &managed.profile_id
    }

    fn loaded_at(managed: &Self::Managed) -> SystemTime {
        managed.loaded_at
    }

    fn set_loaded_at(managed: &mut Self::Managed, loaded_at: SystemTime) {
        managed.loaded_at = loaded_at;
    }

    fn set_active_loaded_at(managed: &mut Self::Managed, loaded_at: SystemTime) {
        managed.active_loaded_at = loaded_at;
    }

    fn load_generation(
        host: &PluginHost,
        package: &super::PluginPackage,
    ) -> Result<Arc<Self::Generation>, RuntimeError> {
        Ok(Arc::new(host.loader.load_gameplay_generation(
            package,
            host.generations.next_generation_id(),
            host.current_runtime_selection().buffer_limits,
        )?))
    }

    fn generation_profile_id(generation: &Self::Generation) -> &Self::ProfileId {
        &generation.profile_id
    }

    fn profile_change_message(
        plugin_id: &str,
        current_profile_id: &Self::ProfileId,
        next_profile_id: &Self::ProfileId,
    ) -> String {
        format!(
            "gameplay plugin `{plugin_id}` changed profile from `{}` to `{}` during reload",
            current_profile_id.as_str(),
            next_profile_id.as_str()
        )
    }

    fn validate_update(
        _host: &PluginHost,
        managed: &Self::Managed,
        update: &PreparedProfileArtifactUpdate<Self::ProfileId, Self::Generation>,
        runtime: &RuntimeReloadContext,
    ) -> Result<bool, RuntimeError> {
        validate_gameplay_session_migration(managed, &update.generation, runtime)
    }

    fn validation_failure_message(
        _managed: &Self::Managed,
        _update: &PreparedProfileArtifactUpdate<Self::ProfileId, Self::Generation>,
    ) -> String {
        "gameplay session migration failed".to_string()
    }

    fn apply_generation(managed: &mut Self::Managed, generation: Arc<Self::Generation>) {
        managed.profile.swap_generation(generation);
    }
}

impl ProfileArtifactKindOps for StorageArtifactOps {
    type ProfileId = StorageProfileId;
    type Managed = ManagedStoragePlugin;
    type Generation = StorageGeneration;

    const KIND: PluginKind = PluginKind::Storage;
    const PROFILE_LABEL: &'static str = "storage";

    fn registry(host: &PluginHost) -> &std::sync::Mutex<HashMap<Self::ProfileId, Self::Managed>> {
        &host.storage
    }

    fn package(managed: &Self::Managed) -> &super::PluginPackage {
        &managed.package
    }

    fn package_mut(managed: &mut Self::Managed) -> &mut super::PluginPackage {
        &mut managed.package
    }

    fn profile_id(managed: &Self::Managed) -> &Self::ProfileId {
        &managed.profile_id
    }

    fn loaded_at(managed: &Self::Managed) -> SystemTime {
        managed.loaded_at
    }

    fn set_loaded_at(managed: &mut Self::Managed, loaded_at: SystemTime) {
        managed.loaded_at = loaded_at;
    }

    fn set_active_loaded_at(managed: &mut Self::Managed, loaded_at: SystemTime) {
        managed.active_loaded_at = loaded_at;
    }

    fn load_generation(
        host: &PluginHost,
        package: &super::PluginPackage,
    ) -> Result<Arc<Self::Generation>, RuntimeError> {
        Ok(Arc::new(host.loader.load_storage_generation(
            package,
            host.generations.next_generation_id(),
            host.current_runtime_selection().buffer_limits,
        )?))
    }

    fn generation_profile_id(generation: &Self::Generation) -> &Self::ProfileId {
        &generation.profile_id
    }

    fn profile_change_message(
        plugin_id: &str,
        current_profile_id: &Self::ProfileId,
        next_profile_id: &Self::ProfileId,
    ) -> String {
        format!(
            "storage plugin `{plugin_id}` changed profile from `{}` to `{}` during reload",
            current_profile_id, next_profile_id
        )
    }

    fn validate_update(
        _host: &PluginHost,
        _managed: &Self::Managed,
        update: &PreparedProfileArtifactUpdate<Self::ProfileId, Self::Generation>,
        runtime: &RuntimeReloadContext,
    ) -> Result<bool, RuntimeError> {
        Ok(import_storage_runtime_state(
            &update.plugin_id,
            &update.generation,
            runtime,
        ))
    }

    fn validation_failure_message(
        _managed: &Self::Managed,
        _update: &PreparedProfileArtifactUpdate<Self::ProfileId, Self::Generation>,
    ) -> String {
        "storage runtime state import failed".to_string()
    }

    fn apply_generation(managed: &mut Self::Managed, generation: Arc<Self::Generation>) {
        let profile = Arc::clone(&managed.profile);
        let generation = Arc::clone(&generation);
        profile.with_reload_write(|_| {
            profile.swap_generation_while_reloading(generation);
        });
    }
}

impl ProfileArtifactKindOps for AuthArtifactOps {
    type ProfileId = AuthProfileId;
    type Managed = ManagedAuthPlugin;
    type Generation = AuthGeneration;

    const KIND: PluginKind = PluginKind::Auth;
    const PROFILE_LABEL: &'static str = "auth";

    fn registry(host: &PluginHost) -> &std::sync::Mutex<HashMap<Self::ProfileId, Self::Managed>> {
        &host.auth
    }

    fn package(managed: &Self::Managed) -> &super::PluginPackage {
        &managed.package
    }

    fn package_mut(managed: &mut Self::Managed) -> &mut super::PluginPackage {
        &mut managed.package
    }

    fn profile_id(managed: &Self::Managed) -> &Self::ProfileId {
        &managed.profile_id
    }

    fn loaded_at(managed: &Self::Managed) -> SystemTime {
        managed.loaded_at
    }

    fn set_loaded_at(managed: &mut Self::Managed, loaded_at: SystemTime) {
        managed.loaded_at = loaded_at;
    }

    fn set_active_loaded_at(managed: &mut Self::Managed, loaded_at: SystemTime) {
        managed.active_loaded_at = loaded_at;
    }

    fn load_generation(
        host: &PluginHost,
        package: &super::PluginPackage,
    ) -> Result<Arc<Self::Generation>, RuntimeError> {
        Ok(Arc::new(host.loader.load_auth_generation(
            package,
            host.generations.next_generation_id(),
            host.current_runtime_selection().buffer_limits,
        )?))
    }

    fn generation_profile_id(generation: &Self::Generation) -> &Self::ProfileId {
        &generation.profile_id
    }

    fn profile_change_message(
        plugin_id: &str,
        current_profile_id: &Self::ProfileId,
        next_profile_id: &Self::ProfileId,
    ) -> String {
        format!(
            "auth plugin `{plugin_id}` changed profile from `{}` to `{}` during reload",
            current_profile_id, next_profile_id
        )
    }

    fn validate_update(
        _host: &PluginHost,
        _managed: &Self::Managed,
        _update: &PreparedProfileArtifactUpdate<Self::ProfileId, Self::Generation>,
        _runtime: &RuntimeReloadContext,
    ) -> Result<bool, RuntimeError> {
        Ok(true)
    }

    fn validation_failure_message(
        _managed: &Self::Managed,
        _update: &PreparedProfileArtifactUpdate<Self::ProfileId, Self::Generation>,
    ) -> String {
        format!("{} reload validation failed", Self::PROFILE_LABEL)
    }

    fn apply_generation(managed: &mut Self::Managed, generation: Arc<Self::Generation>) {
        managed.profile.swap_generation(generation);
    }
}

impl ProfileArtifactKindOps for AdminSurfaceArtifactOps {
    type ProfileId = AdminSurfaceProfileId;
    type Managed = ManagedAdminSurfacePlugin;
    type Generation = AdminSurfaceGeneration;

    const KIND: PluginKind = PluginKind::AdminSurface;
    const PROFILE_LABEL: &'static str = "admin-surface";

    fn registry(host: &PluginHost) -> &std::sync::Mutex<HashMap<Self::ProfileId, Self::Managed>> {
        &host.admin_surface
    }

    fn package(managed: &Self::Managed) -> &super::PluginPackage {
        &managed.package
    }

    fn package_mut(managed: &mut Self::Managed) -> &mut super::PluginPackage {
        &mut managed.package
    }

    fn profile_id(managed: &Self::Managed) -> &Self::ProfileId {
        &managed.profile_id
    }

    fn loaded_at(managed: &Self::Managed) -> SystemTime {
        managed.loaded_at
    }

    fn set_loaded_at(managed: &mut Self::Managed, loaded_at: SystemTime) {
        managed.loaded_at = loaded_at;
    }

    fn set_active_loaded_at(managed: &mut Self::Managed, loaded_at: SystemTime) {
        managed.active_loaded_at = loaded_at;
    }

    fn load_generation(
        host: &PluginHost,
        package: &super::PluginPackage,
    ) -> Result<Arc<Self::Generation>, RuntimeError> {
        Ok(Arc::new(host.loader.load_admin_surface_generation(
            package,
            host.generations.next_generation_id(),
            host.current_runtime_selection().buffer_limits,
        )?))
    }

    fn generation_profile_id(generation: &Self::Generation) -> &Self::ProfileId {
        &generation.profile_id
    }

    fn profile_change_message(
        plugin_id: &str,
        current_profile_id: &Self::ProfileId,
        next_profile_id: &Self::ProfileId,
    ) -> String {
        format!(
            "admin-surface plugin `{plugin_id}` changed profile from `{}` to `{}` during reload",
            current_profile_id, next_profile_id
        )
    }

    fn validate_update(
        _host: &PluginHost,
        _managed: &Self::Managed,
        _update: &PreparedProfileArtifactUpdate<Self::ProfileId, Self::Generation>,
        _runtime: &RuntimeReloadContext,
    ) -> Result<bool, RuntimeError> {
        Ok(true)
    }

    fn validation_failure_message(
        _managed: &Self::Managed,
        _update: &PreparedProfileArtifactUpdate<Self::ProfileId, Self::Generation>,
    ) -> String {
        format!("{} reload validation failed", Self::PROFILE_LABEL)
    }

    fn apply_generation(managed: &mut Self::Managed, generation: Arc<Self::Generation>) {
        managed.profile.swap_generation(generation);
    }
}

impl PluginHost {
    fn current_protocol_topology_candidate(
        &self,
    ) -> Result<RuntimeProtocolTopologyCandidate, RuntimeError> {
        let protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned");
        let mut registry = crate::registry::ProtocolRegistry::new();
        let mut adapter_ids = Vec::new();
        let mut managed = HashMap::new();

        for (plugin_id, entry) in protocols.iter() {
            let adapter = Arc::clone(&entry.adapter) as Arc<dyn mc_proto_common::ProtocolAdapter>;
            let probe = Arc::clone(&entry.adapter) as Arc<dyn mc_proto_common::HandshakeProbe>;
            registry
                .register_adapter(adapter)
                .map_err(|error| RuntimeError::Config(error.to_string()))?;
            registry.register_probe(probe);
            adapter_ids.push(plugin_id.clone());
            managed.insert(plugin_id.clone(), entry.clone());
        }
        adapter_ids.sort();
        Ok(RuntimeProtocolTopologyCandidate::new(
            PreparedProtocolTopology {
                registry,
                adapter_ids,
                managed,
            },
            false,
        ))
    }

    fn protocol_selection_inputs_changed(&self, config: &RuntimeSelectionConfig) -> bool {
        let current = self.current_runtime_selection();
        current.plugin_allowlist != config.plugin_allowlist
            || current.buffer_limits.protocol_response_bytes
                != config.buffer_limits.protocol_response_bytes
            || current.buffer_limits.metadata_bytes != config.buffer_limits.metadata_bytes
    }

    fn protocol_artifacts_reloaded(&self, protocols: &PreparedProtocolTopology) -> bool {
        let active_protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned");
        protocols.managed.iter().any(|(plugin_id, candidate)| {
            active_protocols
                .get(plugin_id)
                .is_some_and(|active| candidate.loaded_at > active.active_loaded_at)
        })
    }

    pub(crate) fn requires_protocol_swap(
        &self,
        config: &RuntimeSelectionConfig,
        protocols: &PreparedProtocolTopology,
    ) -> bool {
        self.protocol_selection_inputs_changed(config)
            || self.protocol_artifacts_reloaded(protocols)
    }

    fn collect_fresh_reloaded_plugin_ids(
        &self,
        protocols: &PreparedProtocolTopology,
        gameplay: &HashMap<GameplayProfileId, ManagedGameplayPlugin>,
        storage: &HashMap<StorageProfileId, ManagedStoragePlugin>,
        auth: &HashMap<AuthProfileId, ManagedAuthPlugin>,
        admin_surface: &HashMap<AdminSurfaceProfileId, ManagedAdminSurfacePlugin>,
    ) -> Vec<String> {
        let active_protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned");
        let active_gameplay = self
            .gameplay
            .lock()
            .expect("plugin host mutex should not be poisoned");
        let active_storage = self
            .storage
            .lock()
            .expect("plugin host mutex should not be poisoned");
        let active_auth = self
            .auth
            .lock()
            .expect("plugin host mutex should not be poisoned");
        let active_admin_surface = self
            .admin_surface
            .lock()
            .expect("plugin host mutex should not be poisoned");

        let mut reloaded = Vec::new();
        for (plugin_id, candidate) in &protocols.managed {
            if active_protocols
                .get(plugin_id)
                .is_some_and(|active| candidate.loaded_at > active.active_loaded_at)
            {
                reloaded.push(plugin_id.clone());
            }
        }
        for candidate in gameplay.values() {
            if active_gameplay
                .get(&candidate.profile_id)
                .is_some_and(|active| candidate.loaded_at > active.active_loaded_at)
            {
                reloaded.push(candidate.package.plugin_id.clone());
            }
        }
        for candidate in storage.values() {
            if active_storage
                .get(&candidate.profile_id)
                .is_some_and(|active| candidate.loaded_at > active.active_loaded_at)
            {
                reloaded.push(candidate.package.plugin_id.clone());
            }
        }
        for candidate in auth.values() {
            if active_auth
                .get(&candidate.profile_id)
                .is_some_and(|active| candidate.loaded_at > active.active_loaded_at)
            {
                reloaded.push(candidate.package.plugin_id.clone());
            }
        }
        for candidate in admin_surface.values() {
            if active_admin_surface
                .get(&candidate.profile_id)
                .is_some_and(|active| candidate.loaded_at > active.active_loaded_at)
            {
                reloaded.push(candidate.package.plugin_id.clone());
            }
        }
        reloaded.sort();
        reloaded.dedup();
        reloaded
    }

    fn validate_fresh_protocol_sessions(
        &self,
        candidate: &PreparedProtocolTopology,
        protocol_sessions: &[ProtocolReloadSession],
    ) -> Result<(), RuntimeError> {
        let active_protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for (plugin_id, active) in active_protocols.iter() {
            let Some(candidate_managed) = candidate.managed.get(plugin_id) else {
                continue;
            };
            let current_generation = active
                .adapter
                .current_generation()
                .map_err(|error| RuntimeError::Config(error.to_string()))?;
            let candidate_generation = candidate_managed
                .adapter
                .current_generation()
                .map_err(|error| RuntimeError::Config(error.to_string()))?;
            if !protocol_reload_compatible(plugin_id, &current_generation, &candidate_generation) {
                return Err(RuntimeError::Config(format!(
                    "protocol session migration failed for `{plugin_id}` because route metadata changed"
                )));
            }
            if !validate_protocol_session_migration(
                active,
                &candidate_generation,
                protocol_sessions,
            )? {
                return Err(RuntimeError::Config(format!(
                    "protocol session migration failed for `{plugin_id}`"
                )));
            }
        }
        Ok(())
    }

    fn validate_fresh_gameplay_sessions(
        &self,
        candidate: &HashMap<GameplayProfileId, ManagedGameplayPlugin>,
        runtime: &RuntimeReloadContext,
    ) -> Result<(), RuntimeError> {
        let active_gameplay = self
            .gameplay
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for (profile_id, active) in active_gameplay.iter() {
            let Some(candidate_managed) = candidate.get(profile_id) else {
                continue;
            };
            let candidate_generation = candidate_managed.profile.current_generation();
            if !validate_gameplay_session_migration(active, &candidate_generation, runtime)? {
                return Err(RuntimeError::Config(format!(
                    "gameplay session migration failed for profile `{}`",
                    profile_id.as_str()
                )));
            }
        }
        Ok(())
    }

    fn validate_fresh_storage_runtime(
        &self,
        candidate: &HashMap<StorageProfileId, ManagedStoragePlugin>,
        runtime: &RuntimeReloadContext,
    ) -> Result<(), RuntimeError> {
        if let Some(managed) = candidate.get(&self.bootstrap_config.storage_profile)
            && !import_storage_runtime_state(
                &managed.package.plugin_id,
                &managed.profile.current_generation(),
                runtime,
            )
        {
            return Err(RuntimeError::Config(format!(
                "storage runtime state import failed for `{}`",
                managed.package.plugin_id
            )));
        }
        Ok(())
    }

    fn stage_profile_artifact_updates<Ops>(
        &self,
    ) -> Result<Vec<PreparedProfileArtifactUpdate<Ops::ProfileId, Ops::Generation>>, RuntimeError>
    where
        Ops: ProfileArtifactKindOps,
    {
        let mut updates = Vec::new();
        let mut registry = Ops::registry(self)
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for managed in registry.values_mut() {
            Ops::package_mut(managed).refresh_dynamic_manifest()?;
            let modified_at = Ops::package(managed).modified_at()?;
            if modified_at <= Ops::loaded_at(managed) {
                continue;
            }
            let identity = Ops::package(managed).artifact_identity(modified_at);
            let plugin_id = Ops::package(managed).plugin_id.clone();
            if self.failures.is_artifact_quarantined(&plugin_id, &identity) {
                continue;
            }
            let generation = match Ops::load_generation(self, Ops::package(managed)) {
                Ok(generation) => generation,
                Err(error) => {
                    self.failures.handle_candidate_failure(
                        Ops::KIND,
                        PluginFailureStage::Reload,
                        &plugin_id,
                        identity,
                        &error.to_string(),
                    )?;
                    continue;
                }
            };
            let profile_id = Ops::profile_id(managed).clone();
            let next_profile_id = Ops::generation_profile_id(&generation);
            if next_profile_id != &profile_id {
                self.failures.handle_candidate_failure(
                    Ops::KIND,
                    PluginFailureStage::Reload,
                    &plugin_id,
                    identity,
                    &Ops::profile_change_message(&plugin_id, &profile_id, next_profile_id),
                )?;
                continue;
            }
            updates.push(PreparedProfileArtifactUpdate {
                plugin_id,
                profile_id,
                identity,
                loaded_at: modified_at,
                generation,
            });
        }
        Ok(updates)
    }

    fn finalize_profile_artifact_updates<Ops>(
        &self,
        updates: Vec<PreparedProfileArtifactUpdate<Ops::ProfileId, Ops::Generation>>,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<PreparedProfileArtifactUpdate<Ops::ProfileId, Ops::Generation>>, RuntimeError>
    where
        Ops: ProfileArtifactKindOps,
    {
        let registry = Ops::registry(self)
            .lock()
            .expect("plugin host mutex should not be poisoned");
        let mut finalized = Vec::new();
        for update in updates {
            let Some(managed) = registry.get(&update.profile_id) else {
                continue;
            };
            if !Ops::validate_update(self, managed, &update, runtime)? {
                self.failures.handle_candidate_failure(
                    Ops::KIND,
                    PluginFailureStage::Reload,
                    &update.plugin_id,
                    update.identity.clone(),
                    &Ops::validation_failure_message(managed, &update),
                )?;
                continue;
            }
            finalized.push(update);
        }
        Ok(finalized)
    }

    fn commit_profile_artifact_updates<Ops>(
        &self,
        updates: Vec<PreparedProfileArtifactUpdate<Ops::ProfileId, Ops::Generation>>,
    ) where
        Ops: ProfileArtifactKindOps,
    {
        let mut registry = Ops::registry(self)
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for update in updates {
            if let Some(managed) = registry.get_mut(&update.profile_id) {
                Ops::apply_generation(managed, update.generation);
                Ops::set_loaded_at(managed, update.loaded_at);
                Ops::set_active_loaded_at(managed, update.loaded_at);
                self.failures.clear_plugin_state(&update.plugin_id);
            }
        }
    }

    fn collect_profile_update_plugin_ids<ProfileId, Generation>(
        updates: &[PreparedProfileArtifactUpdate<ProfileId, Generation>],
    ) -> Vec<String> {
        updates
            .iter()
            .map(|update| update.plugin_id.clone())
            .collect()
    }

    fn normalize_reloaded_plugin_ids(reloaded_plugin_ids: &mut Vec<String>) {
        reloaded_plugin_ids.sort();
        reloaded_plugin_ids.dedup();
    }

    fn stage_protocol_artifact_updates(
        &self,
    ) -> Result<Vec<PreparedProtocolArtifactUpdate>, RuntimeError> {
        let mut updates = Vec::new();
        let mut protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for managed in protocols.values_mut() {
            managed.package.refresh_dynamic_manifest()?;
            let modified_at = managed.package.modified_at()?;
            if modified_at <= managed.loaded_at {
                continue;
            }
            let identity = managed.package.artifact_identity(modified_at);
            if self
                .failures
                .is_artifact_quarantined(&managed.package.plugin_id, &identity)
            {
                continue;
            }
            let generation = match self.loader.load_protocol_generation(
                &managed.package,
                self.generations.next_generation_id(),
                self.current_runtime_selection().buffer_limits,
            ) {
                Ok(generation) => Arc::new(generation),
                Err(error) => {
                    self.failures.handle_candidate_failure(
                        PluginKind::Protocol,
                        PluginFailureStage::Reload,
                        &managed.package.plugin_id,
                        identity,
                        &error.to_string(),
                    )?;
                    continue;
                }
            };
            updates.push(PreparedProtocolArtifactUpdate {
                plugin_id: managed.package.plugin_id.clone(),
                identity,
                loaded_at: modified_at,
                generation,
            });
        }
        Ok(updates)
    }

    fn finalize_protocol_artifact_updates(
        &self,
        updates: Vec<PreparedProtocolArtifactUpdate>,
        protocol_sessions: &[ProtocolReloadSession],
    ) -> Result<Vec<PreparedProtocolArtifactUpdate>, RuntimeError> {
        let protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned");
        let mut finalized = Vec::new();
        for update in updates {
            let Some(managed) = protocols.get(&update.plugin_id) else {
                continue;
            };
            let current_generation = managed
                .adapter
                .current_generation()
                .map_err(|error| RuntimeError::Config(error.to_string()))?;
            if !protocol_reload_compatible(
                &update.plugin_id,
                &current_generation,
                &update.generation,
            ) {
                self.failures.handle_candidate_failure(
                    PluginKind::Protocol,
                    PluginFailureStage::Reload,
                    &update.plugin_id,
                    update.identity,
                    "protocol topology changed during reload",
                )?;
                continue;
            }
            if !validate_protocol_session_migration(managed, &update.generation, protocol_sessions)?
            {
                self.failures.handle_candidate_failure(
                    PluginKind::Protocol,
                    PluginFailureStage::Reload,
                    &update.plugin_id,
                    update.identity,
                    "protocol session migration failed",
                )?;
                continue;
            }
            finalized.push(update);
        }
        Ok(finalized)
    }

    fn stage_gameplay_artifact_updates(
        &self,
    ) -> Result<Vec<PreparedGameplayArtifactUpdate>, RuntimeError> {
        self.stage_profile_artifact_updates::<GameplayArtifactOps>()
    }

    fn finalize_gameplay_artifact_updates(
        &self,
        updates: Vec<PreparedGameplayArtifactUpdate>,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<PreparedGameplayArtifactUpdate>, RuntimeError> {
        self.finalize_profile_artifact_updates::<GameplayArtifactOps>(updates, runtime)
    }

    fn stage_storage_artifact_updates(
        &self,
    ) -> Result<Vec<PreparedStorageArtifactUpdate>, RuntimeError> {
        self.stage_profile_artifact_updates::<StorageArtifactOps>()
    }

    fn finalize_storage_artifact_updates(
        &self,
        updates: Vec<PreparedStorageArtifactUpdate>,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<PreparedStorageArtifactUpdate>, RuntimeError> {
        self.finalize_profile_artifact_updates::<StorageArtifactOps>(updates, runtime)
    }

    fn stage_auth_artifact_updates(&self) -> Result<Vec<PreparedAuthArtifactUpdate>, RuntimeError> {
        self.stage_profile_artifact_updates::<AuthArtifactOps>()
    }

    fn finalize_auth_artifact_updates(
        &self,
        updates: Vec<PreparedAuthArtifactUpdate>,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<PreparedAuthArtifactUpdate>, RuntimeError> {
        self.finalize_profile_artifact_updates::<AuthArtifactOps>(updates, runtime)
    }

    fn stage_admin_surface_artifact_updates(
        &self,
    ) -> Result<Vec<PreparedAdminSurfaceArtifactUpdate>, RuntimeError> {
        self.stage_profile_artifact_updates::<AdminSurfaceArtifactOps>()
    }

    fn finalize_admin_surface_artifact_updates(
        &self,
        updates: Vec<PreparedAdminSurfaceArtifactUpdate>,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<PreparedAdminSurfaceArtifactUpdate>, RuntimeError> {
        self.finalize_profile_artifact_updates::<AdminSurfaceArtifactOps>(updates, runtime)
    }

    /// Reloads modified protocol plugins in place.
    ///
    /// # Errors
    ///
    /// Returns an error when a modified protocol plugin cannot be reloaded.
    ///
    /// # Panics
    ///
    /// Panics if the protocol plugin registry mutex is poisoned.
    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn reload_modified(&self) -> Result<Vec<String>, RuntimeError> {
        let current_topology = self.current_protocol_topology_candidate()?;
        let loaded_plugins = {
            let gameplay = self
                .gameplay
                .lock()
                .expect("plugin host mutex should not be poisoned");
            let storage = self
                .storage
                .lock()
                .expect("plugin host mutex should not be poisoned");
            let auth = self
                .auth
                .lock()
                .expect("plugin host mutex should not be poisoned");
            let admin_surface = self
                .admin_surface
                .lock()
                .expect("plugin host mutex should not be poisoned");
            Self::loaded_plugin_set_from_parts(
                current_topology.registry().clone(),
                &gameplay,
                &storage,
                &auth,
                &admin_surface,
            )
        };
        let protocol_updates =
            self.finalize_protocol_artifact_updates(self.stage_protocol_artifact_updates()?, &[])?;
        let reloaded_plugin_ids = protocol_updates
            .iter()
            .map(|update| update.plugin_id.clone())
            .collect::<Vec<_>>();
        let prepared = PreparedRuntimeSelection::new(
            loaded_plugins,
            reloaded_plugin_ids.clone(),
            current_topology,
            PreparedRuntimeSelectionState::Artifacts(PreparedArtifactRuntimeSelection {
                protocol_updates,
                gameplay_updates: Vec::new(),
                storage_updates: Vec::new(),
                auth_updates: Vec::new(),
                admin_surface_updates: Vec::new(),
            }),
        );
        self.commit_runtime_selection(prepared);
        Ok(reloaded_plugin_ids)
    }

    pub(crate) fn stage_runtime_artifacts(&self) -> Result<StagedRuntimeSelection, RuntimeError> {
        let protocol_updates = self.stage_protocol_artifact_updates()?;
        let gameplay_updates = self.stage_gameplay_artifact_updates()?;
        let storage_updates = self.stage_storage_artifact_updates()?;
        let auth_updates = self.stage_auth_artifact_updates()?;
        let admin_surface_updates = self.stage_admin_surface_artifact_updates()?;
        let mut reloaded_plugin_ids = protocol_updates
            .iter()
            .map(|update| update.plugin_id.clone())
            .collect::<Vec<_>>();
        reloaded_plugin_ids.extend(Self::collect_profile_update_plugin_ids(&gameplay_updates));
        reloaded_plugin_ids.extend(Self::collect_profile_update_plugin_ids(&storage_updates));
        reloaded_plugin_ids.extend(Self::collect_profile_update_plugin_ids(&auth_updates));
        reloaded_plugin_ids.extend(Self::collect_profile_update_plugin_ids(
            &admin_surface_updates,
        ));
        Self::normalize_reloaded_plugin_ids(&mut reloaded_plugin_ids);

        let current_selection = self.current_runtime_selection();
        let current_topology = self.current_protocol_topology_candidate()?;
        let loaded_plugins = {
            let gameplay = self
                .gameplay
                .lock()
                .expect("plugin host mutex should not be poisoned");
            let storage = self
                .storage
                .lock()
                .expect("plugin host mutex should not be poisoned");
            let auth = self
                .auth
                .lock()
                .expect("plugin host mutex should not be poisoned");
            let admin_surface = self
                .admin_surface
                .lock()
                .expect("plugin host mutex should not be poisoned");
            Self::loaded_plugin_set_from_parts(
                current_topology.registry().clone(),
                &gameplay,
                &storage,
                &auth,
                &admin_surface,
            )
        };

        debug_assert_eq!(current_selection, self.current_runtime_selection());
        Ok(StagedRuntimeSelection::new(
            loaded_plugins,
            reloaded_plugin_ids,
            current_topology,
            PreparedRuntimeSelectionState::Artifacts(PreparedArtifactRuntimeSelection {
                protocol_updates,
                gameplay_updates,
                storage_updates,
                auth_updates,
                admin_surface_updates,
            }),
        ))
    }

    pub(crate) fn prepare_runtime_artifacts(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<PreparedRuntimeSelection, RuntimeError> {
        let staged = self.stage_runtime_artifacts()?;
        self.finalize_staged_runtime_selection(staged, runtime)
    }

    pub(crate) fn stage_runtime_selection(
        &self,
        config: &RuntimeSelectionConfig,
    ) -> Result<StagedRuntimeSelection, RuntimeError> {
        let previous_matrix = self.current_runtime_selection().failure_matrix();
        self.failures.update_matrix(config.failure_matrix());
        let staged = (|| {
            let protocols = self.prepare_protocol_topology_for_reload(config)?;
            let gameplay = self.prepare_gameplay_profiles(config, PluginFailureStage::Reload)?;
            let storage = self.prepare_storage_profiles(config, PluginFailureStage::Reload)?;
            let auth = self.prepare_auth_profiles(config, PluginFailureStage::Reload)?;
            let admin_surface =
                self.prepare_admin_surface_profiles(config, PluginFailureStage::Reload)?;

            let loaded_plugins = Self::loaded_plugin_set_from_parts(
                protocols.registry.clone(),
                &gameplay,
                &storage,
                &auth,
                &admin_surface,
            );
            let reloaded_plugin_ids = self.collect_fresh_reloaded_plugin_ids(
                &protocols,
                &gameplay,
                &storage,
                &auth,
                &admin_surface,
            );

            Ok(StagedRuntimeSelection::new(
                loaded_plugins,
                reloaded_plugin_ids,
                RuntimeProtocolTopologyCandidate::new(
                    protocols.clone(),
                    self.requires_protocol_swap(config, &protocols),
                ),
                PreparedRuntimeSelectionState::Fresh(PreparedFreshRuntimeSelection {
                    candidate_config: config.clone(),
                    protocols,
                    gameplay,
                    storage,
                    auth,
                    admin_surface,
                }),
            ))
        })();
        self.failures.update_matrix(previous_matrix);
        staged
    }

    pub(crate) fn prepare_runtime_selection(
        &self,
        config: &RuntimeSelectionConfig,
        runtime: &RuntimeReloadContext,
    ) -> Result<PreparedRuntimeSelection, RuntimeError> {
        let staged = self.stage_runtime_selection(config)?;
        self.finalize_staged_runtime_selection(staged, runtime)
    }

    pub(crate) fn finalize_staged_runtime_selection(
        &self,
        staged: StagedRuntimeSelection,
        runtime: &RuntimeReloadContext,
    ) -> Result<PreparedRuntimeSelection, RuntimeError> {
        let (loaded_plugins, mut reloaded_plugin_ids, protocol_topology, staged_state) =
            staged.into_parts();
        let staged_state = *staged_state
            .downcast::<PreparedRuntimeSelectionState>()
            .expect("staged runtime selection payload type should match");
        match staged_state {
            PreparedRuntimeSelectionState::Fresh(fresh) => {
                self.validate_fresh_protocol_sessions(
                    &fresh.protocols,
                    &runtime.protocol_sessions,
                )?;
                self.validate_fresh_gameplay_sessions(&fresh.gameplay, runtime)?;
                self.validate_fresh_storage_runtime(&fresh.storage, runtime)?;
                Ok(PreparedRuntimeSelection::new(
                    loaded_plugins,
                    reloaded_plugin_ids,
                    protocol_topology,
                    PreparedRuntimeSelectionState::Fresh(fresh),
                ))
            }
            PreparedRuntimeSelectionState::Artifacts(mut artifacts) => {
                artifacts.protocol_updates = self.finalize_protocol_artifact_updates(
                    artifacts.protocol_updates,
                    &runtime.protocol_sessions,
                )?;
                artifacts.gameplay_updates =
                    self.finalize_gameplay_artifact_updates(artifacts.gameplay_updates, runtime)?;
                artifacts.storage_updates =
                    self.finalize_storage_artifact_updates(artifacts.storage_updates, runtime)?;
                artifacts.auth_updates =
                    self.finalize_auth_artifact_updates(artifacts.auth_updates, runtime)?;
                artifacts.admin_surface_updates = self.finalize_admin_surface_artifact_updates(
                    artifacts.admin_surface_updates,
                    runtime,
                )?;
                reloaded_plugin_ids = artifacts
                    .protocol_updates
                    .iter()
                    .map(|update| update.plugin_id.clone())
                    .collect::<Vec<_>>();
                reloaded_plugin_ids.extend(Self::collect_profile_update_plugin_ids(
                    &artifacts.gameplay_updates,
                ));
                reloaded_plugin_ids.extend(Self::collect_profile_update_plugin_ids(
                    &artifacts.storage_updates,
                ));
                reloaded_plugin_ids.extend(Self::collect_profile_update_plugin_ids(
                    &artifacts.auth_updates,
                ));
                reloaded_plugin_ids.extend(Self::collect_profile_update_plugin_ids(
                    &artifacts.admin_surface_updates,
                ));
                Self::normalize_reloaded_plugin_ids(&mut reloaded_plugin_ids);
                Ok(PreparedRuntimeSelection::new(
                    loaded_plugins,
                    reloaded_plugin_ids,
                    protocol_topology,
                    PreparedRuntimeSelectionState::Artifacts(artifacts),
                ))
            }
        }
    }

    pub(crate) fn commit_runtime_selection(&self, prepared: PreparedRuntimeSelection) {
        match prepared.take_staged::<PreparedRuntimeSelectionState>() {
            PreparedRuntimeSelectionState::Fresh(fresh) => {
                let mut cleared_plugin_ids =
                    fresh.protocols.managed.keys().cloned().collect::<Vec<_>>();
                cleared_plugin_ids.extend(
                    fresh
                        .gameplay
                        .values()
                        .map(|managed| managed.package.plugin_id.clone()),
                );
                cleared_plugin_ids.extend(
                    fresh
                        .storage
                        .values()
                        .map(|managed| managed.package.plugin_id.clone()),
                );
                cleared_plugin_ids.extend(
                    fresh
                        .auth
                        .values()
                        .map(|managed| managed.package.plugin_id.clone()),
                );
                cleared_plugin_ids.extend(
                    fresh
                        .admin_surface
                        .values()
                        .map(|managed| managed.package.plugin_id.clone()),
                );
                {
                    let mut runtime_selection = self
                        .runtime_selection
                        .lock()
                        .expect("plugin host mutex should not be poisoned");
                    *runtime_selection = fresh.candidate_config.clone();
                }
                self.activate_protocol_topology(fresh.protocols);
                *self
                    .gameplay
                    .lock()
                    .expect("plugin host mutex should not be poisoned") = fresh.gameplay;
                *self
                    .storage
                    .lock()
                    .expect("plugin host mutex should not be poisoned") = fresh.storage;
                *self
                    .auth
                    .lock()
                    .expect("plugin host mutex should not be poisoned") = fresh.auth;
                *self
                    .admin_surface
                    .lock()
                    .expect("plugin host mutex should not be poisoned") = fresh.admin_surface;
                self.failures
                    .update_matrix(fresh.candidate_config.failure_matrix());
                for plugin_id in cleared_plugin_ids {
                    self.failures.clear_plugin_state(&plugin_id);
                }
            }
            PreparedRuntimeSelectionState::Artifacts(artifacts) => {
                {
                    let mut protocols = self
                        .protocols
                        .lock()
                        .expect("plugin host mutex should not be poisoned");
                    for update in artifacts.protocol_updates {
                        if let Some(managed) = protocols.get_mut(&update.plugin_id) {
                            let _guard = managed
                                .adapter
                                .reload_gate
                                .write()
                                .expect("protocol reload gate should not be poisoned");
                            managed
                                .adapter
                                .swap_generation_while_reloading(update.generation);
                            managed.loaded_at = update.loaded_at;
                            managed.active_loaded_at = update.loaded_at;
                            self.failures.clear_plugin_state(&update.plugin_id);
                        }
                    }
                }
                self.commit_profile_artifact_updates::<GameplayArtifactOps>(
                    artifacts.gameplay_updates,
                );
                self.commit_profile_artifact_updates::<StorageArtifactOps>(
                    artifacts.storage_updates,
                );
                self.commit_profile_artifact_updates::<AuthArtifactOps>(artifacts.auth_updates);
                self.commit_profile_artifact_updates::<AdminSurfaceArtifactOps>(
                    artifacts.admin_surface_updates,
                );
                {}
            }
        }
    }

    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn reload_modified_with_context(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<String>, RuntimeError> {
        let prepared = self.prepare_runtime_artifacts(runtime)?;
        let reloaded = prepared.reloaded_plugin_ids().to_vec();
        self.commit_runtime_selection(prepared);
        Ok(reloaded)
    }
}
