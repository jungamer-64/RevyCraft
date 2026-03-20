use super::{
    Arc, HashMap, HashSet, HotSwappableAuthProfile, HotSwappableGameplayProfile,
    HotSwappableStorageProfile, ManagedAuthPlugin, ManagedGameplayPlugin, ManagedStoragePlugin,
    PluginHost, PluginKind, PluginPackage, RuntimeError, ServerConfig, ensure_known_profiles,
    ensure_profile_known,
};
use crate::registry::LoadedPluginSet;

impl PluginHost {
    fn loaded_plugin_set(&self, protocols: super::ProtocolRegistry) -> LoadedPluginSet {
        let mut loaded = LoadedPluginSet::new();
        loaded.replace_protocols(protocols);

        {
            let gameplay = self
                .gameplay
                .lock()
                .expect("plugin host mutex should not be poisoned");
            for (profile_id, managed) in gameplay.iter() {
                loaded.register_gameplay_profile(profile_id.clone(), Arc::clone(&managed.profile));
            }
        }

        {
            let storage = self
                .storage
                .lock()
                .expect("plugin host mutex should not be poisoned");
            for (profile_id, managed) in storage.iter() {
                loaded.register_storage_profile(profile_id.clone(), Arc::clone(&managed.profile));
            }
        }

        {
            let auth = self
                .auth
                .lock()
                .expect("plugin host mutex should not be poisoned");
            for (profile_id, managed) in auth.iter() {
                loaded.register_auth_profile(profile_id.clone(), Arc::clone(&managed.profile));
            }
        }

        loaded
    }

    fn required_gameplay_profiles(config: &ServerConfig) -> HashSet<String> {
        let mut required_profiles = HashSet::new();
        required_profiles.insert(config.default_gameplay_profile.clone());
        required_profiles.extend(config.gameplay_profile_map.values().cloned());
        required_profiles
    }

    fn runtime_auth_profiles(config: &ServerConfig) -> Vec<String> {
        let mut auth_profiles = vec![config.auth_profile.clone()];
        if config.be_enabled && !auth_profiles.contains(&config.bedrock_auth_profile) {
            auth_profiles.push(config.bedrock_auth_profile.clone());
        }
        auth_profiles
    }

    fn requested_auth_profiles(auth_profiles: &[String]) -> Result<HashSet<String>, RuntimeError> {
        let requested = auth_profiles
            .iter()
            .filter(|profile_id| !profile_id.is_empty())
            .cloned()
            .collect::<HashSet<_>>();
        if requested.is_empty() {
            return Err(RuntimeError::Config(
                "at least one auth profile must be activated".to_string(),
            ));
        }
        Ok(requested)
    }

    fn load_requested_gameplay_plugin(
        &self,
        gameplay: &mut HashMap<String, ManagedGameplayPlugin>,
        package: &PluginPackage,
        required_profiles: &HashSet<String>,
    ) -> Result<(), RuntimeError> {
        let generation = Arc::new(
            self.loader
                .load_gameplay_generation(package, self.generations.next_generation_id())?,
        );
        if !required_profiles.contains(generation.profile_id.as_str()) {
            return Ok(());
        }

        let profile_id = generation.profile_id.clone();
        if gameplay.contains_key(profile_id.as_str()) {
            return Err(RuntimeError::Config(format!(
                "duplicate gameplay profile `{}` discovered",
                profile_id.as_str()
            )));
        }
        gameplay.insert(
            profile_id.as_str().to_string(),
            ManagedGameplayPlugin {
                package: package.clone(),
                profile_id: profile_id.clone(),
                profile: Arc::new(HotSwappableGameplayProfile::new(
                    package.plugin_id.clone(),
                    profile_id,
                    generation,
                    Arc::clone(&self.failures),
                )),
                loaded_at: package.modified_at()?,
                active_loaded_at: package.modified_at()?,
            },
        );
        self.failures.clear_plugin_state(&package.plugin_id);
        Ok(())
    }

    fn load_requested_storage_plugin(
        &self,
        storage: &mut HashMap<String, ManagedStoragePlugin>,
        package: &PluginPackage,
        storage_profile: &str,
    ) -> Result<(), RuntimeError> {
        let generation = Arc::new(
            self.loader
                .load_storage_generation(package, self.generations.next_generation_id())?,
        );
        if generation.profile_id != storage_profile {
            return Ok(());
        }
        if storage.contains_key(storage_profile) {
            return Err(RuntimeError::Config(format!(
                "duplicate storage profile `{storage_profile}` discovered"
            )));
        }
        storage.insert(
            storage_profile.to_string(),
            ManagedStoragePlugin {
                package: package.clone(),
                profile_id: storage_profile.to_string(),
                profile: Arc::new(HotSwappableStorageProfile::new(
                    package.plugin_id.clone(),
                    storage_profile.to_string(),
                    generation,
                )),
                loaded_at: package.modified_at()?,
                active_loaded_at: package.modified_at()?,
            },
        );
        self.failures.clear_plugin_state(&package.plugin_id);
        Ok(())
    }

    fn load_requested_auth_plugin(
        &self,
        auth: &mut HashMap<String, ManagedAuthPlugin>,
        package: &PluginPackage,
        requested_profiles: &HashSet<String>,
    ) -> Result<(), RuntimeError> {
        let generation = Arc::new(
            self.loader
                .load_auth_generation(package, self.generations.next_generation_id())?,
        );
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
                    profile_id,
                    generation,
                    Arc::clone(&self.failures),
                )),
                loaded_at: package.modified_at()?,
                active_loaded_at: package.modified_at()?,
            },
        );
        self.failures.clear_plugin_state(&package.plugin_id);
        Ok(())
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
    pub fn activate_gameplay_profiles(&self, config: &ServerConfig) -> Result<(), RuntimeError> {
        let required_profiles = Self::required_gameplay_profiles(config);

        let mut gameplay = self
            .gameplay
            .lock()
            .expect("plugin host mutex should not be poisoned");
        gameplay.clear();

        for package in self.catalog.packages() {
            if package.plugin_kind != PluginKind::Gameplay {
                continue;
            }
            self.load_requested_gameplay_plugin(&mut gameplay, package, &required_profiles)?;
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
    pub fn activate_storage_profile(&self, storage_profile: &str) -> Result<(), RuntimeError> {
        let mut storage = self
            .storage
            .lock()
            .expect("plugin host mutex should not be poisoned");
        storage.clear();

        for package in self.catalog.packages() {
            if package.plugin_kind != PluginKind::Storage {
                continue;
            }
            self.load_requested_storage_plugin(&mut storage, package, storage_profile)?;
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
    pub fn activate_auth_profiles(&self, auth_profiles: &[String]) -> Result<(), RuntimeError> {
        let requested = Self::requested_auth_profiles(auth_profiles)?;
        let mut auth = self
            .auth
            .lock()
            .expect("plugin host mutex should not be poisoned");
        auth.clear();

        for package in self.catalog.packages() {
            if package.plugin_kind != PluginKind::Auth {
                continue;
            }
            self.load_requested_auth_plugin(&mut auth, package, &requested)?;
        }

        let result = ensure_known_profiles(&auth, &requested, "auth");
        drop(auth);
        result
    }

    /// Activates a single auth profile.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested auth profile cannot be activated.
    pub fn activate_auth_profile(&self, auth_profile: &str) -> Result<(), RuntimeError> {
        self.activate_auth_profiles(&[auth_profile.to_string()])
    }

    /// Activates gameplay, storage, and auth profiles needed by the runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when any configured runtime profile cannot be activated.
    pub fn activate_runtime_profiles(&self, config: &ServerConfig) -> Result<(), RuntimeError> {
        self.activate_gameplay_profiles(config)?;
        self.activate_storage_profile(&config.storage_profile)?;
        self.activate_auth_profiles(&Self::runtime_auth_profiles(config))
    }

    /// Loads protocol adapters and activates runtime-selected profiles, then snapshots the active
    /// plugin set for server boot.
    ///
    /// # Errors
    ///
    /// Returns an error when protocol topology or required runtime profiles cannot be loaded.
    pub fn load_plugin_set(
        self: &Arc<Self>,
        config: &ServerConfig,
    ) -> Result<LoadedPluginSet, RuntimeError> {
        let protocols = self.load_protocol_registry()?;
        self.activate_runtime_profiles(config)?;
        let mut loaded = self.loaded_plugin_set(protocols);
        loaded.attach_plugin_host(Arc::clone(self));
        Ok(loaded)
    }

    /// Loads and activates the protocol registry snapshot used for initial server boot.
    ///
    /// # Errors
    ///
    /// Returns an error when the protocol topology cannot be prepared.
    pub fn load_protocol_registry(
        self: &Arc<Self>,
    ) -> Result<super::ProtocolRegistry, RuntimeError> {
        let prepared = self.prepare_protocol_topology_for_boot()?;
        let registry = prepared.registry.clone();
        self.activate_protocol_topology(prepared);
        Ok(registry)
    }
}
