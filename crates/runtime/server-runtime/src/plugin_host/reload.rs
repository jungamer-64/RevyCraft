use super::{
    Arc, AuthGeneration, GameplayGeneration, ManagedAuthPlugin, ManagedGameplayPlugin,
    ManagedStoragePlugin, PluginHost, PluginKind, RuntimeError, RuntimeReloadContext,
    StorageGeneration, SystemTime, import_storage_runtime_state, migrate_gameplay_sessions,
};

impl PluginHost {
    fn load_gameplay_reload_candidate(
        &self,
        managed: &mut ManagedGameplayPlugin,
    ) -> Result<Option<(SystemTime, Arc<GameplayGeneration>)>, RuntimeError> {
        managed.package.refresh_dynamic_manifest()?;
        let modified_at = managed.package.modified_at()?;
        if modified_at <= managed.loaded_at {
            return Ok(None);
        }

        let generation = match self
            .loader
            .load_gameplay_generation(&managed.package, self.generations.next_generation_id())
        {
            Ok(generation) => Arc::new(generation),
            Err(error) => {
                eprintln!(
                    "gameplay reload load failed for `{}`: {error}",
                    managed.package.plugin_id
                );
                managed.loaded_at = modified_at;
                return Ok(None);
            }
        };
        if generation.profile_id != managed.profile_id {
            eprintln!(
                "gameplay plugin `{}` changed profile from `{}` to `{}` during reload",
                managed.package.plugin_id,
                managed.profile_id.as_str(),
                generation.profile_id.as_str()
            );
            managed.loaded_at = modified_at;
            return Ok(None);
        }
        Ok(Some((modified_at, generation)))
    }

    fn reload_gameplay_plugin_with_context(
        &self,
        managed: &mut ManagedGameplayPlugin,
        runtime: &RuntimeReloadContext,
        reloaded: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        let Some((modified_at, generation)) = self.load_gameplay_reload_candidate(managed)? else {
            return Ok(());
        };
        let migration_succeeded = migrate_gameplay_sessions(managed, &generation, runtime)?;
        managed.loaded_at = modified_at;
        if !migration_succeeded {
            return Ok(());
        }
        managed.profile.swap_generation(generation);
        reloaded.push(managed.package.plugin_id.clone());
        Ok(())
    }

    fn reload_gameplay_plugins_with_context(
        &self,
        runtime: &RuntimeReloadContext,
        reloaded: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        {
            let mut gameplay = self
                .gameplay
                .lock()
                .expect("plugin host mutex should not be poisoned");
            for managed in gameplay.values_mut() {
                self.reload_gameplay_plugin_with_context(managed, runtime, reloaded)?;
            }
        }
        Ok(())
    }

    fn load_storage_reload_candidate(
        &self,
        managed: &mut ManagedStoragePlugin,
    ) -> Result<Option<(SystemTime, Arc<StorageGeneration>)>, RuntimeError> {
        managed.package.refresh_dynamic_manifest()?;
        let modified_at = managed.package.modified_at()?;
        if modified_at <= managed.loaded_at {
            return Ok(None);
        }

        let generation = match self
            .loader
            .load_storage_generation(&managed.package, self.generations.next_generation_id())
        {
            Ok(generation) => Arc::new(generation),
            Err(error) => {
                eprintln!(
                    "storage reload load failed for `{}`: {error}",
                    managed.package.plugin_id
                );
                managed.loaded_at = modified_at;
                return Ok(None);
            }
        };
        if generation.profile_id != managed.profile_id {
            eprintln!(
                "storage plugin `{}` changed profile from `{}` to `{}` during reload",
                managed.package.plugin_id, managed.profile_id, generation.profile_id
            );
            managed.loaded_at = modified_at;
            return Ok(None);
        }
        Ok(Some((modified_at, generation)))
    }

    fn reload_storage_plugin_with_context(
        &self,
        managed: &mut ManagedStoragePlugin,
        runtime: &RuntimeReloadContext,
        reloaded: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        let Some((modified_at, generation)) = self.load_storage_reload_candidate(managed)? else {
            return Ok(());
        };
        let _reload_guard = managed
            .profile
            .reload_gate
            .write()
            .expect("storage reload gate should not be poisoned");
        if import_storage_runtime_state(&managed.package.plugin_id, &generation, runtime) {
            managed.profile.swap_generation(generation);
            reloaded.push(managed.package.plugin_id.clone());
        }
        managed.loaded_at = modified_at;
        Ok(())
    }

    fn reload_storage_plugins_with_context(
        &self,
        runtime: &RuntimeReloadContext,
        reloaded: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        {
            let mut storage = self
                .storage
                .lock()
                .expect("plugin host mutex should not be poisoned");
            for managed in storage.values_mut() {
                self.reload_storage_plugin_with_context(managed, runtime, reloaded)?;
            }
        }
        Ok(())
    }

    fn load_auth_reload_candidate(
        &self,
        managed: &mut ManagedAuthPlugin,
    ) -> Result<Option<(SystemTime, Arc<AuthGeneration>)>, RuntimeError> {
        managed.package.refresh_dynamic_manifest()?;
        let modified_at = managed.package.modified_at()?;
        if modified_at <= managed.loaded_at {
            return Ok(None);
        }

        let generation = match self
            .loader
            .load_auth_generation(&managed.package, self.generations.next_generation_id())
        {
            Ok(generation) => Arc::new(generation),
            Err(error) => {
                eprintln!(
                    "auth reload load failed for `{}`: {error}",
                    managed.package.plugin_id
                );
                managed.loaded_at = modified_at;
                return Ok(None);
            }
        };
        if generation.profile_id != managed.profile_id {
            eprintln!(
                "auth plugin `{}` changed profile from `{}` to `{}` during reload",
                managed.package.plugin_id, managed.profile_id, generation.profile_id
            );
            managed.loaded_at = modified_at;
            return Ok(None);
        }
        Ok(Some((modified_at, generation)))
    }

    fn reload_auth_plugin(
        &self,
        managed: &mut ManagedAuthPlugin,
        reloaded: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        let Some((modified_at, generation)) = self.load_auth_reload_candidate(managed)? else {
            return Ok(());
        };
        managed.profile.swap_generation(generation);
        managed.loaded_at = modified_at;
        reloaded.push(managed.package.plugin_id.clone());
        Ok(())
    }

    fn reload_auth_plugins(&self, reloaded: &mut Vec<String>) -> Result<(), RuntimeError> {
        {
            let mut auth = self
                .auth
                .lock()
                .expect("plugin host mutex should not be poisoned");
            for managed in auth.values_mut() {
                self.reload_auth_plugin(managed, reloaded)?;
            }
        }
        Ok(())
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
    pub fn reload_modified(&self) -> Result<Vec<String>, RuntimeError> {
        let mut reloaded = Vec::new();
        {
            let mut protocols = self
                .protocols
                .lock()
                .expect("plugin host mutex should not be poisoned");
            for managed in protocols.values_mut() {
                if managed.package.plugin_kind != PluginKind::Protocol {
                    continue;
                }
                managed.package.refresh_dynamic_manifest()?;
                let modified_at = managed.package.modified_at()?;
                if modified_at <= managed.loaded_at {
                    continue;
                }
                let generation = Arc::new(self.loader.load_protocol_generation(
                    &managed.package,
                    self.generations.next_generation_id(),
                )?);
                managed.adapter.swap_generation(generation);
                managed.loaded_at = modified_at;
                reloaded.push(managed.package.plugin_id.clone());
            }
        }
        Ok(reloaded)
    }

    pub(crate) fn reload_modified_with_context(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<String>, RuntimeError> {
        let mut reloaded = self.reload_modified()?;
        self.reload_gameplay_plugins_with_context(runtime, &mut reloaded)?;
        self.reload_storage_plugins_with_context(runtime, &mut reloaded)?;
        self.reload_auth_plugins(&mut reloaded)?;
        Ok(reloaded)
    }
}
