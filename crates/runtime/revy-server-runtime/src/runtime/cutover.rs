use super::authority::{FrozenDataPlane, RuntimeEpoch};
use super::selection::SelectionResolver;
use super::session_registry::PreparedSessionDirectory;
use super::topology_resources::{
    FrozenTopologyIngress, PrecommittedTopologyReload, PreparedTopologyReload,
};
use super::{
    ArtifactsReloadResult, CoreCandidatePlan, CoreDeltaSeal, CoreReloadResult, FullReloadResult,
    RuntimeReloadResult, RuntimeServer, TopologyReloadResult,
};
use crate::{
    CutoverConnectionMix, CutoverOperation, CutoverOutcome, CutoverReport, RuntimeError,
    RuntimeReloadMode,
};
use mc_plugin_host::runtime::{
    PreparedRuntimeSelection, RuntimePluginHost, RuntimeProtocolTopologyCandidate,
    RuntimeReloadContext,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct RuntimeChangeSet {
    mode: RuntimeReloadMode,
}

struct StagedCutover {
    change: RuntimeChangeSet,
    active: Arc<RuntimeEpoch>,
    candidate_selection: super::selection::ResolvedRuntimeSelection,
    candidate_core: Arc<super::CoreStore>,
    candidate_core_plan: CoreCandidatePlan,
    prepared_plugins: Option<PreparedRuntimeSelection>,
    prepared_topology: PreparedTopologyReload,
    reloaded_plugin_ids: Vec<String>,
    stage_duration: Duration,
}

struct PreparingCutover {
    change: RuntimeChangeSet,
    active: Arc<RuntimeEpoch>,
    candidate: Arc<RuntimeEpoch>,
    candidate_core_plan: CoreCandidatePlan,
    prepared_plugins: Option<PreparedRuntimeSelection>,
    prepared_topology: PrecommittedTopologyReload,
    prepared_sessions: PreparedSessionDirectory,
    reloaded_plugin_ids: Vec<String>,
    stage_duration: Duration,
    prepare_duration: Duration,
    connection_mix: CutoverConnectionMix,
    session_count: usize,
}

struct FrozenCutover {
    prepared: PreparingCutover,
    ingress: FrozenTopologyIngress,
    data_plane: FrozenDataPlane,
}

struct PreparedCutover {
    prepared: PreparingCutover,
    ingress: FrozenTopologyIngress,
    data_plane: FrozenDataPlane,
    // Keep the sealed event payloads alive until the data-plane opens. Actor queues retain
    // their own encoded frames; releasing this backing batch is old-epoch drain work.
    sealed_events: Vec<super::SharedCoreEvent>,
}

impl RuntimeServer {
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
        self.terminate_all_sessions("Server stopping due to runtime failure")
            .await;
        self.join_all_session_tasks().await;
        self.shutdown_listener_workers().await;
        if attempt_best_effort_save && let Err(save_error) = self.maybe_save().await {
            eprintln!("best-effort save during fatal shutdown failed: {save_error}");
        }
        Err(error)
    }

    async fn reload_context(&self, active: &RuntimeEpoch) -> RuntimeReloadContext {
        let (protocol_sessions, gameplay_sessions) = tokio::join!(
            self.sessions.protocol_reload_sessions(),
            self.sessions.gameplay_reload_sessions(),
        );
        let snapshot = active.core.version().snapshot();
        RuntimeReloadContext {
            protocol_sessions,
            gameplay_sessions,
            snapshot,
            world_dir: active.core.world_dir().to_path_buf(),
        }
    }

    pub(in crate::runtime) async fn reload_runtime(
        &self,
        reload_host: &dyn RuntimePluginHost,
        mode: RuntimeReloadMode,
    ) -> Result<RuntimeReloadResult, RuntimeError> {
        let _reload_serial = self.reload.lock_reload_serial().await;
        let staged = self.stage_cutover(reload_host, mode).await?;
        let preparing = staged.prepare(self).await?;
        let frozen = preparing.freeze(self).await?;
        frozen.finalize(self).await?.commit(self, reload_host).await
    }

    pub(in crate::runtime) async fn maybe_reload_runtime_watch(
        &self,
        reload_host: &dyn RuntimePluginHost,
    ) -> Result<Option<FullReloadResult>, RuntimeError> {
        if self.authority.executable_upgrade_status().is_some() {
            return Ok(None);
        }
        let Some(_reload_serial) = self.reload.try_lock_reload_serial() else {
            return Ok(None);
        };
        let loaded_config = self.reload.config_source().load()?;
        let active_config = self.authority.active().selection.config.clone();
        if !loaded_config.topology.reload_watch
            && !loaded_config.plugins.reload_watch
            && !active_config.topology.reload_watch
            && !active_config.plugins.reload_watch
        {
            return Ok(None);
        }
        let staged = self
            .stage_cutover_with_loaded(reload_host, RuntimeReloadMode::Full, loaded_config)
            .await?;
        let preparing = staged.prepare(self).await?;
        let result = preparing
            .freeze(self)
            .await?
            .finalize(self)
            .await?
            .commit(self, reload_host)
            .await?;
        match result {
            RuntimeReloadResult::Full(result) => Ok(Some(result)),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Core(_) => {
                unreachable!("runtime watch uses the full reload change set")
            }
        }
    }

    async fn stage_cutover(
        &self,
        reload_host: &dyn RuntimePluginHost,
        mode: RuntimeReloadMode,
    ) -> Result<StagedCutover, RuntimeError> {
        if mode == RuntimeReloadMode::Artifacts {
            return self
                .stage_cutover_with_config(
                    reload_host,
                    mode,
                    self.authority.active().selection.config.clone(),
                )
                .await;
        }
        let loaded = self.reload.config_source().load()?;
        self.stage_cutover_with_loaded(reload_host, mode, loaded)
            .await
    }

    async fn stage_cutover_with_loaded(
        &self,
        reload_host: &dyn RuntimePluginHost,
        mode: RuntimeReloadMode,
        loaded: crate::config::ValidatedServerConfig,
    ) -> Result<StagedCutover, RuntimeError> {
        let active = self.authority.active();
        let candidate_config = match mode {
            RuntimeReloadMode::Topology => {
                active
                    .selection
                    .config
                    .plan_topology_reload(&loaded)?
                    .next_active_config
            }
            RuntimeReloadMode::Core => {
                active
                    .selection
                    .config
                    .plan_core_reload(&loaded)?
                    .next_active_config
            }
            RuntimeReloadMode::Full => {
                active
                    .selection
                    .config
                    .plan_full_reload(&loaded)?
                    .next_active_config
            }
            RuntimeReloadMode::Artifacts => active.selection.config.clone(),
        };
        self.stage_cutover_with_config(reload_host, mode, candidate_config)
            .await
    }

    async fn stage_cutover_with_config(
        &self,
        reload_host: &dyn RuntimePluginHost,
        mode: RuntimeReloadMode,
        candidate_config: crate::config::ServerConfig,
    ) -> Result<StagedCutover, RuntimeError> {
        let started_at = Instant::now();
        let active = self.authority.active();
        let context = self.reload_context(&active).await;
        let prepared_plugins = match mode {
            RuntimeReloadMode::Artifacts => Some(reload_host.finalize_staged_runtime_selection(
                reload_host.stage_runtime_artifacts()?,
                &context,
            )?),
            RuntimeReloadMode::Topology | RuntimeReloadMode::Full => {
                let selection_view = candidate_config.plugin_host_runtime_selection_view();
                let selection_config =
                    mc_plugin_host::config::RuntimeSelectionConfig::from(&selection_view);
                Some(reload_host.finalize_staged_runtime_selection(
                    reload_host.stage_runtime_selection(&selection_config)?,
                    &context,
                )?)
            }
            RuntimeReloadMode::Core => None,
        };
        let loaded_plugins = prepared_plugins.as_ref().map_or_else(
            || active.selection.loaded_plugins.clone(),
            |prepared| prepared.loaded_plugins().clone(),
        );
        let candidate_selection = SelectionResolver::resolve(
            candidate_config.clone(),
            loaded_plugins,
            &context.gameplay_sessions,
        )?;
        let current_protocol_topology;
        let protocol_topology: &RuntimeProtocolTopologyCandidate =
            if let Some(prepared) = prepared_plugins.as_ref() {
                prepared.protocol_topology()
            } else {
                current_protocol_topology = reload_host.prepare_protocol_topology_for_reload()?;
                &current_protocol_topology
            };
        let prepared_topology = if mode == RuntimeReloadMode::Core {
            let mut candidate_generation = (*active.topology).clone();
            candidate_generation.config = candidate_config.clone();
            PreparedTopologyReload::ProtocolOnly {
                candidate_generation: Arc::new(candidate_generation),
                result: TopologyReloadResult {
                    activated_generation_id: active.topology.generation_id,
                    retired_generation_ids: Vec::new(),
                    applied_config_change: false,
                    reconfigured_adapter_ids: Vec::new(),
                },
            }
        } else {
            self.topology_resources
                .prepare_generation_reload(
                    &active.topology,
                    candidate_config.clone(),
                    false,
                    protocol_topology,
                )
                .await?
        };
        let storage_profile = SelectionResolver::resolve_storage_profile(
            &candidate_config,
            &candidate_selection.loaded_plugins,
        )?;
        let core_config = matches!(mode, RuntimeReloadMode::Core | RuntimeReloadMode::Full)
            .then(|| SelectionResolver::core_config(&candidate_config));
        let (candidate_core_plan, candidate_core) = active
            .core
            .plan_candidate(core_config, storage_profile)
            .await;
        let reloaded_plugin_ids = prepared_plugins
            .as_ref()
            .map(|prepared| prepared.reloaded_plugin_ids().to_vec())
            .unwrap_or_default();
        Ok(StagedCutover {
            change: RuntimeChangeSet { mode },
            active,
            candidate_selection,
            candidate_core,
            candidate_core_plan,
            prepared_plugins,
            prepared_topology,
            reloaded_plugin_ids,
            stage_duration: started_at.elapsed(),
        })
    }
}

impl StagedCutover {
    async fn prepare(self, server: &RuntimeServer) -> Result<PreparingCutover, RuntimeError> {
        let started_at = Instant::now();
        let candidate_topology = self
            .prepared_topology
            .candidate_generation(&self.active.topology);
        let candidate = RuntimeEpoch::candidate(
            &self.active,
            self.candidate_selection,
            candidate_topology,
            self.candidate_core,
        );
        let prepared_topology = match server
            .topology_resources
            .precommit_generation_reload(self.prepared_topology, &server.sessions)
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                self.active.core.end_precopy().await;
                return Err(error);
            }
        };
        let prepared_sessions = match server
            .sessions
            .prepare_cutover(
                Arc::clone(&candidate),
                Arc::clone(&candidate.core),
                matches!(
                    self.change.mode,
                    RuntimeReloadMode::Core | RuntimeReloadMode::Full
                ),
            )
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                self.active.core.end_precopy().await;
                let _ = server.sessions.abort_cutover(candidate.revision()).await;
                server
                    .topology_resources
                    .abort_precommitted_reload(prepared_topology)
                    .await;
                server.authority.record_cutover(CutoverReport {
                    operation: CutoverOperation::Reload,
                    mode: Some(self.change.mode),
                    connection_mix: server.sessions.connection_mix().await,
                    session_count: server.sessions.session_count().await,
                    stage_us: duration_us(self.stage_duration),
                    prepare_us: duration_us(started_at.elapsed()),
                    freeze_us: 0,
                    resume_us: 0,
                    outcome: CutoverOutcome::Aborted,
                    epoch_revision: self.active.revision(),
                });
                return Err(error);
            }
        };
        let connection_mix = server.sessions.connection_mix().await;
        let session_count = connection_mix.java.saturating_add(connection_mix.bedrock);
        Ok(PreparingCutover {
            change: self.change,
            active: self.active,
            candidate,
            candidate_core_plan: self.candidate_core_plan,
            prepared_plugins: self.prepared_plugins,
            prepared_topology,
            prepared_sessions,
            reloaded_plugin_ids: self.reloaded_plugin_ids,
            stage_duration: self.stage_duration,
            prepare_duration: started_at.elapsed(),
            connection_mix,
            session_count,
        })
    }
}

impl PreparingCutover {
    async fn freeze(self, server: &RuntimeServer) -> Result<FrozenCutover, RuntimeError> {
        let data_plane = server.authority.freeze().await;
        let ingress = match server.topology_resources.freeze_ingress().await {
            Ok(ingress) => ingress,
            Err(error) => {
                let _ = server
                    .sessions
                    .abort_cutover(self.candidate.revision())
                    .await;
                server
                    .topology_resources
                    .abort_precommitted_reload(self.prepared_topology)
                    .await;
                self.active.core.end_precopy().await;
                let resumed = data_plane.resume();
                server.authority.record_cutover(CutoverReport {
                    operation: CutoverOperation::Reload,
                    mode: Some(self.change.mode),
                    connection_mix: self.connection_mix,
                    session_count: self.session_count,
                    stage_us: duration_us(self.stage_duration),
                    prepare_us: duration_us(self.prepare_duration),
                    freeze_us: duration_us(resumed.elapsed()),
                    resume_us: 0,
                    outcome: CutoverOutcome::Aborted,
                    epoch_revision: self.active.revision(),
                });
                return Err(error);
            }
        };
        Ok(FrozenCutover {
            prepared: self,
            ingress,
            data_plane,
        })
    }
}

impl FrozenCutover {
    async fn finalize(mut self, server: &RuntimeServer) -> Result<PreparedCutover, RuntimeError> {
        let delta = self
            .prepared
            .active
            .core
            .seal_delta_since(self.prepared.candidate_core_plan.base_revision())
            .await;
        let CoreDeltaSeal { precopy, events } = match delta {
            Ok(delta) => delta,
            Err(error) => return Err(self.abort(server, error).await),
        };
        if let Err(error) = server
            .sessions
            .seal_cutover(
                &self.prepared.prepared_sessions,
                self.prepared.candidate.revision(),
                &events,
            )
            .await
        {
            return Err(self.abort(server, error).await);
        }
        let final_core = self.prepared.candidate_core_plan.materialize(precopy);
        self.prepared.candidate = RuntimeEpoch::candidate(
            &self.prepared.active,
            self.prepared.candidate.selection.clone(),
            Arc::clone(&self.prepared.candidate.topology),
            final_core,
        );
        Ok(PreparedCutover {
            prepared: self.prepared,
            ingress: self.ingress,
            data_plane: self.data_plane,
            sealed_events: events,
        })
    }

    /// Every pre-publication failure retains the old epoch and completes the same resume
    /// boundary. Recovery failures remain distinguishable from the failure that caused abort.
    async fn abort(self, server: &RuntimeServer, cause: RuntimeError) -> RuntimeError {
        let sessions = server
            .sessions
            .abort_cutover(self.prepared.candidate.revision())
            .await;
        server
            .topology_resources
            .abort_precommitted_reload(self.prepared.prepared_topology)
            .await;
        self.prepared.active.core.end_precopy().await;
        let ingress = self.ingress.resume().await;
        let resumed = self.data_plane.resume();
        server.authority.record_cutover(CutoverReport {
            operation: CutoverOperation::Reload,
            mode: Some(self.prepared.change.mode),
            connection_mix: self.prepared.connection_mix,
            session_count: self.prepared.session_count,
            stage_us: duration_us(self.prepared.stage_duration),
            prepare_us: duration_us(self.prepared.prepare_duration),
            freeze_us: duration_us(resumed.elapsed()),
            resume_us: 0,
            outcome: CutoverOutcome::Aborted,
            epoch_revision: self.prepared.active.revision(),
        });
        match (sessions, ingress) {
            (Ok(()), Ok(())) => cause,
            (sessions, ingress) => RuntimeError::CutoverAbortFailed {
                cause: Box::new(cause),
                sessions: sessions.err().map(Box::new),
                ingress: ingress.err().map(Box::new),
            },
        }
    }
}

impl PreparedCutover {
    async fn commit(
        self,
        server: &RuntimeServer,
        reload_host: &dyn RuntimePluginHost,
    ) -> Result<RuntimeReloadResult, RuntimeError> {
        let committed_topology = server.topology_resources.commit_generation_resources(
            Arc::clone(&self.prepared.active.topology),
            self.prepared.prepared_topology,
        );
        if let Some(prepared_plugins) = self.prepared.prepared_plugins {
            reload_host.commit_runtime_selection(prepared_plugins);
        }
        self.data_plane
            .publish(&server.authority, Arc::clone(&self.prepared.candidate));
        let freeze_before_resume = self.data_plane.elapsed();
        let ingress_result = self.ingress.resume().await;
        let resumed = self.data_plane.resume();
        server
            .authority
            .activate_epoch(self.prepared.candidate.revision());
        let freeze_duration = resumed.elapsed();
        let resume_duration = freeze_duration.saturating_sub(freeze_before_resume);
        drop(self.sealed_events);
        let topology_result = server
            .topology_resources
            .finish_committed_reload(committed_topology, &server.sessions)
            .await;
        let report = CutoverReport {
            operation: CutoverOperation::Reload,
            mode: Some(self.prepared.change.mode),
            connection_mix: self.prepared.connection_mix,
            session_count: self.prepared.session_count,
            stage_us: duration_us(self.prepared.stage_duration),
            prepare_us: duration_us(self.prepared.prepare_duration),
            freeze_us: duration_us(freeze_duration),
            resume_us: duration_us(resume_duration),
            outcome: CutoverOutcome::Committed,
            epoch_revision: self.prepared.candidate.revision(),
        };
        if report.freeze_us > 50_000 {
            eprintln!(
                "runtime reload freeze exceeded target: {}us (target=50000us acceptance=100000us hard-limit=200000us)",
                report.freeze_us
            );
        }
        server.authority.record_cutover(report);
        ingress_result?;
        Ok(match self.prepared.change.mode {
            RuntimeReloadMode::Artifacts => RuntimeReloadResult::Artifacts(ArtifactsReloadResult {
                reloaded_plugin_ids: self.prepared.reloaded_plugin_ids,
            }),
            RuntimeReloadMode::Topology => RuntimeReloadResult::Topology(topology_result),
            RuntimeReloadMode::Core => RuntimeReloadResult::Core(CoreReloadResult {}),
            RuntimeReloadMode::Full => RuntimeReloadResult::Full(FullReloadResult {
                reloaded_plugin_ids: self.prepared.reloaded_plugin_ids,
                topology: topology_result,
            }),
        })
    }
}

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}
