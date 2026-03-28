use super::{
    AdminSurfaceProfileId, Arc, AuthProfileId, GameplayProfileId, HashMap, HashSet,
    HotSwappableAdminSurfaceProfile, HotSwappableAuthProfile, HotSwappableGameplayProfile,
    HotSwappableStorageProfile, ManagedAdminSurfacePlugin, ManagedAuthPlugin,
    ManagedGameplayPlugin, ManagedStoragePlugin, PluginFailureStage, PluginHost, PluginKind,
    PluginPackage, RuntimeError, RuntimeSelectionConfig, StorageProfileId, ensure_known_profiles,
    ensure_profile_known,
};
use crate::config::PluginBufferLimits;
use crate::registry::LoadedPluginSet;

impl PluginHost {
    pub(crate) fn loaded_plugin_set_from_parts(
        protocols: super::ProtocolRegistry,
        gameplay: &HashMap<GameplayProfileId, ManagedGameplayPlugin>,
        storage: &HashMap<StorageProfileId, ManagedStoragePlugin>,
        auth: &HashMap<AuthProfileId, ManagedAuthPlugin>,
        admin_surface: &HashMap<AdminSurfaceProfileId, ManagedAdminSurfacePlugin>,
    ) -> LoadedPluginSet {
        let mut loaded = LoadedPluginSet::new();
        loaded.replace_protocols(protocols);

        for (profile_id, managed) in gameplay {
            loaded.register_gameplay_profile(
                profile_id.clone(),
                Arc::clone(&managed.profile) as Arc<dyn crate::runtime::GameplayProfileHandle>,
            );
        }

        for (profile_id, managed) in storage {
            loaded.register_storage_profile(
                profile_id.clone(),
                Arc::clone(&managed.profile) as Arc<dyn crate::runtime::StorageProfileHandle>,
            );
        }

        for (profile_id, managed) in auth {
            loaded.register_auth_profile(
                profile_id.clone(),
                Arc::clone(&managed.profile) as Arc<dyn crate::runtime::AuthProfileHandle>,
            );
        }

        for (profile_id, managed) in admin_surface {
            loaded.register_admin_surface_profile(
                profile_id.clone(),
                Arc::clone(&managed.profile) as Arc<dyn crate::runtime::AdminSurfaceProfileHandle>,
            );
        }

        loaded
    }

    pub(crate) fn loaded_plugin_set(&self, protocols: super::ProtocolRegistry) -> LoadedPluginSet {
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

        Self::loaded_plugin_set_from_parts(protocols, &gameplay, &storage, &auth, &admin_surface)
    }

    fn required_gameplay_profiles(config: &RuntimeSelectionConfig) -> HashSet<GameplayProfileId> {
        let mut required_profiles = HashSet::new();
        required_profiles.insert(config.default_gameplay_profile.clone());
        required_profiles.extend(config.gameplay_profile_map.values().cloned());
        required_profiles
    }

    pub(crate) fn runtime_auth_profiles(config: &RuntimeSelectionConfig) -> Vec<AuthProfileId> {
        let mut auth_profiles = vec![config.auth_profile.clone()];
        if config.be_enabled && !auth_profiles.contains(&config.bedrock_auth_profile) {
            auth_profiles.push(config.bedrock_auth_profile.clone());
        }
        auth_profiles
    }

    fn requested_auth_profiles(
        auth_profiles: &[AuthProfileId],
    ) -> Result<HashSet<AuthProfileId>, RuntimeError> {
        let requested = auth_profiles
            .iter()
            .filter(|profile_id| !profile_id.as_str().is_empty())
            .cloned()
            .collect::<HashSet<_>>();
        if requested.is_empty() {
            return Err(RuntimeError::Config(
                "at least one auth profile must be activated".to_string(),
            ));
        }
        Ok(requested)
    }

    fn requested_admin_surface_profiles(
        config: &RuntimeSelectionConfig,
    ) -> HashSet<AdminSurfaceProfileId> {
        config
            .admin_surfaces
            .iter()
            .filter(|surface| !surface.profile.as_str().is_empty())
            .map(|surface| surface.profile.clone())
            .collect()
    }

    fn load_requested_gameplay_plugin(
        &self,
        gameplay: &mut HashMap<GameplayProfileId, ManagedGameplayPlugin>,
        package: &PluginPackage,
        required_profiles: &HashSet<GameplayProfileId>,
        config: &RuntimeSelectionConfig,
        stage: PluginFailureStage,
        clear_failure_state: bool,
    ) -> Result<(), RuntimeError> {
        let modified_at = package.modified_at()?;
        let identity = package.artifact_identity(modified_at);
        if self
            .failures
            .is_artifact_quarantined(&package.plugin_id, &identity)
        {
            return Ok(());
        }
        let generation = match self.loader.load_gameplay_generation(
            package,
            self.generations.next_generation_id(),
            config.buffer_limits,
        ) {
            Ok(generation) => Arc::new(generation),
            Err(error) => {
                let reason = error.to_string();
                eprintln!(
                    "gameplay {} load failed for `{}`: {reason}",
                    stage.as_str(),
                    package.plugin_id
                );
                self.failures.handle_candidate_failure(
                    PluginKind::Gameplay,
                    stage,
                    &package.plugin_id,
                    identity,
                    &reason,
                )?;
                return Ok(());
            }
        };
        if !required_profiles.contains(&generation.profile_id) {
            return Ok(());
        }

        let profile_id = generation.profile_id.clone();
        if gameplay.contains_key(&profile_id) {
            return Err(RuntimeError::Config(format!(
                "duplicate gameplay profile `{}` discovered",
                profile_id.as_str()
            )));
        }
        gameplay.insert(
            profile_id.clone(),
            ManagedGameplayPlugin {
                package: package.clone(),
                profile_id: profile_id.clone(),
                profile: Arc::new(HotSwappableGameplayProfile::new(
                    package.plugin_id.clone(),
                    profile_id,
                    generation,
                    Arc::clone(&self.failures),
                )),
                loaded_at: modified_at,
                active_loaded_at: modified_at,
            },
        );
        if clear_failure_state {
            self.failures.clear_plugin_state(&package.plugin_id);
        }
        Ok(())
    }

    fn load_requested_storage_plugin(
        &self,
        storage: &mut HashMap<StorageProfileId, ManagedStoragePlugin>,
        package: &PluginPackage,
        storage_profile: &StorageProfileId,
        buffer_limits: PluginBufferLimits,
        stage: PluginFailureStage,
        clear_failure_state: bool,
    ) -> Result<(), RuntimeError> {
        let modified_at = package.modified_at()?;
        let identity = package.artifact_identity(modified_at);
        if self
            .failures
            .is_artifact_quarantined(&package.plugin_id, &identity)
        {
            return Ok(());
        }
        let generation = match self.loader.load_storage_generation(
            package,
            self.generations.next_generation_id(),
            buffer_limits,
        ) {
            Ok(generation) => Arc::new(generation),
            Err(error) => {
                let reason = error.to_string();
                eprintln!(
                    "storage {} load failed for `{}`: {reason}",
                    stage.as_str(),
                    package.plugin_id
                );
                self.failures.handle_candidate_failure(
                    PluginKind::Storage,
                    stage,
                    &package.plugin_id,
                    identity,
                    &reason,
                )?;
                return Ok(());
            }
        };
        if generation.profile_id != *storage_profile {
            return Ok(());
        }
        if storage.contains_key(storage_profile) {
            return Err(RuntimeError::Config(format!(
                "duplicate storage profile `{storage_profile}` discovered"
            )));
        }
        storage.insert(
            storage_profile.clone(),
            ManagedStoragePlugin {
                package: package.clone(),
                profile_id: storage_profile.clone(),
                profile: Arc::new(HotSwappableStorageProfile::new(
                    package.plugin_id.clone(),
                    generation,
                )),
                loaded_at: modified_at,
                active_loaded_at: modified_at,
            },
        );
        if clear_failure_state {
            self.failures.clear_plugin_state(&package.plugin_id);
        }
        Ok(())
    }

    fn load_requested_auth_plugin(
        &self,
        auth: &mut HashMap<AuthProfileId, ManagedAuthPlugin>,
        package: &PluginPackage,
        requested_profiles: &HashSet<AuthProfileId>,
        buffer_limits: PluginBufferLimits,
        stage: PluginFailureStage,
        clear_failure_state: bool,
    ) -> Result<(), RuntimeError> {
        let modified_at = package.modified_at()?;
        let identity = package.artifact_identity(modified_at);
        if self
            .failures
            .is_artifact_quarantined(&package.plugin_id, &identity)
        {
            return Ok(());
        }
        let generation = match self.loader.load_auth_generation(
            package,
            self.generations.next_generation_id(),
            buffer_limits,
        ) {
            Ok(generation) => Arc::new(generation),
            Err(error) => {
                let reason = error.to_string();
                eprintln!(
                    "auth {} load failed for `{}`: {reason}",
                    stage.as_str(),
                    package.plugin_id
                );
                self.failures.handle_candidate_failure(
                    PluginKind::Auth,
                    stage,
                    &package.plugin_id,
                    identity,
                    &reason,
                )?;
                return Ok(());
            }
        };
        if !requested_profiles.contains(&generation.profile_id) {
            return Ok(());
        }

        let profile_id = generation.profile_id.clone();
        if auth.contains_key(&profile_id) {
            return Err(RuntimeError::Config(format!(
                "duplicate auth profile `{profile_id}` discovered"
            )));
        }
        auth.insert(
            profile_id.clone(),
            ManagedAuthPlugin {
                package: package.clone(),
                profile_id: profile_id.clone(),
                profile: Arc::new(HotSwappableAuthProfile::new(
                    package.plugin_id.clone(),
                    generation,
                    Arc::clone(&self.failures),
                )),
                loaded_at: modified_at,
                active_loaded_at: modified_at,
            },
        );
        if clear_failure_state {
            self.failures.clear_plugin_state(&package.plugin_id);
        }
        Ok(())
    }

    fn load_requested_admin_surface_plugin(
        &self,
        admin_surface: &mut HashMap<AdminSurfaceProfileId, ManagedAdminSurfacePlugin>,
        package: &PluginPackage,
        requested_profiles: &HashSet<AdminSurfaceProfileId>,
        config: &RuntimeSelectionConfig,
        stage: PluginFailureStage,
        clear_failure_state: bool,
    ) -> Result<(), RuntimeError> {
        if requested_profiles.is_empty() {
            return Ok(());
        }
        let modified_at = package.modified_at()?;
        let identity = package.artifact_identity(modified_at);
        if self
            .failures
            .is_artifact_quarantined(&package.plugin_id, &identity)
        {
            return Ok(());
        }
        let generation = match self.loader.load_admin_surface_generation(
            package,
            self.generations.next_generation_id(),
            config.buffer_limits,
        ) {
            Ok(generation) => Arc::new(generation),
            Err(error) => {
                let reason = error.to_string();
                eprintln!(
                    "admin-surface {} load failed for `{}`: {reason}",
                    stage.as_str(),
                    package.plugin_id
                );
                self.failures.handle_candidate_failure(
                    PluginKind::AdminSurface,
                    stage,
                    &package.plugin_id,
                    identity,
                    &reason,
                )?;
                return Ok(());
            }
        };
        if !requested_profiles.contains(&generation.profile_id) {
            return Ok(());
        }

        let profile_id = generation.profile_id.clone();
        if admin_surface.contains_key(&profile_id) {
            return Err(RuntimeError::Config(format!(
                "duplicate admin-surface profile `{}` discovered",
                profile_id.as_str()
            )));
        }
        admin_surface.insert(
            profile_id.clone(),
            ManagedAdminSurfacePlugin {
                package: package.clone(),
                profile_id: profile_id.clone(),
                profile: Arc::new(HotSwappableAdminSurfaceProfile::new(
                    package.plugin_id.clone(),
                    profile_id,
                    generation,
                    Arc::clone(&self.failures),
                )),
                loaded_at: modified_at,
                active_loaded_at: modified_at,
            },
        );
        if clear_failure_state {
            self.failures.clear_plugin_state(&package.plugin_id);
        }
        Ok(())
    }

    pub(crate) fn prepare_gameplay_profiles(
        &self,
        config: &RuntimeSelectionConfig,
        stage: PluginFailureStage,
    ) -> Result<HashMap<GameplayProfileId, ManagedGameplayPlugin>, RuntimeError> {
        let required_profiles = Self::required_gameplay_profiles(config);
        let allowlist = config
            .plugin_allowlist
            .as_ref()
            .map(|entries| entries.iter().cloned().collect::<HashSet<_>>());
        let catalog = self.protocol_catalog()?;
        let mut gameplay = HashMap::new();

        for package in catalog.packages() {
            if package.plugin_kind != PluginKind::Gameplay {
                continue;
            }
            if let Some(allowlist) = allowlist.as_ref()
                && !allowlist.contains(&package.plugin_id)
            {
                continue;
            }
            self.load_requested_gameplay_plugin(
                &mut gameplay,
                package,
                &required_profiles,
                config,
                stage,
                false,
            )?;
        }

        ensure_known_profiles(&gameplay, &required_profiles, "gameplay")?;
        Ok(gameplay)
    }

    pub(crate) fn prepare_storage_profiles(
        &self,
        config: &RuntimeSelectionConfig,
        stage: PluginFailureStage,
    ) -> Result<HashMap<StorageProfileId, ManagedStoragePlugin>, RuntimeError> {
        let allowlist = config
            .plugin_allowlist
            .as_ref()
            .map(|entries| entries.iter().cloned().collect::<HashSet<_>>());
        let catalog = self.protocol_catalog()?;
        let mut storage = HashMap::new();

        for package in catalog.packages() {
            if package.plugin_kind != PluginKind::Storage {
                continue;
            }
            if let Some(allowlist) = allowlist.as_ref()
                && !allowlist.contains(&package.plugin_id)
            {
                continue;
            }
            self.load_requested_storage_plugin(
                &mut storage,
                package,
                &self.bootstrap_config.storage_profile,
                config.buffer_limits,
                stage,
                false,
            )?;
        }

        ensure_profile_known(&storage, &self.bootstrap_config.storage_profile, "storage")?;
        Ok(storage)
    }

    pub(crate) fn prepare_auth_profiles(
        &self,
        config: &RuntimeSelectionConfig,
        stage: PluginFailureStage,
    ) -> Result<HashMap<AuthProfileId, ManagedAuthPlugin>, RuntimeError> {
        let requested = Self::requested_auth_profiles(&Self::runtime_auth_profiles(config))?;
        let allowlist = config
            .plugin_allowlist
            .as_ref()
            .map(|entries| entries.iter().cloned().collect::<HashSet<_>>());
        let catalog = self.protocol_catalog()?;
        let mut auth = HashMap::new();

        for package in catalog.packages() {
            if package.plugin_kind != PluginKind::Auth {
                continue;
            }
            if let Some(allowlist) = allowlist.as_ref()
                && !allowlist.contains(&package.plugin_id)
            {
                continue;
            }
            self.load_requested_auth_plugin(
                &mut auth,
                package,
                &requested,
                config.buffer_limits,
                stage,
                false,
            )?;
        }

        ensure_known_profiles(&auth, &requested, "auth")?;
        Ok(auth)
    }

    pub(crate) fn prepare_admin_surface_profiles(
        &self,
        config: &RuntimeSelectionConfig,
        stage: PluginFailureStage,
    ) -> Result<HashMap<AdminSurfaceProfileId, ManagedAdminSurfacePlugin>, RuntimeError> {
        let requested_profiles = Self::requested_admin_surface_profiles(config);
        let allowlist = config
            .plugin_allowlist
            .as_ref()
            .map(|entries| entries.iter().cloned().collect::<HashSet<_>>());
        let catalog = self.protocol_catalog()?;
        let mut admin_surface = HashMap::new();

        for package in catalog.packages() {
            if package.plugin_kind != PluginKind::AdminSurface {
                continue;
            }
            if let Some(allowlist) = allowlist.as_ref()
                && !allowlist.contains(&package.plugin_id)
            {
                continue;
            }
            self.load_requested_admin_surface_plugin(
                &mut admin_surface,
                package,
                &requested_profiles,
                config,
                stage,
                false,
            )?;
        }

        ensure_known_profiles(&admin_surface, &requested_profiles, "admin-surface")?;
        Ok(admin_surface)
    }

    /// Activates every gameplay profile required by the current server configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when a required profile cannot be loaded, is duplicated, or is unknown.
    ///
    /// # Panics
    ///
    /// Panics if the gameplay plugin registry mutex is poisoned.
    pub(crate) fn activate_gameplay_profiles(
        &self,
        config: &RuntimeSelectionConfig,
    ) -> Result<(), RuntimeError> {
        let required_profiles = Self::required_gameplay_profiles(config);
        let allowlist = config
            .plugin_allowlist
            .as_ref()
            .map(|entries| entries.iter().cloned().collect::<HashSet<_>>());
        let catalog = self.protocol_catalog()?;

        let mut gameplay = self
            .gameplay
            .lock()
            .expect("plugin host mutex should not be poisoned");
        gameplay.clear();

        for package in catalog.packages() {
            if package.plugin_kind != PluginKind::Gameplay {
                continue;
            }
            if let Some(allowlist) = allowlist.as_ref()
                && !allowlist.contains(&package.plugin_id)
            {
                continue;
            }
            self.load_requested_gameplay_plugin(
                &mut gameplay,
                package,
                &required_profiles,
                config,
                PluginFailureStage::Boot,
                true,
            )?;
        }

        let result = ensure_known_profiles(&gameplay, &required_profiles, "gameplay");
        drop(gameplay);
        result
    }

    /// Activates the configured storage profile.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested storage profile cannot be loaded, is duplicated, or
    /// is unknown.
    ///
    /// # Panics
    ///
    /// Panics if the storage plugin registry mutex is poisoned.
    pub(crate) fn activate_storage_profile(
        &self,
        storage_profile: &StorageProfileId,
    ) -> Result<(), RuntimeError> {
        let allowlist = self
            .current_runtime_selection()
            .plugin_allowlist
            .map(|entries| entries.into_iter().collect::<HashSet<_>>());
        let buffer_limits = self.current_runtime_selection().buffer_limits;
        let catalog = self.protocol_catalog()?;
        let mut storage = self
            .storage
            .lock()
            .expect("plugin host mutex should not be poisoned");
        storage.clear();

        for package in catalog.packages() {
            if package.plugin_kind != PluginKind::Storage {
                continue;
            }
            if let Some(allowlist) = allowlist.as_ref()
                && !allowlist.contains(&package.plugin_id)
            {
                continue;
            }
            self.load_requested_storage_plugin(
                &mut storage,
                package,
                storage_profile,
                buffer_limits,
                PluginFailureStage::Boot,
                true,
            )?;
        }

        let result = ensure_profile_known(&storage, storage_profile, "storage");
        drop(storage);
        result
    }

    /// Activates the requested auth profiles.
    ///
    /// # Errors
    ///
    /// Returns an error when no profiles are requested, when a requested profile cannot be
    /// loaded, is duplicated, or is unknown.
    ///
    /// # Panics
    ///
    /// Panics if the auth plugin registry mutex is poisoned.
    pub(crate) fn activate_auth_profiles(
        &self,
        auth_profiles: &[AuthProfileId],
    ) -> Result<(), RuntimeError> {
        let requested = Self::requested_auth_profiles(auth_profiles)?;
        let allowlist = self
            .current_runtime_selection()
            .plugin_allowlist
            .map(|entries| entries.into_iter().collect::<HashSet<_>>());
        let buffer_limits = self.current_runtime_selection().buffer_limits;
        let catalog = self.protocol_catalog()?;
        let mut auth = self
            .auth
            .lock()
            .expect("plugin host mutex should not be poisoned");
        auth.clear();

        for package in catalog.packages() {
            if package.plugin_kind != PluginKind::Auth {
                continue;
            }
            if let Some(allowlist) = allowlist.as_ref()
                && !allowlist.contains(&package.plugin_id)
            {
                continue;
            }
            self.load_requested_auth_plugin(
                &mut auth,
                package,
                &requested,
                buffer_limits,
                PluginFailureStage::Boot,
                true,
            )?;
        }

        let result = ensure_known_profiles(&auth, &requested, "auth");
        drop(auth);
        result
    }

    /// Activates the admin surface profiles referenced by the runtime configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when duplicate matching admin surface profiles are discovered or when a
    /// referenced profile is unavailable.
    pub(crate) fn activate_admin_surface_profiles(
        &self,
        config: &RuntimeSelectionConfig,
    ) -> Result<(), RuntimeError> {
        let requested_profiles = Self::requested_admin_surface_profiles(config);
        let allowlist = self
            .current_runtime_selection()
            .plugin_allowlist
            .map(|entries| entries.into_iter().collect::<HashSet<_>>());
        let catalog = self.protocol_catalog()?;
        let mut admin_surface = self
            .admin_surface
            .lock()
            .expect("plugin host mutex should not be poisoned");
        admin_surface.clear();

        for package in catalog.packages() {
            if package.plugin_kind != PluginKind::AdminSurface {
                continue;
            }
            if let Some(allowlist) = allowlist.as_ref()
                && !allowlist.contains(&package.plugin_id)
            {
                continue;
            }
            self.load_requested_admin_surface_plugin(
                &mut admin_surface,
                package,
                &requested_profiles,
                config,
                PluginFailureStage::Boot,
                true,
            )?;
        }

        let result = ensure_known_profiles(&admin_surface, &requested_profiles, "admin-surface");
        drop(admin_surface);
        result
    }

    /// Activates a single auth profile.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested auth profile cannot be activated.
    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn activate_auth_profile(&self, auth_profile: &str) -> Result<(), RuntimeError> {
        self.activate_auth_profiles(&[AuthProfileId::new(auth_profile)])
    }

    /// Activates gameplay, storage, and auth profiles needed by the runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when any configured runtime profile cannot be activated.
    pub(crate) fn activate_runtime_profiles(
        &self,
        config: &RuntimeSelectionConfig,
    ) -> Result<(), RuntimeError> {
        self.activate_gameplay_profiles(config)?;
        self.activate_storage_profile(&self.bootstrap_config.storage_profile)?;
        self.activate_auth_profiles(&Self::runtime_auth_profiles(config))?;
        self.activate_admin_surface_profiles(config)
    }

    /// Loads protocol adapters and activates runtime-selected profiles, then snapshots the active
    /// plugin set for server boot.
    ///
    /// # Errors
    ///
    /// Returns an error when protocol topology or required runtime profiles cannot be loaded.
    pub fn load_plugin_set(
        self: &Arc<Self>,
        config: &RuntimeSelectionConfig,
    ) -> Result<LoadedPluginSet, RuntimeError> {
        self.failures.update_matrix(config.failure_matrix());
        {
            let mut runtime_selection = self
                .runtime_selection
                .lock()
                .expect("plugin host mutex should not be poisoned");
            *runtime_selection = config.clone();
        }
        let protocols = self.load_protocol_registry(config)?;
        self.activate_runtime_profiles(config)?;
        Ok(self.loaded_plugin_set(protocols))
    }

    /// Loads and activates the protocol registry snapshot used for initial server boot.
    ///
    /// # Errors
    ///
    /// Returns an error when the protocol topology cannot be prepared.
    pub(crate) fn load_protocol_registry(
        self: &Arc<Self>,
        config: &RuntimeSelectionConfig,
    ) -> Result<super::ProtocolRegistry, RuntimeError> {
        let prepared = self.prepare_protocol_topology_for_boot(config)?;
        let registry = prepared.registry.clone();
        self.activate_protocol_topology(prepared);
        Ok(registry)
    }
}
