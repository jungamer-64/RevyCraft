use super::{
    Arc, HashMap, HashSet, HotSwappableAuthProfile, HotSwappableGameplayProfile,
    HotSwappableStorageProfile, ManagedAuthPlugin, ManagedGameplayPlugin, ManagedStoragePlugin,
    PluginHost, PluginKind, PluginPackage, RuntimeError, RuntimeRegistries, ServerConfig,
    ensure_known_profiles, ensure_profile_known,
};

impl PluginHost {
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
                    Arc::clone(&self.quarantine),
                )),
                loaded_at: package.modified_at()?,
            },
        );
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
                    Arc::clone(&self.quarantine),
                )),
                loaded_at: package.modified_at()?,
            },
        );
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
                    Arc::clone(&self.quarantine),
                )),
                loaded_at: package.modified_at()?,
            },
        );
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

    /// Registers protocol adapters and activates runtime-selected profiles.
    ///
    /// # Errors
    ///
    /// Returns an error when registries cannot be initialized or required profiles cannot be
    /// activated.
    pub fn initialize_runtime_registries(
        self: &Arc<Self>,
        config: &ServerConfig,
        registries: &mut RuntimeRegistries,
    ) -> Result<(), RuntimeError> {
        self.load_into_registries(registries)?;
        self.activate_runtime_profiles(config)?;
        if let Some(storage_profile) = self.resolve_storage_profile(&config.storage_profile) {
            registries.register_storage_profile(config.storage_profile.clone(), storage_profile);
        }
        Ok(())
    }
}
