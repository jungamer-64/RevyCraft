use super::{
    AdminUiGeneration, Arc, AuthGeneration, GameplayGeneration, ManagedAdminUiPlugin,
    ManagedAuthPlugin, ManagedGameplayPlugin, ManagedStoragePlugin, PluginFailureStage, PluginHost,
    PluginKind, PreparedProtocolTopology, RuntimeError, RuntimeReloadContext,
    RuntimeSelectionConfig, StorageGeneration, SystemTime, import_storage_runtime_state,
    protocol_reload_compatible, validate_gameplay_session_migration,
    validate_protocol_session_migration,
};
use crate::runtime::ProtocolReloadSession;
use crate::runtime::{PreparedRuntimeSelection, RuntimeProtocolTopologyCandidate};
use std::collections::HashMap;

struct PreparedFreshRuntimeSelection {
    candidate_config: RuntimeSelectionConfig,
    protocols: PreparedProtocolTopology,
    gameplay: HashMap<mc_core::GameplayProfileId, ManagedGameplayPlugin>,
    storage: HashMap<mc_core::StorageProfileId, ManagedStoragePlugin>,
    auth: HashMap<mc_core::AuthProfileId, ManagedAuthPlugin>,
    admin_ui: HashMap<mc_core::AdminUiProfileId, ManagedAdminUiPlugin>,
}

struct PreparedProtocolArtifactUpdate {
    plugin_id: String,
    loaded_at: SystemTime,
    generation: Arc<super::ProtocolGeneration>,
}

struct PreparedGameplayArtifactUpdate {
    plugin_id: String,
    loaded_at: SystemTime,
    generation: Arc<GameplayGeneration>,
}

struct PreparedStorageArtifactUpdate {
    plugin_id: String,
    profile_id: mc_core::StorageProfileId,
    loaded_at: SystemTime,
    generation: Arc<StorageGeneration>,
}

struct PreparedAuthArtifactUpdate {
    plugin_id: String,
    profile_id: mc_core::AuthProfileId,
    loaded_at: SystemTime,
    generation: Arc<AuthGeneration>,
}

struct PreparedAdminUiArtifactUpdate {
    plugin_id: String,
    profile_id: mc_core::AdminUiProfileId,
    loaded_at: SystemTime,
    generation: Arc<AdminUiGeneration>,
}

struct PreparedArtifactRuntimeSelection {
    protocol_updates: Vec<PreparedProtocolArtifactUpdate>,
    gameplay_updates: Vec<PreparedGameplayArtifactUpdate>,
    storage_updates: Vec<PreparedStorageArtifactUpdate>,
    auth_updates: Vec<PreparedAuthArtifactUpdate>,
    admin_ui_updates: Vec<PreparedAdminUiArtifactUpdate>,
}

enum PreparedRuntimeSelectionState {
    Fresh(PreparedFreshRuntimeSelection),
    Artifacts(PreparedArtifactRuntimeSelection),
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
        gameplay: &HashMap<mc_core::GameplayProfileId, ManagedGameplayPlugin>,
        storage: &HashMap<mc_core::StorageProfileId, ManagedStoragePlugin>,
        auth: &HashMap<mc_core::AuthProfileId, ManagedAuthPlugin>,
        admin_ui: &HashMap<mc_core::AdminUiProfileId, ManagedAdminUiPlugin>,
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
        let active_admin_ui = self
            .admin_ui
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
        for candidate in admin_ui.values() {
            if active_admin_ui
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
        candidate: &HashMap<mc_core::GameplayProfileId, ManagedGameplayPlugin>,
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
        candidate: &HashMap<mc_core::StorageProfileId, ManagedStoragePlugin>,
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

    fn prepare_protocol_artifact_updates(
        &self,
        protocol_sessions: &[ProtocolReloadSession],
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
            let current_generation = managed
                .adapter
                .current_generation()
                .map_err(|error| RuntimeError::Config(error.to_string()))?;
            if !protocol_reload_compatible(
                &managed.package.plugin_id,
                &current_generation,
                &generation,
            ) {
                self.failures.handle_candidate_failure(
                    PluginKind::Protocol,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    "protocol topology changed during reload",
                )?;
                continue;
            }
            if !validate_protocol_session_migration(managed, &generation, protocol_sessions)? {
                self.failures.handle_candidate_failure(
                    PluginKind::Protocol,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    "protocol session migration failed",
                )?;
                continue;
            }
            updates.push(PreparedProtocolArtifactUpdate {
                plugin_id: managed.package.plugin_id.clone(),
                loaded_at: modified_at,
                generation,
            });
        }
        Ok(updates)
    }

    fn prepare_gameplay_artifact_updates(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<PreparedGameplayArtifactUpdate>, RuntimeError> {
        let mut updates = Vec::new();
        let mut gameplay = self
            .gameplay
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for managed in gameplay.values_mut() {
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
            let generation = match self.loader.load_gameplay_generation(
                &managed.package,
                self.generations.next_generation_id(),
                self.current_runtime_selection().buffer_limits,
            ) {
                Ok(generation) => Arc::new(generation),
                Err(error) => {
                    self.failures.handle_candidate_failure(
                        PluginKind::Gameplay,
                        PluginFailureStage::Reload,
                        &managed.package.plugin_id,
                        identity,
                        &error.to_string(),
                    )?;
                    continue;
                }
            };
            if generation.profile_id != managed.profile_id {
                self.failures.handle_candidate_failure(
                    PluginKind::Gameplay,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    &format!(
                        "gameplay plugin `{}` changed profile from `{}` to `{}` during reload",
                        managed.package.plugin_id,
                        managed.profile_id.as_str(),
                        generation.profile_id.as_str()
                    ),
                )?;
                continue;
            }
            if !validate_gameplay_session_migration(managed, &generation, runtime)? {
                self.failures.handle_candidate_failure(
                    PluginKind::Gameplay,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    "gameplay session migration failed",
                )?;
                continue;
            }
            updates.push(PreparedGameplayArtifactUpdate {
                plugin_id: managed.package.plugin_id.clone(),
                loaded_at: modified_at,
                generation,
            });
        }
        Ok(updates)
    }

    fn prepare_storage_artifact_updates(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<PreparedStorageArtifactUpdate>, RuntimeError> {
        let mut updates = Vec::new();
        let mut storage = self
            .storage
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for managed in storage.values_mut() {
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
            let generation = match self.loader.load_storage_generation(
                &managed.package,
                self.generations.next_generation_id(),
                self.current_runtime_selection().buffer_limits,
            ) {
                Ok(generation) => Arc::new(generation),
                Err(error) => {
                    self.failures.handle_candidate_failure(
                        PluginKind::Storage,
                        PluginFailureStage::Reload,
                        &managed.package.plugin_id,
                        identity,
                        &error.to_string(),
                    )?;
                    continue;
                }
            };
            if generation.profile_id != managed.profile_id {
                self.failures.handle_candidate_failure(
                    PluginKind::Storage,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    &format!(
                        "storage plugin `{}` changed profile from `{}` to `{}` during reload",
                        managed.package.plugin_id, managed.profile_id, generation.profile_id
                    ),
                )?;
                continue;
            }
            if !import_storage_runtime_state(&managed.package.plugin_id, &generation, runtime) {
                self.failures.handle_candidate_failure(
                    PluginKind::Storage,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    "storage runtime state import failed",
                )?;
                continue;
            }
            updates.push(PreparedStorageArtifactUpdate {
                plugin_id: managed.package.plugin_id.clone(),
                profile_id: managed.profile_id.clone(),
                loaded_at: modified_at,
                generation,
            });
        }
        Ok(updates)
    }

    fn prepare_auth_artifact_updates(
        &self,
    ) -> Result<Vec<PreparedAuthArtifactUpdate>, RuntimeError> {
        let mut updates = Vec::new();
        let mut auth = self
            .auth
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for managed in auth.values_mut() {
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
            let generation = match self.loader.load_auth_generation(
                &managed.package,
                self.generations.next_generation_id(),
                self.current_runtime_selection().buffer_limits,
            ) {
                Ok(generation) => Arc::new(generation),
                Err(error) => {
                    self.failures.handle_candidate_failure(
                        PluginKind::Auth,
                        PluginFailureStage::Reload,
                        &managed.package.plugin_id,
                        identity,
                        &error.to_string(),
                    )?;
                    continue;
                }
            };
            if generation.profile_id != managed.profile_id {
                self.failures.handle_candidate_failure(
                    PluginKind::Auth,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    &format!(
                        "auth plugin `{}` changed profile from `{}` to `{}` during reload",
                        managed.package.plugin_id, managed.profile_id, generation.profile_id
                    ),
                )?;
                continue;
            }
            updates.push(PreparedAuthArtifactUpdate {
                plugin_id: managed.package.plugin_id.clone(),
                profile_id: managed.profile_id.clone(),
                loaded_at: modified_at,
                generation,
            });
        }
        Ok(updates)
    }

    fn prepare_admin_ui_artifact_updates(
        &self,
    ) -> Result<Vec<PreparedAdminUiArtifactUpdate>, RuntimeError> {
        let mut updates = Vec::new();
        let mut admin_ui = self
            .admin_ui
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for managed in admin_ui.values_mut() {
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
            let generation = match self.loader.load_admin_ui_generation(
                &managed.package,
                self.generations.next_generation_id(),
                self.current_runtime_selection().buffer_limits,
            ) {
                Ok(generation) => Arc::new(generation),
                Err(error) => {
                    self.failures.handle_candidate_failure(
                        PluginKind::AdminUi,
                        PluginFailureStage::Reload,
                        &managed.package.plugin_id,
                        identity,
                        &error.to_string(),
                    )?;
                    continue;
                }
            };
            if generation.profile_id != managed.profile_id {
                self.failures.handle_candidate_failure(
                    PluginKind::AdminUi,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    &format!(
                        "admin-ui plugin `{}` changed profile from `{}` to `{}` during reload",
                        managed.package.plugin_id, managed.profile_id, generation.profile_id
                    ),
                )?;
                continue;
            }
            updates.push(PreparedAdminUiArtifactUpdate {
                plugin_id: managed.package.plugin_id.clone(),
                profile_id: managed.profile_id.clone(),
                loaded_at: modified_at,
                generation,
            });
        }
        Ok(updates)
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
            let admin_ui = self
                .admin_ui
                .lock()
                .expect("plugin host mutex should not be poisoned");
            Self::loaded_plugin_set_from_parts(
                current_topology.registry().clone(),
                &gameplay,
                &storage,
                &auth,
                &admin_ui,
            )
        };
        let protocol_updates = self.prepare_protocol_artifact_updates(&[])?;
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
                admin_ui_updates: Vec::new(),
            }),
        );
        self.commit_runtime_selection(prepared);
        Ok(reloaded_plugin_ids)
    }

    pub(crate) fn prepare_runtime_artifacts(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<PreparedRuntimeSelection, RuntimeError> {
        let protocol_updates =
            self.prepare_protocol_artifact_updates(&runtime.protocol_sessions)?;
        let gameplay_updates = self.prepare_gameplay_artifact_updates(runtime)?;
        let storage_updates = self.prepare_storage_artifact_updates(runtime)?;
        let auth_updates = self.prepare_auth_artifact_updates()?;
        let admin_ui_updates = self.prepare_admin_ui_artifact_updates()?;
        let mut reloaded_plugin_ids = protocol_updates
            .iter()
            .map(|update| update.plugin_id.clone())
            .collect::<Vec<_>>();
        reloaded_plugin_ids.extend(
            gameplay_updates
                .iter()
                .map(|update| update.plugin_id.clone()),
        );
        reloaded_plugin_ids.extend(
            storage_updates
                .iter()
                .map(|update| update.plugin_id.clone()),
        );
        reloaded_plugin_ids.extend(auth_updates.iter().map(|update| update.plugin_id.clone()));
        reloaded_plugin_ids.extend(
            admin_ui_updates
                .iter()
                .map(|update| update.plugin_id.clone()),
        );
        reloaded_plugin_ids.sort();
        reloaded_plugin_ids.dedup();

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
            let admin_ui = self
                .admin_ui
                .lock()
                .expect("plugin host mutex should not be poisoned");
            Self::loaded_plugin_set_from_parts(
                current_topology.registry().clone(),
                &gameplay,
                &storage,
                &auth,
                &admin_ui,
            )
        };

        debug_assert_eq!(current_selection, self.current_runtime_selection());
        Ok(PreparedRuntimeSelection::new(
            loaded_plugins,
            reloaded_plugin_ids,
            current_topology,
            PreparedRuntimeSelectionState::Artifacts(PreparedArtifactRuntimeSelection {
                protocol_updates,
                gameplay_updates,
                storage_updates,
                auth_updates,
                admin_ui_updates,
            }),
        ))
    }

    pub(crate) fn prepare_runtime_selection(
        &self,
        config: &RuntimeSelectionConfig,
        runtime: &RuntimeReloadContext,
    ) -> Result<PreparedRuntimeSelection, RuntimeError> {
        let previous_matrix = self.current_runtime_selection().failure_matrix();
        self.failures.update_matrix(config.failure_matrix());
        let prepared = (|| {
            let protocols = self.prepare_protocol_topology_for_reload(config)?;
            let gameplay = self.prepare_gameplay_profiles(config, PluginFailureStage::Reload)?;
            let storage = self.prepare_storage_profiles(config, PluginFailureStage::Reload)?;
            let auth = self.prepare_auth_profiles(config, PluginFailureStage::Reload)?;
            let admin_ui = self.prepare_admin_ui_profiles(config, PluginFailureStage::Reload)?;

            self.validate_fresh_protocol_sessions(&protocols, &runtime.protocol_sessions)?;
            self.validate_fresh_gameplay_sessions(&gameplay, runtime)?;
            self.validate_fresh_storage_runtime(&storage, runtime)?;

            let loaded_plugins = Self::loaded_plugin_set_from_parts(
                protocols.registry.clone(),
                &gameplay,
                &storage,
                &auth,
                &admin_ui,
            );
            let reloaded_plugin_ids = self.collect_fresh_reloaded_plugin_ids(
                &protocols, &gameplay, &storage, &auth, &admin_ui,
            );

            Ok(PreparedRuntimeSelection::new(
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
                    admin_ui,
                }),
            ))
        })();
        self.failures.update_matrix(previous_matrix);
        prepared
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
                        .admin_ui
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
                    .admin_ui
                    .lock()
                    .expect("plugin host mutex should not be poisoned") = fresh.admin_ui;
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
                {
                    let mut gameplay = self
                        .gameplay
                        .lock()
                        .expect("plugin host mutex should not be poisoned");
                    for update in artifacts.gameplay_updates {
                        if let Some(managed) = gameplay
                            .values_mut()
                            .find(|managed| managed.package.plugin_id == update.plugin_id)
                        {
                            managed.profile.swap_generation(update.generation);
                            managed.loaded_at = update.loaded_at;
                            managed.active_loaded_at = update.loaded_at;
                            self.failures.clear_plugin_state(&update.plugin_id);
                        }
                    }
                }
                {
                    let mut storage = self
                        .storage
                        .lock()
                        .expect("plugin host mutex should not be poisoned");
                    for update in artifacts.storage_updates {
                        if let Some(managed) = storage.get_mut(&update.profile_id) {
                            let profile = Arc::clone(&managed.profile);
                            let generation = Arc::clone(&update.generation);
                            profile.with_reload_write(|_| {
                                profile.swap_generation_while_reloading(generation);
                            });
                            managed.loaded_at = update.loaded_at;
                            managed.active_loaded_at = update.loaded_at;
                            self.failures.clear_plugin_state(&update.plugin_id);
                        }
                    }
                }
                {
                    let mut auth = self
                        .auth
                        .lock()
                        .expect("plugin host mutex should not be poisoned");
                    for update in artifacts.auth_updates {
                        if let Some(managed) = auth.get_mut(&update.profile_id) {
                            managed.profile.swap_generation(update.generation);
                            managed.loaded_at = update.loaded_at;
                            managed.active_loaded_at = update.loaded_at;
                            self.failures.clear_plugin_state(&update.plugin_id);
                        }
                    }
                }
                {
                    let mut admin_ui = self
                        .admin_ui
                        .lock()
                        .expect("plugin host mutex should not be poisoned");
                    for update in artifacts.admin_ui_updates {
                        if let Some(managed) = admin_ui.get_mut(&update.profile_id) {
                            managed.profile.swap_generation(update.generation);
                            managed.loaded_at = update.loaded_at;
                            managed.active_loaded_at = update.loaded_at;
                            self.failures.clear_plugin_state(&update.plugin_id);
                        }
                    }
                }
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
