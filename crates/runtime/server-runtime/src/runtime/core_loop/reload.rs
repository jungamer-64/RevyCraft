use crate::RuntimeError;
use crate::runtime::selection::{ResolvedRuntimeSelection, SelectionResolver};
use crate::runtime::{
    ArtifactsReloadResult, CoreReloadResult, FullReloadResult, RuntimeReloadContext,
    RuntimeReloadMode, RuntimeReloadResult, RuntimeServer, SessionControl,
    SessionReattachInstruction, SessionReattachRecord,
};
use mc_plugin_host::runtime::{PreparedRuntimeSelection, RuntimePluginHost};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::Ordering;
use tokio::sync::RwLockWriteGuard;
use tokio::sync::oneshot;

struct PreparedSelectionReload {
    previous_selection: ResolvedRuntimeSelection,
    candidate_selection: ResolvedRuntimeSelection,
    prepared_selection: PreparedRuntimeSelection,
}

struct CoreReloadRollbackState {
    rollback_core: mc_core::ServerCore,
    records: Vec<SessionReattachRecord>,
}

struct CoreReloadPlanFailure {
    error: RuntimeError,
    rollback: CoreReloadRollbackState,
}

impl RuntimeServer {
    fn validate_static_reload_candidate(
        &self,
        candidate: &crate::config::ServerConfig,
        allow_core_bootstrap_reload: bool,
    ) -> Result<(), RuntimeError> {
        let active_static = self.reload.static_config();
        let candidate_static = candidate.static_config();
        if active_static.admin_grpc != candidate_static.admin_grpc {
            return Err(RuntimeError::Config(
                "admin.grpc transport changes require a restart".to_string(),
            ));
        }

        let active_bootstrap = &active_static.bootstrap;
        let candidate_bootstrap = &candidate_static.bootstrap;
        let restart_required_bootstrap_diff = active_bootstrap.online_mode
            != candidate_bootstrap.online_mode
            || active_bootstrap.level_type != candidate_bootstrap.level_type
            || active_bootstrap.world_dir != candidate_bootstrap.world_dir
            || active_bootstrap.storage_profile != candidate_bootstrap.storage_profile
            || active_bootstrap.plugins_dir != candidate_bootstrap.plugins_dir
            || active_bootstrap.plugin_abi_min != candidate_bootstrap.plugin_abi_min
            || active_bootstrap.plugin_abi_max != candidate_bootstrap.plugin_abi_max
            || (!allow_core_bootstrap_reload
                && (active_bootstrap.level_name != candidate_bootstrap.level_name
                    || active_bootstrap.game_mode != candidate_bootstrap.game_mode
                    || active_bootstrap.difficulty != candidate_bootstrap.difficulty
                    || active_bootstrap.view_distance != candidate_bootstrap.view_distance));
        if restart_required_bootstrap_diff {
            return Err(RuntimeError::Config(
                "bootstrap config changes require a restart".to_string(),
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_nth_reattach_send_for_test(&self, ordinal: usize) {
        self.fail_nth_reattach_send.store(ordinal, Ordering::SeqCst);
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

    pub(in crate::runtime) async fn reload_runtime(
        &self,
        reload_host: &dyn RuntimePluginHost,
        mode: RuntimeReloadMode,
    ) -> Result<RuntimeReloadResult, RuntimeError> {
        match mode {
            RuntimeReloadMode::Artifacts => self
                .reload_artifacts_scope(reload_host)
                .await
                .map(RuntimeReloadResult::Artifacts),
            RuntimeReloadMode::Topology | RuntimeReloadMode::Full | RuntimeReloadMode::Core => {
                let loaded_config = self.reload.config_source().load()?;
                self.reload_runtime_with_loaded(reload_host, loaded_config, mode)
                    .await
            }
        }
    }

    pub(in crate::runtime) async fn maybe_reload_runtime_watch(
        &self,
        reload_host: &dyn RuntimePluginHost,
    ) -> Result<Option<FullReloadResult>, RuntimeError> {
        let loaded_config = self.reload.config_source().load()?;
        let active_config = self.selection_state().await.config;
        if !loaded_config.topology.reload_watch
            && !loaded_config.plugins.reload_watch
            && !active_config.topology.reload_watch
            && !active_config.plugins.reload_watch
        {
            return Ok(None);
        }
        self.reload_runtime_with_loaded(reload_host, loaded_config, RuntimeReloadMode::Full)
            .await
            .map(|result| match result {
                RuntimeReloadResult::Full(result) => Some(result),
                RuntimeReloadResult::Artifacts(_)
                | RuntimeReloadResult::Topology(_)
                | RuntimeReloadResult::Core(_) => {
                    unreachable!("runtime watch reload should only produce a full-scoped result")
                }
            })
    }

    async fn prepare_selection_reload(
        &self,
        reload_host: &dyn RuntimePluginHost,
        loaded_config: crate::config::ServerConfig,
        consistency_guard: &RwLockWriteGuard<'_, ()>,
    ) -> Result<PreparedSelectionReload, RuntimeError> {
        self.validate_static_reload_candidate(&loaded_config, true)?;
        let context = self.reload_context(consistency_guard).await;
        let previous_selection = self.selection_state().await;
        let prepared_selection = reload_host.prepare_runtime_selection(
            &loaded_config.plugin_host_runtime_selection_config(),
            &context,
        )?;
        let candidate_selection = match SelectionResolver::resolve(
            loaded_config.clone(),
            prepared_selection.loaded_plugins().clone(),
            &context.gameplay_sessions,
        ) {
            Ok(candidate_selection) => candidate_selection,
            Err(error) => return Err(error),
        };
        Ok(PreparedSelectionReload {
            previous_selection,
            candidate_selection,
            prepared_selection,
        })
    }

    async fn reload_artifacts_scope(
        &self,
        reload_host: &dyn RuntimePluginHost,
    ) -> Result<ArtifactsReloadResult, RuntimeError> {
        let consistency_guard = self.reload.write_consistency().await;
        let context = self.reload_context(&consistency_guard).await;
        let previous_selection = self.selection_state().await;
        let prepared_selection = reload_host.prepare_runtime_artifacts(&context)?;
        let candidate_selection = match SelectionResolver::resolve(
            previous_selection.config.clone(),
            prepared_selection.loaded_plugins().clone(),
            &context.gameplay_sessions,
        ) {
            Ok(candidate_selection) => candidate_selection,
            Err(error) => return Err(error),
        };
        let reloaded = prepared_selection.reloaded_plugin_ids().to_vec();
        reload_host.commit_runtime_selection(prepared_selection);
        self.selection.replace(candidate_selection).await;
        Ok(ArtifactsReloadResult {
            reloaded_plugin_ids: reloaded,
        })
    }

    async fn reload_runtime_with_loaded(
        &self,
        reload_host: &dyn RuntimePluginHost,
        loaded_config: crate::config::ServerConfig,
        mode: RuntimeReloadMode,
    ) -> Result<RuntimeReloadResult, RuntimeError> {
        if matches!(mode, RuntimeReloadMode::Topology) {
            self.validate_static_reload_candidate(&loaded_config, false)?;
            let mut candidate_config = self.selection_state().await.config;
            candidate_config.network = loaded_config.network;
            candidate_config.topology = loaded_config.topology;
            let result = self
                .reload_generation_with_config(reload_host, candidate_config.clone(), false)
                .await
                .map(RuntimeReloadResult::Topology)?;
            self.update_generation_config(&candidate_config).await;
            return Ok(result);
        }

        if matches!(mode, RuntimeReloadMode::Core) {
            self.validate_static_reload_candidate(&loaded_config, true)?;
            let active_selection = self.selection_state().await;
            let consistency_guard = self.reload.write_consistency().await;
            let active_generation = self.topology.active_generation();
            let (candidate_core, dirty, rollback) = match self
                .reload_core_with_plan(
                    &consistency_guard,
                    SelectionResolver::core_config(&active_selection.config),
                    SelectionResolver::core_config(&loaded_config),
                    Arc::clone(&active_generation),
                    &active_selection,
                )
                .await
            {
                Ok(result) => result,
                Err(failure) => {
                    self.rollback_reattached_sessions(
                        &failure.rollback,
                        &active_generation,
                        &active_selection,
                    )
                    .await;
                    return Err(failure.error);
                }
            };
            self.kernel.swap_core(candidate_core, dirty).await;
            self.update_core_reload_config(&loaded_config).await;
            drop(rollback);
            return Ok(RuntimeReloadResult::Core(CoreReloadResult {}));
        }

        let consistency_guard = self.reload.write_consistency().await;
        let prepared = self
            .prepare_selection_reload(reload_host, loaded_config.clone(), &consistency_guard)
            .await?;
        let prepared_topology = match self
            .topology
            .prepare_generation_reload(
                loaded_config.clone(),
                false,
                prepared.prepared_selection.protocol_topology(),
            )
            .await
        {
            Ok(prepared_topology) => prepared_topology,
            Err(error) => return Err(error),
        };
        let candidate_generation =
            prepared_topology.candidate_generation(&self.topology.active_generation());
        let (candidate_core, dirty, rollback) = match self
            .reload_core_with_plan(
                &consistency_guard,
                SelectionResolver::core_config(&prepared.previous_selection.config),
                SelectionResolver::core_config(&loaded_config),
                candidate_generation,
                &prepared.candidate_selection,
            )
            .await
        {
            Ok(candidate_core) => candidate_core,
            Err(failure) => {
                self.topology.rollback_generation_reload(prepared_topology);
                let rollback_generation = self.topology.active_generation();
                self.rollback_reattached_sessions(
                    &failure.rollback,
                    &rollback_generation,
                    &prepared.previous_selection,
                )
                .await;
                return Err(failure.error);
            }
        };
        let prepared_topology = match self
            .topology
            .precommit_generation_reload(prepared_topology, &self.sessions)
            .await
        {
            Ok(prepared_topology) => prepared_topology,
            Err(error) => {
                let rollback_generation = self.topology.active_generation();
                self.rollback_reattached_sessions(
                    &rollback,
                    &rollback_generation,
                    &prepared.previous_selection,
                )
                .await;
                return Err(error);
            }
        };
        self.kernel.swap_core(candidate_core, dirty).await;
        let generation_result = match self
            .topology
            .commit_generation_reload(prepared_topology, &self.kernel, &self.sessions)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                let rollback_generation = self.topology.active_generation();
                self.rollback_reattached_sessions(
                    &rollback,
                    &rollback_generation,
                    &prepared.previous_selection,
                )
                .await;
                return Err(error);
            }
        };
        let reloaded_plugin_ids = prepared.prepared_selection.reloaded_plugin_ids().to_vec();
        reload_host.commit_runtime_selection(prepared.prepared_selection);
        self.selection.replace(prepared.candidate_selection).await;
        Ok(RuntimeReloadResult::Full(FullReloadResult {
            reloaded_plugin_ids,
            topology: generation_result,
        }))
    }

    async fn reload_core_with_plan(
        &self,
        _consistency_guard: &RwLockWriteGuard<'_, ()>,
        rollback_core_config: mc_core::CoreConfig,
        candidate_core_config: mc_core::CoreConfig,
        candidate_generation: Arc<crate::runtime::ActiveGeneration>,
        candidate_selection: &ResolvedRuntimeSelection,
    ) -> Result<(mc_core::ServerCore, bool, CoreReloadRollbackState), CoreReloadPlanFailure> {
        let exported = self.kernel.export_core_runtime_state().await;
        let rollback_core =
            mc_core::ServerCore::from_runtime_state(rollback_core_config, exported.blob.clone());
        let candidate_core =
            mc_core::ServerCore::from_runtime_state(candidate_core_config, exported.blob);
        let records = self.sessions.play_reattach_records().await;
        let rollback = CoreReloadRollbackState {
            rollback_core,
            records,
        };

        for record in &rollback.records {
            let instruction = match self.build_candidate_reattach_instruction(
                record,
                &candidate_generation,
                candidate_selection,
                &candidate_core,
            ) {
                Ok(instruction) => instruction,
                Err(error) => {
                    return Err(CoreReloadPlanFailure { error, rollback });
                }
            };
            if let Err(error) = self.send_reattach_instruction(record, instruction).await {
                return Err(CoreReloadPlanFailure { error, rollback });
            }
        }

        Ok((candidate_core, exported.dirty, rollback))
    }

    fn build_candidate_reattach_instruction(
        &self,
        record: &SessionReattachRecord,
        candidate_generation: &Arc<crate::runtime::ActiveGeneration>,
        candidate_selection: &ResolvedRuntimeSelection,
        candidate_core: &mc_core::ServerCore,
    ) -> Result<SessionReattachInstruction, RuntimeError> {
        let _previous_protocol_generation = record.protocol_generation;
        let _previous_gameplay_generation = record.gameplay_generation;
        let _previous_gameplay_profile = record.gameplay_profile.clone();
        let adapter_id = record.adapter_id.as_deref().ok_or_else(|| {
            RuntimeError::Config(format!(
                "play session {:?} is missing an adapter id during reattach",
                record.connection_id
            ))
        })?;
        let adapter = candidate_generation
            .protocol_registry
            .resolve_adapter(adapter_id)
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "candidate generation is missing adapter `{adapter_id}` for session {:?}",
                    record.connection_id
                ))
            })?;
        if adapter.descriptor().transport != record.transport {
            return Err(RuntimeError::Config(format!(
                "candidate adapter `{adapter_id}` transport changed for session {:?}",
                record.connection_id
            )));
        }
        let gameplay_profile = SelectionResolver::gameplay_profile_for_adapter(
            &candidate_selection.config,
            adapter_id,
        );
        let gameplay = candidate_selection
            .loaded_plugins
            .resolve_gameplay_profile(gameplay_profile.as_str())
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "candidate gameplay profile `{}` is not active for adapter `{adapter_id}`",
                    gameplay_profile.as_str()
                ))
            })?;
        let resync_events = record
            .player_id
            .map(|player_id| {
                candidate_core
                    .session_resync_events(player_id)
                    .into_iter()
                    .map(|event| Arc::new(event.event))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(SessionReattachInstruction {
            generation: Arc::clone(candidate_generation),
            adapter: Some(adapter),
            gameplay: Some(gameplay),
            phase: record.phase,
            player_id: record.player_id,
            entity_id: record.entity_id,
            resync_events,
        })
    }

    fn build_rollback_reattach_instruction(
        &self,
        record: &SessionReattachRecord,
        rollback_generation: &Arc<crate::runtime::ActiveGeneration>,
        rollback_selection: &ResolvedRuntimeSelection,
        rollback_core: &mc_core::ServerCore,
    ) -> Result<SessionReattachInstruction, RuntimeError> {
        let adapter_id = record.adapter_id.as_deref().ok_or_else(|| {
            RuntimeError::Config(format!(
                "play session {:?} is missing an adapter id during rollback",
                record.connection_id
            ))
        })?;
        let adapter = rollback_generation
            .protocol_registry
            .resolve_adapter(adapter_id)
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "rollback generation is missing adapter `{adapter_id}` for session {:?}",
                    record.connection_id
                ))
            })?;
        let gameplay_profile =
            SelectionResolver::gameplay_profile_for_adapter(&rollback_selection.config, adapter_id);
        let gameplay = rollback_selection
            .loaded_plugins
            .resolve_gameplay_profile(gameplay_profile.as_str())
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "rollback gameplay profile `{}` is not active for adapter `{adapter_id}`",
                    gameplay_profile.as_str()
                ))
            })?;
        Ok(SessionReattachInstruction {
            generation: Arc::clone(rollback_generation),
            adapter: Some(adapter),
            gameplay: Some(gameplay),
            phase: record.phase,
            player_id: record.player_id,
            entity_id: record.entity_id,
            resync_events: record
                .player_id
                .map(|player_id| {
                    rollback_core
                        .session_resync_events(player_id)
                        .into_iter()
                        .map(|event| Arc::new(event.event))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    async fn rollback_reattached_sessions(
        &self,
        rollback: &CoreReloadRollbackState,
        rollback_generation: &Arc<crate::runtime::ActiveGeneration>,
        rollback_selection: &ResolvedRuntimeSelection,
    ) {
        for record in rollback.records.iter().rev() {
            let Ok(instruction) = self.build_rollback_reattach_instruction(
                record,
                rollback_generation,
                rollback_selection,
                &rollback.rollback_core,
            ) else {
                continue;
            };
            let _ = self.send_reattach_instruction(record, instruction).await;
        }
    }

    async fn send_reattach_instruction(
        &self,
        record: &SessionReattachRecord,
        instruction: SessionReattachInstruction,
    ) -> Result<(), RuntimeError> {
        #[cfg(test)]
        if self.should_fail_reattach_send() {
            return Err(RuntimeError::Config(format!(
                "injected reattach failure for session {:?}",
                record.connection_id
            )));
        }
        let (ack_tx, ack_rx) = oneshot::channel();
        record
            .control_tx
            .send(SessionControl::Reattach {
                instruction,
                ack_tx,
            })
            .await
            .map_err(|_| {
                RuntimeError::Config(format!(
                    "failed to send reattach control to session {:?}",
                    record.connection_id
                ))
            })?;
        ack_rx.await.map_err(|_| {
            RuntimeError::Config(format!(
                "session {:?} dropped reattach acknowledgement",
                record.connection_id
            ))
        })?
    }
}

#[cfg(test)]
impl RuntimeServer {
    fn should_fail_reattach_send(&self) -> bool {
        loop {
            let current = self.fail_nth_reattach_send.load(Ordering::SeqCst);
            if current == 0 {
                return false;
            }
            if self
                .fail_nth_reattach_send
                .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return current == 1;
            }
        }
    }
}
