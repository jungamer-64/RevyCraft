use super::listeners::{bind_runtime_listeners, spawn_listener_workers};
use super::r#loop::spawn_runtime_loop;
use super::protocols::{ActiveProtocols, activate_protocols};
use crate::RuntimeError;
use crate::config::ServerConfigSource;
use crate::runtime::kernel::RuntimeKernel;
use crate::runtime::reload_coordinator::ReloadCoordinator;
use crate::runtime::selection::{
    BootstrapSelectionResolution, SelectionManager, SelectionResolver,
};
use crate::runtime::session_registry::SessionRegistry;
use crate::runtime::topology_manager::TopologyManager;
use crate::runtime::{
    ACCEPT_QUEUE_CAPACITY, ActiveGeneration, GenerationId, RunningServer, RuntimeServer,
};
use mc_plugin_host::registry::LoadedPluginSet;
use mc_plugin_host::runtime::RuntimePluginHost;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
pub(crate) async fn boot_server(
    config_source: ServerConfigSource,
    config: crate::config::ServerConfig,
    loaded_plugins: LoadedPluginSet,
    reload_host: Option<Arc<dyn RuntimePluginHost>>,
) -> Result<RunningServer, RuntimeError> {
    config.validate()?;
    if reload_host.is_none() {
        if config.plugins.reload_watch {
            return Err(RuntimeError::Config(
                "plugins.reload_watch requires a reload-capable supervisor boot".to_string(),
            ));
        }
        if config.topology.reload_watch {
            return Err(RuntimeError::Config(
                "topology.reload_watch requires a reload-capable supervisor boot".to_string(),
            ));
        }
    }

    let ActiveProtocols {
        protocols,
        default_adapter,
        default_bedrock_adapter,
    } = activate_protocols(&config, loaded_plugins.protocols())?;
    let BootstrapSelectionResolution {
        selection,
        storage_profile,
        online_auth_keys,
        core,
    } = SelectionResolver::resolve_bootstrap(&config, loaded_plugins.clone())?;
    let super::listeners::BoundListeners {
        listener_bindings,
        bound_listeners,
    } = bind_runtime_listeners(&config, &protocols).await?;
    let initial_generation_id = GenerationId(1);
    let active_generation = Arc::new(ActiveGeneration {
        generation_id: initial_generation_id,
        config: config.clone(),
        protocol_registry: protocols,
        default_adapter,
        default_bedrock_adapter,
        listener_bindings: listener_bindings.clone(),
    });
    let (accepted_tx, accepted_rx) = mpsc::channel(ACCEPT_QUEUE_CAPACITY);
    let sessions = SessionRegistry::new(accepted_tx);
    let listener_workers = spawn_listener_workers(
        bound_listeners,
        initial_generation_id,
        sessions.accepted_sender(),
        sessions.queued_accepts(),
    )?;

    let server = Arc::new(RuntimeServer {
        reload: ReloadCoordinator::new(config.static_config(), config_source, reload_host),
        selection: SelectionManager::new(selection, online_auth_keys),
        topology: TopologyManager::new(active_generation, listener_workers, 2),
        kernel: RuntimeKernel::new(core, storage_profile, config.bootstrap.world_dir.clone()),
        sessions,
        #[cfg(test)]
        fail_nth_reattach_send: std::sync::atomic::AtomicUsize::new(0),
    });

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (runtime_completion_tx, runtime_completion_rx) = tokio::sync::watch::channel(false);
    server.reload.install_shutdown_tx(shutdown_tx);
    let run_server = Arc::clone(&server);
    let join_handle =
        spawn_runtime_loop(run_server, shutdown_rx, accepted_rx, runtime_completion_tx);

    Ok(RunningServer {
        runtime: server,
        join_handle,
        runtime_completion_rx,
    })
}
