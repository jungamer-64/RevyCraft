use super::listeners::{
    bind_runtime_listeners, spawn_listener_workers, spawn_paused_listener_workers,
};
use super::r#loop::spawn_runtime_loop;
use super::protocols::{ActiveProtocols, activate_protocols};
use crate::RuntimeError;
use crate::config::{ServerConfig, ServerConfigSource, ValidatedServerConfig};
use crate::runtime::authority::{RuntimeAuthority, RuntimeEpoch};
use crate::runtime::core_store::CoreStore;
use crate::runtime::reload_coordinator::ReloadCoordinator;
use crate::runtime::selection::{BootstrapSelectionResolution, SelectionResolver};
use crate::runtime::session_registry::SessionRegistry;
use crate::runtime::topology_resources::TopologyResources;
use crate::runtime::{
    ACCEPT_QUEUE_CAPACITY, ActiveGeneration, ExecutableChildCommit, ExecutableChildRuntimePrepared,
    GenerationId, RunningServer, RuntimeServer,
};
use crate::transport::{BedrockListenerSocket, BoundTransportListener};
use mc_plugin_host::registry::LoadedPluginSet;
use mc_plugin_host::runtime::RuntimePluginHost;
use mc_proto_common::TransportKind;
use revy_voxel_core::CoreHandoff;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
pub(crate) async fn boot_server(
    config_source: ServerConfigSource,
    config: ValidatedServerConfig,
    loaded_plugins: LoadedPluginSet,
    reload_host: Option<Arc<dyn RuntimePluginHost>>,
) -> Result<RunningServer, RuntimeError> {
    validate_reload_capable_boot(&config, reload_host.as_ref())?;

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
        config: config.as_inner().clone(),
        protocol_registry: protocols,
        default_adapter,
        default_bedrock_adapter,
        listener_bindings: listener_bindings.clone(),
    });
    let core = Arc::new(CoreStore::new(
        core,
        storage_profile,
        config.bootstrap.world_dir.clone(),
    ));
    let epoch = RuntimeEpoch::initial(
        selection,
        Arc::clone(&active_generation),
        core,
        online_auth_keys,
    );
    let authority = RuntimeAuthority::new(epoch);
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
        topology_resources: TopologyResources::new(listener_workers, 2),
        authority,
        sessions,
    });

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (runtime_completion_tx, runtime_completion_rx) = tokio::sync::watch::channel(false);
    server.reload.install_shutdown_tx(shutdown_tx);
    let run_server = Arc::clone(&server);
    let join_handle =
        spawn_runtime_loop(run_server, shutdown_rx, accepted_rx, runtime_completion_tx);

    Ok(RunningServer {
        runtime: server,
        join_handle: tokio::sync::Mutex::new(Some(join_handle)),
        runtime_completion_rx,
    })
}

pub(crate) async fn boot_imported_server(
    commit: ExecutableChildCommit,
) -> Result<ExecutableChildRuntimePrepared, RuntimeError> {
    let parts = commit.into_boot_parts();
    let storage_profile = SelectionResolver::resolve_storage_profile(
        parts.config.as_inner(),
        &parts.selection.loaded_plugins,
    )?;
    let child_epoch_revision = parts
        .parent_epoch_revision
        .checked_add(1)
        .ok_or_else(|| RuntimeError::Config("runtime epoch revision overflow".to_string()))?;
    let next_generation_id = parts
        .active_generation_id
        .0
        .checked_add(1)
        .ok_or_else(|| RuntimeError::Config("runtime generation id overflow".to_string()))?;
    let ActiveProtocols {
        protocols,
        default_adapter,
        default_bedrock_adapter,
    } = parts.active_protocols;
    let tcp_binding = crate::ListenerBinding {
        transport: TransportKind::Tcp,
        local_addr: parts.tcp_listener.local_addr()?,
        adapter_ids: protocols.adapter_ids_for_transport(TransportKind::Tcp),
    };
    let mut listener_bindings = vec![tcp_binding.clone()];
    let mut bound_listeners = vec![BoundTransportListener::Tcp {
        listener: parts.tcp_listener,
        adapter_ids: tcp_binding.adapter_ids,
    }];
    if let Some(listener) = parts.bedrock_listener {
        let binding = crate::ListenerBinding {
            transport: TransportKind::Udp,
            local_addr: listener.local_addr()?,
            adapter_ids: protocols.adapter_ids_for_transport(TransportKind::Udp),
        };
        listener_bindings.push(binding.clone());
        bound_listeners.push(BoundTransportListener::Bedrock {
            listener: BedrockListenerSocket::ReceivePaused(Box::new(listener)),
            adapter_ids: binding.adapter_ids,
            bind_addr: binding.local_addr,
        });
    }
    let active_generation = Arc::new(ActiveGeneration {
        generation_id: parts.active_generation_id,
        config: parts.config.as_inner().clone(),
        protocol_registry: protocols,
        default_adapter,
        default_bedrock_adapter,
        listener_bindings,
    });
    let core = Arc::new(CoreStore::from_handoff(
        CoreHandoff::from_committed(parts.core),
        storage_profile,
        parts.config.bootstrap.world_dir.clone(),
        parts.persisted_revision,
        parts.latest_dirty_revision,
    ));
    let epoch = RuntimeEpoch::transferred(
        child_epoch_revision,
        parts.selection,
        Arc::clone(&active_generation),
        core,
        parts.online_auth_keys,
    )?;
    let authority = RuntimeAuthority::new(epoch);
    let data_plane = authority.freeze().await;
    let (accepted_tx, accepted_rx) = mpsc::channel(ACCEPT_QUEUE_CAPACITY);
    let sessions = SessionRegistry::new(accepted_tx);
    let listener_workers = spawn_paused_listener_workers(
        bound_listeners,
        parts.active_generation_id,
        sessions.accepted_sender(),
        sessions.queued_accepts(),
    )?;
    let server = Arc::new(RuntimeServer {
        reload: ReloadCoordinator::new(
            parts.config.static_config(),
            parts.config_source,
            Some(parts.plugin_host),
        ),
        topology_resources: TopologyResources::new(listener_workers, next_generation_id),
        authority,
        sessions,
    });

    let activation_receivers = match server
        .spawn_imported_sessions(parts.sessions, child_epoch_revision)
        .await
    {
        Ok(activation) => activation,
        Err(error) => {
            server.shutdown_listener_workers().await;
            server
                .terminate_all_sessions("Executable child import failed")
                .await;
            server.join_all_session_tasks().await;
            data_plane.resume();
            return Err(error);
        }
    };

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (runtime_completion_tx, runtime_completion_rx) = tokio::sync::watch::channel(false);
    server.reload.install_shutdown_tx(shutdown_tx);
    let join_handle = spawn_runtime_loop(
        Arc::clone(&server),
        shutdown_rx,
        accepted_rx,
        runtime_completion_tx,
    );
    Ok(ExecutableChildRuntimePrepared::new(
        RunningServer {
            runtime: server,
            join_handle: tokio::sync::Mutex::new(Some(join_handle)),
            runtime_completion_rx,
        },
        data_plane,
        activation_receivers,
        parts.queued_bedrock_peers,
        parts.active_generation_id,
        parts.cutover_context,
    ))
}

fn validate_reload_capable_boot(
    config: &ServerConfig,
    reload_host: Option<&Arc<dyn RuntimePluginHost>>,
) -> Result<(), RuntimeError> {
    if reload_host.is_some() {
        return Ok(());
    }
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
    Ok(())
}
