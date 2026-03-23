use crate::RuntimeError;
use crate::runtime::admin::remote_admin_subjects_from_config;
use crate::runtime::{
    ConfigReloadResult, ReloadResult, ReloadScope, RuntimeReloadContext, RuntimeSelectionState,
    RuntimeServer,
};
use mc_plugin_api::codec::auth::AuthMode;
use mc_plugin_api::codec::gameplay::GameplaySessionSnapshot;
use mc_plugin_api::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_host::registry::LoadedPluginSet;
use mc_plugin_host::runtime::{ProtocolReloadSession, RuntimePluginHost};
use std::collections::HashSet;
use tokio::sync::RwLockWriteGuard;

struct PreparedSelectionReload {
    context: RuntimeReloadContext,
    previous_selection: RuntimeSelectionState,
    candidate_selection: RuntimeSelectionState,
    reloaded_plugins: Vec<String>,
}

impl RuntimeServer {
    fn validate_static_reload_candidate(
        &self,
        candidate: &crate::config::ServerConfig,
    ) -> Result<(), RuntimeError> {
        Ok(self
            .static_config
            .validate_reload_compatibility(&candidate.static_config())?)
    }

    fn ensure_candidate_gameplay_profiles_active(
        &self,
        candidate: &crate::config::ServerConfig,
        context: &RuntimeReloadContext,
    ) -> Result<(), RuntimeError> {
        let mut active_profiles = HashSet::new();
        let _ = active_profiles.insert(candidate.profiles.default_gameplay.clone());
        active_profiles.extend(candidate.profiles.gameplay_map.values().cloned());
        for session in &context.gameplay_sessions {
            if !active_profiles.contains(session.gameplay_profile.as_str()) {
                return Err(RuntimeError::Config(format!(
                    "cannot remove gameplay profile `{}` while sessions are still using it",
                    session.gameplay_profile.as_str()
                )));
            }
        }
        Ok(())
    }

    fn resolve_selection_state(
        &self,
        config: crate::config::ServerConfig,
        loaded_plugins: LoadedPluginSet,
    ) -> Result<RuntimeSelectionState, RuntimeError> {
        let auth_profile = loaded_plugins
            .resolve_auth_profile(&config.profiles.auth)
            .ok_or_else(|| {
                RuntimeError::Config(format!("unknown auth-profile `{}`", config.profiles.auth))
            })?;
        match (
            self.static_config.bootstrap.online_mode,
            auth_profile.mode()?,
        ) {
            (true, AuthMode::Online) | (false, AuthMode::Offline) => {}
            (true, mode) => {
                return Err(RuntimeError::Config(format!(
                    "online-mode=true requires an online auth profile, got {mode:?}"
                )));
            }
            (false, mode) => {
                return Err(RuntimeError::Config(format!(
                    "online-mode=false requires an offline auth profile, got {mode:?}"
                )));
            }
        }

        let bedrock_auth_profile = if config.topology.be_enabled {
            let profile = loaded_plugins
                .resolve_auth_profile(&config.profiles.bedrock_auth)
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "unknown bedrock-auth-profile `{}`",
                        config.profiles.bedrock_auth
                    ))
                })?;
            match profile.mode()? {
                AuthMode::BedrockOffline | AuthMode::BedrockXbl => {}
                mode => {
                    return Err(RuntimeError::Config(format!(
                        "bedrock-auth-profile requires a bedrock auth mode, got {mode:?}"
                    )));
                }
            }
            Some(profile)
        } else {
            None
        };
        let admin_ui = loaded_plugins.resolve_admin_ui_profile(&config.admin.ui_profile);
        let remote_admin_subjects = remote_admin_subjects_from_config(&config);

        Ok(RuntimeSelectionState {
            config,
            loaded_plugins,
            auth_profile,
            bedrock_auth_profile,
            admin_ui,
            remote_admin_subjects,
        })
    }

    async fn restore_runtime_selection(
        &self,
        reload_host: &dyn RuntimePluginHost,
        previous_selection: &RuntimeSelectionState,
        context: &RuntimeReloadContext,
    ) {
        if let Err(error) = reload_host.reconcile_runtime_selection(
            &previous_selection
                .config
                .plugin_host_runtime_selection_config(),
            context,
        ) {
            eprintln!("failed to restore previous runtime selection after reload failure: {error}");
        }
    }

    pub(in crate::runtime) fn take_pending_plugin_fatal_error(&self) -> Option<RuntimeError> {
        self.reload_host.as_ref().and_then(|reload_host| {
            reload_host
                .take_pending_fatal_error()
                .map(RuntimeError::from)
        })
    }

    pub(in crate::runtime) async fn finish_with_runtime_error(
        &self,
        error: RuntimeError,
        attempt_best_effort_save: bool,
    ) -> Result<(), RuntimeError> {
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.shutdown_listener_workers().await;
        self.terminate_all_sessions("Server stopping due to runtime failure")
            .await;
        self.join_all_session_tasks().await;
        if attempt_best_effort_save {
            if let Err(save_error) = self.maybe_save().await {
                eprintln!("best-effort save during fatal shutdown failed: {save_error}");
            }
        }
        Err(error)
    }

    async fn reload_context(
        &self,
        _consistency_guard: &RwLockWriteGuard<'_, ()>,
    ) -> RuntimeReloadContext {
        let sessions = self.sessions.lock().await;
        let protocol_sessions = sessions
            .iter()
            .filter_map(|(connection_id, handle)| {
                let adapter_id = handle.adapter_id.clone()?;
                if !matches!(
                    handle.phase,
                    mc_proto_common::ConnectionPhase::Status
                        | mc_proto_common::ConnectionPhase::Login
                        | mc_proto_common::ConnectionPhase::Play
                ) {
                    return None;
                }
                Some(ProtocolReloadSession {
                    adapter_id,
                    session: ProtocolSessionSnapshot {
                        connection_id: *connection_id,
                        phase: handle.phase,
                        player_id: handle.player_id,
                        entity_id: handle.entity_id,
                    },
                })
            })
            .collect::<Vec<_>>();
        let gameplay_sessions = sessions
            .values()
            .filter_map(|handle| {
                Some(GameplaySessionSnapshot {
                    phase: handle.phase,
                    player_id: Some(handle.player_id?),
                    entity_id: handle.entity_id,
                    gameplay_profile: handle.gameplay_profile.clone()?,
                })
            })
            .collect::<Vec<_>>();
        let snapshot = { self.state.lock().await.core.snapshot() };
        RuntimeReloadContext {
            protocol_sessions,
            gameplay_sessions,
            snapshot,
            world_dir: self.static_config.bootstrap.world_dir.clone(),
        }
    }

    pub(in crate::runtime) async fn reload(
        &self,
        reload_host: &dyn RuntimePluginHost,
        scope: ReloadScope,
    ) -> Result<ReloadResult, RuntimeError> {
        match scope {
            ReloadScope::Plugins => self
                .reload_plugins_scope(reload_host)
                .await
                .map(ReloadResult::Plugins),
            ReloadScope::Config | ReloadScope::Generation => {
                let loaded_config = self.config_source.load()?;
                self.reload_scope_with_loaded(reload_host, loaded_config, scope)
                    .await
            }
        }
    }

    pub(in crate::runtime) async fn maybe_reload_config_watch(
        &self,
        reload_host: &dyn RuntimePluginHost,
    ) -> Result<Option<ConfigReloadResult>, RuntimeError> {
        let loaded_config = self.config_source.load()?;
        let active_config = self.selection_state().await.config;
        if !loaded_config.topology.reload_watch
            && !loaded_config.plugins.reload_watch
            && !active_config.topology.reload_watch
            && !active_config.plugins.reload_watch
        {
            return Ok(None);
        }
        self.reload_scope_with_loaded(reload_host, loaded_config, ReloadScope::Config)
            .await
            .map(|result| match result {
                ReloadResult::Config(result) => Some(result),
                ReloadResult::Plugins(_) | ReloadResult::Generation(_) => {
                    unreachable!("config watch reload should only produce a config-scoped result")
                }
            })
    }

    async fn prepare_selection_reload(
        &self,
        reload_host: &dyn RuntimePluginHost,
        loaded_config: crate::config::ServerConfig,
        consistency_guard: &RwLockWriteGuard<'_, ()>,
    ) -> Result<PreparedSelectionReload, RuntimeError> {
        self.validate_static_reload_candidate(&loaded_config)?;
        let context = self.reload_context(consistency_guard).await;
        self.ensure_candidate_gameplay_profiles_active(&loaded_config, &context)?;
        let previous_selection = self.selection_state().await;
        let selection_result = reload_host.reconcile_runtime_selection(
            &loaded_config.plugin_host_runtime_selection_config(),
            &context,
        )?;
        let mc_plugin_host::runtime::RuntimeSelectionResult {
            loaded_plugins,
            reloaded: reloaded_plugins,
        } = selection_result;
        let candidate_selection =
            match self.resolve_selection_state(loaded_config.clone(), loaded_plugins) {
                Ok(candidate_selection) => candidate_selection,
                Err(error) => {
                    self.restore_runtime_selection(reload_host, &previous_selection, &context)
                        .await;
                    return Err(error);
                }
            };
        Ok(PreparedSelectionReload {
            context,
            previous_selection,
            candidate_selection,
            reloaded_plugins,
        })
    }

    async fn reload_plugins_scope(
        &self,
        reload_host: &dyn RuntimePluginHost,
    ) -> Result<Vec<String>, RuntimeError> {
        let consistency_guard = self.consistency_gate.write().await;
        let context = self.reload_context(&consistency_guard).await;
        let previous_selection = self.selection_state().await;
        let selection_result = reload_host.reconcile_runtime_selection(
            &previous_selection
                .config
                .plugin_host_runtime_selection_config(),
            &context,
        )?;
        let mc_plugin_host::runtime::RuntimeSelectionResult {
            loaded_plugins,
            reloaded,
        } = selection_result;
        let candidate_selection =
            match self.resolve_selection_state(previous_selection.config.clone(), loaded_plugins) {
                Ok(candidate_selection) => candidate_selection,
                Err(error) => {
                    self.restore_runtime_selection(reload_host, &previous_selection, &context)
                        .await;
                    return Err(error);
                }
            };
        *self.selection_state.write().await = candidate_selection;
        Ok(reloaded)
    }

    async fn reload_scope_with_loaded(
        &self,
        reload_host: &dyn RuntimePluginHost,
        loaded_config: crate::config::ServerConfig,
        scope: ReloadScope,
    ) -> Result<ReloadResult, RuntimeError> {
        if matches!(scope, ReloadScope::Generation) {
            self.validate_static_reload_candidate(&loaded_config)?;
            let mut candidate_config = self.selection_state().await.config;
            candidate_config.network = loaded_config.network;
            candidate_config.topology = loaded_config.topology;
            let result = self
                .reload_generation_with_config(reload_host, candidate_config.clone(), false)
                .await
                .map(ReloadResult::Generation)?;
            self.update_generation_config(&candidate_config).await;
            return Ok(result);
        }

        let consistency_guard = self.consistency_gate.write().await;
        let prepared = self
            .prepare_selection_reload(reload_host, loaded_config.clone(), &consistency_guard)
            .await?;
        let generation_result = match self
            .reload_generation_with_config(reload_host, loaded_config, false)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                self.restore_runtime_selection(
                    reload_host,
                    &prepared.previous_selection,
                    &prepared.context,
                )
                .await;
                return Err(error);
            }
        };
        *self.selection_state.write().await = prepared.candidate_selection;
        Ok(ReloadResult::Config(ConfigReloadResult {
            reloaded_plugins: prepared.reloaded_plugins,
            generation: generation_result,
        }))
    }
}
