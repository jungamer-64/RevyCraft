use super::{
    Arc, AuthGeneration, GameplayGeneration, ManagedAuthPlugin, ManagedGameplayPlugin,
    ManagedProtocolPlugin, ManagedStoragePlugin, PluginFailureAction, PluginFailureStage,
    PluginHost, PluginKind, RuntimeError, RuntimeReloadContext, StorageGeneration, SystemTime,
    import_storage_runtime_state, migrate_gameplay_sessions, migrate_protocol_sessions,
    protocol_reload_compatible,
};
use crate::runtime::ProtocolReloadSession;

impl PluginHost {
    fn load_protocol_reload_candidate(
        &self,
        managed: &mut ManagedProtocolPlugin,
    ) -> Result<
        Option<(
            SystemTime,
            super::ArtifactIdentity,
            Arc<super::ProtocolGeneration>,
        )>,
        RuntimeError,
    > {
        managed.package.refresh_dynamic_manifest()?;
        let modified_at = managed.package.modified_at()?;
        if modified_at <= managed.loaded_at {
            return Ok(None);
        }
        let identity = managed.package.artifact_identity(modified_at);
        if self
            .failures
            .is_artifact_quarantined(&managed.package.plugin_id, &identity)
        {
            if let Some(reason) = self
                .failures
                .artifact_reason(&managed.package.plugin_id, &identity)
            {
                eprintln!(
                    "skipping quarantined protocol reload candidate `{}`: {reason}",
                    managed.package.plugin_id
                );
            }
            return Ok(None);
        }

        let generation = match self
            .loader
            .load_protocol_generation(&managed.package, self.generations.next_generation_id())
        {
            Ok(generation) => Arc::new(generation),
            Err(error) => {
                let reason = error.to_string();
                eprintln!(
                    "protocol reload load failed for `{}`: {reason}",
                    managed.package.plugin_id
                );
                let action = self.failures.action_for_kind(PluginKind::Protocol);
                self.failures.handle_candidate_failure(
                    PluginKind::Protocol,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    &reason,
                )?;
                if action == PluginFailureAction::Skip {
                    managed.loaded_at = modified_at;
                }
                return Ok(None);
            }
        };

        Ok(Some((modified_at, identity, generation)))
    }

    fn reload_protocol_plugin_with_sessions(
        &self,
        managed: &mut ManagedProtocolPlugin,
        protocol_sessions: &[ProtocolReloadSession],
        reloaded: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        let Some((modified_at, identity, generation)) =
            self.load_protocol_reload_candidate(managed)?
        else {
            return Ok(());
        };
        let current_generation = managed
            .adapter
            .current_generation()
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        if !protocol_reload_compatible(&managed.package.plugin_id, &current_generation, &generation)
        {
            let reason = "protocol topology changed during reload".to_string();
            let action = self.failures.action_for_kind(PluginKind::Protocol);
            self.failures.handle_candidate_failure(
                PluginKind::Protocol,
                PluginFailureStage::Reload,
                &managed.package.plugin_id,
                identity,
                &reason,
            )?;
            if action == PluginFailureAction::Skip {
                managed.loaded_at = modified_at;
            }
            return Ok(());
        }
        if !migrate_protocol_sessions(managed, &generation, protocol_sessions)? {
            let reason = "protocol session migration failed".to_string();
            let action = self.failures.action_for_kind(PluginKind::Protocol);
            self.failures.handle_candidate_failure(
                PluginKind::Protocol,
                PluginFailureStage::Reload,
                &managed.package.plugin_id,
                identity,
                &reason,
            )?;
            if action == PluginFailureAction::Skip {
                managed.loaded_at = modified_at;
            }
            return Ok(());
        }
        self.failures.clear_plugin_state(&managed.package.plugin_id);
        managed.loaded_at = modified_at;
        managed.active_loaded_at = modified_at;
        reloaded.push(managed.package.plugin_id.clone());
        Ok(())
    }

    fn reload_protocol_plugins_with_sessions(
        &self,
        protocol_sessions: &[ProtocolReloadSession],
        reloaded: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        let mut protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for managed in protocols.values_mut() {
            self.reload_protocol_plugin_with_sessions(managed, protocol_sessions, reloaded)?;
        }
        Ok(())
    }

    fn load_gameplay_reload_candidate(
        &self,
        managed: &mut ManagedGameplayPlugin,
    ) -> Result<Option<(SystemTime, super::ArtifactIdentity, Arc<GameplayGeneration>)>, RuntimeError>
    {
        managed.package.refresh_dynamic_manifest()?;
        let modified_at = managed.package.modified_at()?;
        if modified_at <= managed.loaded_at {
            return Ok(None);
        }
        let identity = managed.package.artifact_identity(modified_at);
        if self
            .failures
            .is_artifact_quarantined(&managed.package.plugin_id, &identity)
        {
            if let Some(reason) = self
                .failures
                .artifact_reason(&managed.package.plugin_id, &identity)
            {
                eprintln!(
                    "skipping quarantined gameplay reload candidate `{}`: {reason}",
                    managed.package.plugin_id
                );
            }
            return Ok(None);
        }

        let generation = match self
            .loader
            .load_gameplay_generation(&managed.package, self.generations.next_generation_id())
        {
            Ok(generation) => Arc::new(generation),
            Err(error) => {
                let reason = error.to_string();
                eprintln!(
                    "gameplay reload load failed for `{}`: {reason}",
                    managed.package.plugin_id
                );
                let action = self.failures.action_for_kind(PluginKind::Gameplay);
                self.failures.handle_candidate_failure(
                    PluginKind::Gameplay,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    &reason,
                )?;
                if action == PluginFailureAction::Skip {
                    managed.loaded_at = modified_at;
                }
                return Ok(None);
            }
        };
        if generation.profile_id != managed.profile_id {
            let reason = format!(
                "gameplay plugin `{}` changed profile from `{}` to `{}` during reload",
                managed.package.plugin_id,
                managed.profile_id.as_str(),
                generation.profile_id.as_str()
            );
            let action = self.failures.action_for_kind(PluginKind::Gameplay);
            self.failures.handle_candidate_failure(
                PluginKind::Gameplay,
                PluginFailureStage::Reload,
                &managed.package.plugin_id,
                identity,
                &reason,
            )?;
            if action == PluginFailureAction::Skip {
                managed.loaded_at = modified_at;
            }
            return Ok(None);
        }
        Ok(Some((modified_at, identity, generation)))
    }

    fn reload_gameplay_plugin_with_context(
        &self,
        managed: &mut ManagedGameplayPlugin,
        runtime: &RuntimeReloadContext,
        reloaded: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        let Some((modified_at, identity, generation)) =
            self.load_gameplay_reload_candidate(managed)?
        else {
            return Ok(());
        };
        let migration_succeeded = migrate_gameplay_sessions(managed, &generation, runtime)?;
        if !migration_succeeded {
            let action = self.failures.action_for_kind(PluginKind::Gameplay);
            self.failures.handle_candidate_failure(
                PluginKind::Gameplay,
                PluginFailureStage::Reload,
                &managed.package.plugin_id,
                identity,
                "gameplay session migration failed",
            )?;
            if action == PluginFailureAction::Skip {
                managed.loaded_at = modified_at;
            }
            return Ok(());
        }
        managed.profile.swap_generation(generation);
        self.failures.clear_plugin_state(&managed.package.plugin_id);
        managed.loaded_at = modified_at;
        managed.active_loaded_at = modified_at;
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
    ) -> Result<Option<(SystemTime, super::ArtifactIdentity, Arc<StorageGeneration>)>, RuntimeError>
    {
        managed.package.refresh_dynamic_manifest()?;
        let modified_at = managed.package.modified_at()?;
        if modified_at <= managed.loaded_at {
            return Ok(None);
        }
        let identity = managed.package.artifact_identity(modified_at);
        if self
            .failures
            .is_artifact_quarantined(&managed.package.plugin_id, &identity)
        {
            if let Some(reason) = self
                .failures
                .artifact_reason(&managed.package.plugin_id, &identity)
            {
                eprintln!(
                    "skipping quarantined storage reload candidate `{}`: {reason}",
                    managed.package.plugin_id
                );
            }
            return Ok(None);
        }

        let generation = match self
            .loader
            .load_storage_generation(&managed.package, self.generations.next_generation_id())
        {
            Ok(generation) => Arc::new(generation),
            Err(error) => {
                let reason = error.to_string();
                eprintln!(
                    "storage reload load failed for `{}`: {reason}",
                    managed.package.plugin_id
                );
                let action = self.failures.action_for_kind(PluginKind::Storage);
                self.failures.handle_candidate_failure(
                    PluginKind::Storage,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    &reason,
                )?;
                if action == PluginFailureAction::Skip {
                    managed.loaded_at = modified_at;
                }
                return Ok(None);
            }
        };
        if generation.profile_id != managed.profile_id {
            let reason = format!(
                "storage plugin `{}` changed profile from `{}` to `{}` during reload",
                managed.package.plugin_id, managed.profile_id, generation.profile_id
            );
            let action = self.failures.action_for_kind(PluginKind::Storage);
            self.failures.handle_candidate_failure(
                PluginKind::Storage,
                PluginFailureStage::Reload,
                &managed.package.plugin_id,
                identity,
                &reason,
            )?;
            if action == PluginFailureAction::Skip {
                managed.loaded_at = modified_at;
            }
            return Ok(None);
        }
        Ok(Some((modified_at, identity, generation)))
    }

    fn reload_storage_plugin_with_context(
        &self,
        managed: &mut ManagedStoragePlugin,
        runtime: &RuntimeReloadContext,
        reloaded: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        let Some((modified_at, identity, generation)) =
            self.load_storage_reload_candidate(managed)?
        else {
            return Ok(());
        };
        let _reload_guard = managed
            .profile
            .reload_gate
            .write()
            .expect("storage reload gate should not be poisoned");
        if import_storage_runtime_state(&managed.package.plugin_id, &generation, runtime) {
            managed.profile.swap_generation(generation);
            self.failures.clear_plugin_state(&managed.package.plugin_id);
            managed.loaded_at = modified_at;
            managed.active_loaded_at = modified_at;
            reloaded.push(managed.package.plugin_id.clone());
            return Ok(());
        }
        let action = self.failures.action_for_kind(PluginKind::Storage);
        self.failures.handle_candidate_failure(
            PluginKind::Storage,
            PluginFailureStage::Reload,
            &managed.package.plugin_id,
            identity,
            "storage runtime state import failed",
        )?;
        if action == PluginFailureAction::Skip {
            managed.loaded_at = modified_at;
        }
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
    ) -> Result<Option<(SystemTime, super::ArtifactIdentity, Arc<AuthGeneration>)>, RuntimeError>
    {
        managed.package.refresh_dynamic_manifest()?;
        let modified_at = managed.package.modified_at()?;
        if modified_at <= managed.loaded_at {
            return Ok(None);
        }
        let identity = managed.package.artifact_identity(modified_at);
        if self
            .failures
            .is_artifact_quarantined(&managed.package.plugin_id, &identity)
        {
            if let Some(reason) = self
                .failures
                .artifact_reason(&managed.package.plugin_id, &identity)
            {
                eprintln!(
                    "skipping quarantined auth reload candidate `{}`: {reason}",
                    managed.package.plugin_id
                );
            }
            return Ok(None);
        }

        let generation = match self
            .loader
            .load_auth_generation(&managed.package, self.generations.next_generation_id())
        {
            Ok(generation) => Arc::new(generation),
            Err(error) => {
                let reason = error.to_string();
                eprintln!(
                    "auth reload load failed for `{}`: {reason}",
                    managed.package.plugin_id
                );
                let action = self.failures.action_for_kind(PluginKind::Auth);
                self.failures.handle_candidate_failure(
                    PluginKind::Auth,
                    PluginFailureStage::Reload,
                    &managed.package.plugin_id,
                    identity,
                    &reason,
                )?;
                if action == PluginFailureAction::Skip {
                    managed.loaded_at = modified_at;
                }
                return Ok(None);
            }
        };
        if generation.profile_id != managed.profile_id {
            let reason = format!(
                "auth plugin `{}` changed profile from `{}` to `{}` during reload",
                managed.package.plugin_id, managed.profile_id, generation.profile_id
            );
            let action = self.failures.action_for_kind(PluginKind::Auth);
            self.failures.handle_candidate_failure(
                PluginKind::Auth,
                PluginFailureStage::Reload,
                &managed.package.plugin_id,
                identity,
                &reason,
            )?;
            if action == PluginFailureAction::Skip {
                managed.loaded_at = modified_at;
            }
            return Ok(None);
        }
        Ok(Some((modified_at, identity, generation)))
    }

    fn reload_auth_plugin(
        &self,
        managed: &mut ManagedAuthPlugin,
        reloaded: &mut Vec<String>,
    ) -> Result<(), RuntimeError> {
        let Some((modified_at, _identity, generation)) =
            self.load_auth_reload_candidate(managed)?
        else {
            return Ok(());
        };
        managed.profile.swap_generation(generation);
        self.failures.clear_plugin_state(&managed.package.plugin_id);
        managed.loaded_at = modified_at;
        managed.active_loaded_at = modified_at;
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
    pub(crate) fn reload_modified(&self) -> Result<Vec<String>, RuntimeError> {
        let mut reloaded = Vec::new();
        self.reload_protocol_plugins_with_sessions(&[], &mut reloaded)?;
        Ok(reloaded)
    }

    pub(crate) fn reload_modified_with_context(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<String>, RuntimeError> {
        let mut reloaded = Vec::new();
        self.reload_protocol_plugins_with_sessions(&runtime.protocol_sessions, &mut reloaded)?;
        self.reload_gameplay_plugins_with_context(runtime, &mut reloaded)?;
        self.reload_storage_plugins_with_context(runtime, &mut reloaded)?;
        self.reload_auth_plugins(&mut reloaded)?;
        Ok(reloaded)
    }
}
