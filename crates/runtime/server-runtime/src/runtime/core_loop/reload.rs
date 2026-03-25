use crate::RuntimeError;
use crate::runtime::selection::{ResolvedRuntimeSelection, SelectionResolver};
use crate::runtime::{
    ConfigReloadResult, ReloadResult, ReloadScope, RuntimeReloadContext, RuntimeServer,
};
use mc_plugin_host::runtime::RuntimePluginHost;
use tokio::sync::RwLockWriteGuard;

struct PreparedSelectionReload {
    context: RuntimeReloadContext,
    previous_selection: ResolvedRuntimeSelection,
    candidate_selection: ResolvedRuntimeSelection,
    reloaded_plugins: Vec<String>,
}

impl RuntimeServer {
    fn validate_static_reload_candidate(
        &self,
        candidate: &crate::config::ServerConfig,
    ) -> Result<(), RuntimeError> {
        Ok(self
            .reload
            .static_config()
            .validate_reload_compatibility(&candidate.static_config())?)
    }

    async fn restore_runtime_selection(
        &self,
        reload_host: &dyn RuntimePluginHost,
        previous_selection: &ResolvedRuntimeSelection,
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
        self.reload.reload_host().and_then(|reload_host| {
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
        self.reload.mark_shutting_down();
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
        let protocol_sessions = self.sessions.protocol_reload_sessions().await;
        let gameplay_sessions = self.sessions.gameplay_reload_sessions().await;
        let snapshot = self.kernel.snapshot().await;
        RuntimeReloadContext {
            protocol_sessions,
            gameplay_sessions,
            snapshot,
            world_dir: self.kernel.world_dir().to_path_buf(),
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
                let loaded_config = self.reload.config_source().load()?;
                self.reload_scope_with_loaded(reload_host, loaded_config, scope)
                    .await
            }
        }
    }

    pub(in crate::runtime) async fn maybe_reload_config_watch(
        &self,
        reload_host: &dyn RuntimePluginHost,
    ) -> Result<Option<ConfigReloadResult>, RuntimeError> {
        let loaded_config = self.reload.config_source().load()?;
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
        let previous_selection = self.selection_state().await;
        let selection_result = reload_host.reconcile_runtime_selection(
            &loaded_config.plugin_host_runtime_selection_config(),
            &context,
        )?;
        let mc_plugin_host::runtime::RuntimeSelectionResult {
            loaded_plugins,
            reloaded: reloaded_plugins,
        } = selection_result;
        let candidate_selection = match SelectionResolver::resolve(
            loaded_config.clone(),
            loaded_plugins,
            &context.gameplay_sessions,
        ) {
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
        let consistency_guard = self.reload.write_consistency().await;
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
        let candidate_selection = match SelectionResolver::resolve(
            previous_selection.config.clone(),
            loaded_plugins,
            &context.gameplay_sessions,
        ) {
            Ok(candidate_selection) => candidate_selection,
            Err(error) => {
                self.restore_runtime_selection(reload_host, &previous_selection, &context)
                    .await;
                return Err(error);
            }
        };
        self.selection.replace(candidate_selection).await;
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

        let consistency_guard = self.reload.write_consistency().await;
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
        self.selection.replace(prepared.candidate_selection).await;
        Ok(ReloadResult::Config(ConfigReloadResult {
            reloaded_plugins: prepared.reloaded_plugins,
            generation: generation_result,
        }))
    }
}
