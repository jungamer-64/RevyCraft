use super::listeners::{bind_runtime_listeners, spawn_listener_workers};
use super::r#loop::spawn_runtime_loop;
use super::profiles::{RuntimeProfiles, resolve_runtime_profiles};
use super::protocols::{ActiveProtocols, activate_protocols};
use crate::RuntimeError;
use crate::config::ServerConfigSource;
use crate::runtime::admin::remote_admin_subjects_from_config;
use crate::runtime::{
    ACCEPT_QUEUE_CAPACITY, ActiveGeneration, GenerationId, RunningServer, RuntimeGenerationState,
    RuntimeSelectionState, RuntimeServer, RuntimeState,
};
use mc_plugin_host::registry::LoadedPluginSet;
use mc_plugin_host::runtime::RuntimePluginHost;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{Mutex, mpsc, oneshot};
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
    let RuntimeProfiles {
        storage_profile,
        auth_profile,
        bedrock_auth_profile,
        online_auth_keys,
        core,
    } = resolve_runtime_profiles(&config, &loaded_plugins)?;
    let super::listeners::BoundListeners {
        listener_bindings,
        bound_listeners,
    } = bind_runtime_listeners(&config, &protocols).await?;
    let admin_ui = loaded_plugins.resolve_admin_ui_profile(&config.admin.ui_profile);
    let selection_state = RuntimeSelectionState {
        config: config.clone(),
        loaded_plugins: loaded_plugins.clone(),
        auth_profile,
        bedrock_auth_profile,
        admin_ui,
        remote_admin_subjects: remote_admin_subjects_from_config(&config),
    };
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
    let queued_accepts = crate::runtime::QueuedAcceptTracker::default();
    let listener_workers = spawn_listener_workers(
        bound_listeners,
        initial_generation_id,
        accepted_tx.clone(),
        queued_accepts.clone(),
    )?;

    let server = Arc::new(RuntimeServer {
        static_config: config.static_config(),
        config_source,
        reload_host,
        selection_state: tokio::sync::RwLock::new(selection_state),
        consistency_gate: tokio::sync::RwLock::new(()),
        generation_state: std::sync::RwLock::new(RuntimeGenerationState {
            active: active_generation,
            draining: Vec::new(),
            listener_workers,
            next_generation_id: 2,
        }),
        online_auth_keys,
        storage_profile,
        state: Mutex::new(RuntimeState { core, dirty: false }),
        sessions: Mutex::new(HashMap::new()),
        session_tasks: Mutex::new(tokio::task::JoinSet::new()),
        next_connection_id: Mutex::new(1),
        accepted_tx,
        queued_accepts,
        shutting_down: AtomicBool::new(false),
        shutdown_tx: std::sync::Mutex::new(None),
    });

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (runtime_completion_tx, runtime_completion_rx) = tokio::sync::watch::channel(false);
    *server
        .shutdown_tx
        .lock()
        .expect("shutdown mutex should not be poisoned") = Some(shutdown_tx);
    let run_server = Arc::clone(&server);
    let join_handle =
        spawn_runtime_loop(run_server, shutdown_rx, accepted_rx, runtime_completion_tx);

    Ok(RunningServer {
        runtime: server,
        join_handle,
        runtime_completion_rx,
    })
}
