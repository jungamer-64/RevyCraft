use super::bootstrap::{boot_imported_server, boot_server};
use super::selection::ResolvedRuntimeSelection;
use super::status::{RuntimeStatusSnapshot, SessionStatusSnapshot};
use super::{
    ExecutableChildCommit, ExecutableChildRuntimePrepared, ExecutableUpgradeStaged,
    RuntimeReloadMode, RuntimeServer,
};
use crate::RuntimeError;
use crate::config::ServerConfigSource;
use crate::runtime::{AdminControlPlaneHandle, ListenerBinding};
use mc_plugin_host::runtime::AdminSurfaceProfileHandle;
use revy_runtime_transfer::SharedTransferArena;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub struct ServerSupervisor {
    pub(crate) running: RunningServer,
}

#[derive(Clone)]
pub struct AdminSurfaceSelection {
    pub instance_id: String,
    pub surface_config_path: Option<PathBuf>,
    pub profile: Arc<dyn AdminSurfaceProfileHandle>,
}

pub(crate) struct RunningServer {
    pub(crate) runtime: Arc<RuntimeServer>,
    pub(crate) join_handle: tokio::sync::Mutex<Option<JoinHandle<Result<(), RuntimeError>>>>,
    pub(crate) runtime_completion_rx: watch::Receiver<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactsReloadResult {
    pub reloaded_plugin_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopologyReloadResult {
    pub activated_generation_id: crate::runtime::GenerationId,
    pub retired_generation_ids: Vec<crate::runtime::GenerationId>,
    pub applied_config_change: bool,
    pub reconfigured_adapter_ids: Vec<String>,
}

impl TopologyReloadResult {
    #[must_use]
    pub fn changed(&self, previous_generation_id: crate::runtime::GenerationId) -> bool {
        self.activated_generation_id != previous_generation_id
            || self.applied_config_change
            || !self.retired_generation_ids.is_empty()
            || !self.reconfigured_adapter_ids.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeReloadResult {
    Artifacts(ArtifactsReloadResult),
    Topology(TopologyReloadResult),
    Core(CoreReloadResult),
    Full(FullReloadResult),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreReloadResult {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullReloadResult {
    pub reloaded_plugin_ids: Vec<String>,
    pub topology: TopologyReloadResult,
}

impl ServerSupervisor {
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when config loading, plugin resolution, or runtime boot fails.
    pub async fn boot(config_source: ServerConfigSource) -> Result<Self, RuntimeError> {
        let config = config_source.load()?;
        let bootstrap_view = config.plugin_host_bootstrap_view();
        let bootstrap_config = mc_plugin_host::config::BootstrapConfig::from(&bootstrap_view);
        let plugin_host = mc_plugin_host::host::plugin_host_from_config(&bootstrap_config)?
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "no packaged plugins discovered under `{}`",
                    config.bootstrap.plugins_dir.display()
                ))
            })?;
        let runtime_selection_view = config.plugin_host_runtime_selection_view();
        let runtime_selection =
            mc_plugin_host::config::RuntimeSelectionConfig::from(&runtime_selection_view);
        let loaded_plugins = plugin_host.load_plugin_set(&runtime_selection)?;
        let running = boot_server(config_source, config, loaded_plugins, Some(plugin_host)).await?;
        Ok(Self { running })
    }

    /// Builds a committed child runtime with all imported network authorities still paused.
    /// Calling [`ExecutableChildRuntimePrepared::activate`] is the only transition that opens the
    /// child data plane.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the committed core, topology, or imported sessions cannot be
    /// installed as one runtime epoch.
    pub async fn boot_executable_child(
        commit: ExecutableChildCommit,
    ) -> Result<ExecutableChildRuntimePrepared, RuntimeError> {
        boot_imported_server(commit).await
    }

    /// Builds the immutable, full core pre-copy and exact active plugin manifest required before
    /// spawning an executable-upgrade child.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if active artifacts or the core pre-copy cannot be materialized.
    pub async fn prepare_executable_upgrade(
        &self,
        arena: Arc<SharedTransferArena>,
    ) -> Result<ExecutableUpgradeStaged, RuntimeError> {
        self.running.runtime.prepare_executable_upgrade(arena).await
    }

    #[must_use]
    pub fn listener_bindings(&self) -> Vec<ListenerBinding> {
        self.running.listener_bindings()
    }

    #[must_use]
    pub fn admin_control_plane(&self) -> AdminControlPlaneHandle {
        self.running.admin_control_plane()
    }

    pub async fn current_admin_surfaces(&self) -> Vec<AdminSurfaceSelection> {
        self.running.current_admin_surfaces().await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the requested reload mode cannot be applied.
    pub async fn reload_runtime(
        &self,
        mode: RuntimeReloadMode,
    ) -> Result<RuntimeReloadResult, RuntimeError> {
        self.running.reload_runtime(mode).await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a loaded plugin cannot be reloaded successfully.
    pub async fn reload_runtime_artifacts(&self) -> Result<ArtifactsReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Artifacts).await? {
            RuntimeReloadResult::Artifacts(result) => Ok(result),
            RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Core(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("artifacts reload should only produce an artifacts-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot materialize a candidate topology.
    pub async fn reload_runtime_topology(&self) -> Result<TopologyReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Topology).await? {
            RuntimeReloadResult::Topology(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Core(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("topology reload should only produce a topology-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot reload the live core.
    pub async fn reload_runtime_core(&self) -> Result<CoreReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Core).await? {
            RuntimeReloadResult::Core(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("core reload should only produce a core-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot reconcile live config state or apply a
    /// candidate topology.
    pub async fn reload_runtime_full(&self) -> Result<FullReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Full).await? {
            RuntimeReloadResult::Full(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Core(_) => {
                unreachable!("full reload should only produce a full-scoped result")
            }
        }
    }

    #[must_use]
    pub async fn status(&self) -> RuntimeStatusSnapshot {
        self.running.status().await
    }

    #[must_use]
    pub async fn session_status(&self) -> Vec<SessionStatusSnapshot> {
        self.running.session_status().await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime completion watcher closes unexpectedly.
    pub async fn wait_for_runtime_completion(&self) -> Result<(), RuntimeError> {
        self.running.wait_for_runtime_completion().await
    }

    /// Waits until runtime shutdown has been accepted, before session draining completes.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if the lifecycle watcher closes before shutdown is requested.
    pub async fn wait_for_shutdown_requested(&self) -> Result<(), RuntimeError> {
        self.running.wait_for_shutdown_requested().await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime task exits with an error or the join fails.
    pub async fn join_runtime(&self) -> Result<(), RuntimeError> {
        self.running.join_runtime().await
    }

    pub fn request_shutdown(&self) -> bool {
        self.running.request_shutdown()
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the server task fails while shutting down.
    pub async fn shutdown(self) -> Result<(), RuntimeError> {
        self.running.shutdown().await
    }
}

impl RunningServer {
    #[must_use]
    pub fn listener_bindings(&self) -> Vec<ListenerBinding> {
        self.runtime.listener_bindings()
    }

    #[must_use]
    pub fn admin_control_plane(&self) -> AdminControlPlaneHandle {
        AdminControlPlaneHandle::new(Arc::clone(&self.runtime))
    }

    pub async fn current_admin_surfaces(&self) -> Vec<AdminSurfaceSelection> {
        self.runtime.current_admin_surfaces().await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime completion watcher closes unexpectedly.
    pub async fn wait_for_runtime_completion(&self) -> Result<(), RuntimeError> {
        let mut completion_rx = self.runtime_completion_rx.clone();
        if *completion_rx.borrow() {
            return Ok(());
        }
        completion_rx.changed().await.map_err(|error| {
            RuntimeError::Config(format!(
                "runtime completion watcher closed unexpectedly: {error}"
            ))
        })?;
        Ok(())
    }

    pub(crate) async fn wait_for_shutdown_requested(&self) -> Result<(), RuntimeError> {
        let mut shutdown_rx = self.runtime.reload.subscribe_shutdown_requested();
        if *shutdown_rx.borrow() {
            return Ok(());
        }
        shutdown_rx.changed().await.map_err(|error| {
            RuntimeError::Config(format!(
                "runtime shutdown watcher closed before shutdown was requested: {error}"
            ))
        })
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the requested reload mode cannot be applied.
    pub async fn reload_runtime(
        &self,
        mode: RuntimeReloadMode,
    ) -> Result<RuntimeReloadResult, RuntimeError> {
        let reload_host = self.runtime.reload.reload_host().ok_or_else(|| {
            RuntimeError::Config(
                "reload is unavailable without a reload-capable supervisor boot".into(),
            )
        })?;
        self.runtime
            .reload_runtime(reload_host.as_ref(), mode)
            .await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a loaded plugin cannot be reloaded successfully.
    #[allow(dead_code)]
    pub async fn reload_runtime_artifacts(&self) -> Result<ArtifactsReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Artifacts).await? {
            RuntimeReloadResult::Artifacts(result) => Ok(result),
            RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Core(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("artifacts reload should only produce an artifacts-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot materialize a candidate topology.
    #[allow(dead_code)]
    pub async fn reload_runtime_topology(&self) -> Result<TopologyReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Topology).await? {
            RuntimeReloadResult::Topology(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Core(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("topology reload should only produce a topology-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot reload the live core.
    #[allow(dead_code)]
    pub async fn reload_runtime_core(&self) -> Result<CoreReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Core).await? {
            RuntimeReloadResult::Core(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Full(_) => {
                unreachable!("core reload should only produce a core-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot reconcile live config state or apply a
    /// candidate topology.
    #[allow(dead_code)]
    pub async fn reload_runtime_full(&self) -> Result<FullReloadResult, RuntimeError> {
        match self.reload_runtime(RuntimeReloadMode::Full).await? {
            RuntimeReloadResult::Full(result) => Ok(result),
            RuntimeReloadResult::Artifacts(_)
            | RuntimeReloadResult::Topology(_)
            | RuntimeReloadResult::Core(_) => {
                unreachable!("full reload should only produce a full-scoped result")
            }
        }
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the server task fails while shutting down.
    pub async fn shutdown(self) -> Result<(), RuntimeError> {
        let _ = self.runtime.request_shutdown();
        self.join_runtime().await
    }

    pub fn request_shutdown(&self) -> bool {
        self.runtime.request_shutdown()
    }

    pub async fn join_runtime(&self) -> Result<(), RuntimeError> {
        let join_handle = self.join_handle.lock().await.take();
        match join_handle {
            Some(join_handle) => join_handle.await?,
            None => self.wait_for_runtime_completion().await,
        }
    }
}

impl RuntimeServer {
    pub(crate) async fn current_admin_surfaces(&self) -> Vec<AdminSurfaceSelection> {
        let active = self.authority.active();
        active
            .selection
            .admin_surfaces
            .iter()
            .map(|selection| AdminSurfaceSelection {
                instance_id: selection.instance_id.clone(),
                surface_config_path: selection.surface_config_path.clone(),
                profile: Arc::clone(&selection.profile),
            })
            .collect()
    }

    pub(crate) async fn selection_state(&self) -> ResolvedRuntimeSelection {
        self.authority.active().selection.clone()
    }
}
