use super::{
    AcceptedTopologySession, OnlineAuthKeys, RunningServer, RuntimeServer, RuntimeState,
    RuntimeTopologyGeneration, RuntimeTopologyState, TopologyGenerationId, TopologyListenerWorker,
};
use crate::RuntimeError;
use crate::config::{ServerConfig, ServerConfigSource};
use crate::host::plugin_reload_poll_interval_ms;
use crate::plugin_host::{HotSwappableAuthProfile, HotSwappableStorageProfile, PluginHost};
use crate::registry::{ListenerBinding, ProtocolRegistry, RuntimeRegistries};
use crate::transport::{
    AcceptedTransportSession, BoundTransportListener, TransportSessionIo, bind_transport_listener,
    build_listener_plans,
};
use mc_core::{CoreConfig, ServerCore};
use mc_plugin_api::AuthMode;
use mc_proto_common::{Edition, ProtocolAdapter, TransportKind};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

/// # Errors
///
/// Returns [`RuntimeError`] when the server cannot bind, load its persisted
/// world state, or starts with unsupported configuration such as an auth
/// profile mode mismatch.
pub async fn spawn_server(
    config_source: ServerConfigSource,
    registries: RuntimeRegistries,
) -> Result<RunningServer, RuntimeError> {
    let config = config_source.load()?;
    let plugin_host = resolve_plugin_host(&registries, &config)?;
    let active_protocols = activate_protocols(&config, registries.protocols())?;
    let RuntimeProfiles {
        storage_profile,
        auth_profile,
        bedrock_auth_profile,
        online_auth_keys,
        core,
    } = resolve_runtime_profiles(&config, &plugin_host)?;
    let BoundListeners {
        listener_bindings,
        bound_listeners,
    } = bind_runtime_listeners(&config, &active_protocols.protocols).await?;
    let initial_generation_id = TopologyGenerationId(1);
    let topology_generation = Arc::new(RuntimeTopologyGeneration {
        generation_id: initial_generation_id,
        config: config.clone(),
        protocol_registry: active_protocols.protocols,
        default_adapter: active_protocols.default_adapter,
        default_bedrock_adapter: active_protocols.default_bedrock_adapter,
        listener_bindings: listener_bindings.clone(),
    });
    let (accepted_tx, accepted_rx) = mpsc::unbounded_channel();
    let listener_workers =
        spawn_listener_workers(bound_listeners, initial_generation_id, accepted_tx.clone())?;

    let server = Arc::new(RuntimeServer {
        config,
        config_source,
        plugin_host: Some(plugin_host.clone()),
        topology: std::sync::RwLock::new(RuntimeTopologyState {
            active: topology_generation,
            draining: Vec::new(),
            listener_workers,
            next_generation_id: 2,
        }),
        auth_profile,
        bedrock_auth_profile,
        online_auth_keys,
        storage_profile,
        state: Mutex::new(RuntimeState { core, dirty: false }),
        sessions: Mutex::new(HashMap::new()),
        next_connection_id: Mutex::new(1),
        accepted_tx,
    });

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let run_server = Arc::clone(&server);
    let join_handle = spawn_runtime_loop(run_server, shutdown_rx, accepted_rx);

    Ok(RunningServer {
        plugin_host: Some(plugin_host),
        runtime: server,
        shutdown_tx: Some(shutdown_tx),
        join_handle,
    })
}

pub(super) struct ActiveProtocols {
    pub(super) protocols: ProtocolRegistry,
    pub(super) default_adapter: Arc<dyn ProtocolAdapter>,
    pub(super) default_bedrock_adapter: Option<Arc<dyn ProtocolAdapter>>,
}

struct RuntimeProfiles {
    storage_profile: Arc<HotSwappableStorageProfile>,
    auth_profile: Arc<HotSwappableAuthProfile>,
    bedrock_auth_profile: Option<Arc<HotSwappableAuthProfile>>,
    online_auth_keys: Option<Arc<OnlineAuthKeys>>,
    core: ServerCore,
}

struct BoundListeners {
    listener_bindings: Vec<ListenerBinding>,
    bound_listeners: Vec<BoundTransportListener>,
}

fn resolve_plugin_host(
    registries: &RuntimeRegistries,
    config: &ServerConfig,
) -> Result<Arc<PluginHost>, RuntimeError> {
    registries.plugin_host().ok_or_else(|| {
        RuntimeError::Config(format!(
            "no plugins discovered under `{}`",
            config.plugins_dir.display()
        ))
    })
}

pub(super) fn activate_protocols(
    config: &ServerConfig,
    protocols: &ProtocolRegistry,
) -> Result<ActiveProtocols, RuntimeError> {
    if protocols.resolve_adapter(&config.default_adapter).is_none() {
        return Err(RuntimeError::Config(format!(
            "unknown default-adapter `{}`",
            config.default_adapter
        )));
    }
    if config.be_enabled
        && protocols
            .resolve_adapter(&config.default_bedrock_adapter)
            .is_none()
    {
        return Err(RuntimeError::Config(format!(
            "unknown default-bedrock-adapter `{}`",
            config.default_bedrock_adapter
        )));
    }

    let mut enabled_adapter_ids = config.effective_enabled_adapters();
    if !enabled_adapter_ids
        .iter()
        .any(|adapter_id| adapter_id == &config.default_adapter)
    {
        return Err(RuntimeError::Config(format!(
            "default-adapter `{}` must be included in enabled-adapters",
            config.default_adapter
        )));
    }
    let enabled_bedrock_adapter_ids = if config.be_enabled {
        let enabled = config.effective_enabled_bedrock_adapters();
        if !enabled
            .iter()
            .any(|adapter_id| adapter_id == &config.default_bedrock_adapter)
        {
            return Err(RuntimeError::Config(format!(
                "default-bedrock-adapter `{}` must be included in enabled-bedrock-adapters",
                config.default_bedrock_adapter
            )));
        }
        enabled
    } else {
        Vec::new()
    };
    enabled_adapter_ids.extend(enabled_bedrock_adapter_ids.iter().cloned());
    let active_protocols = protocols.filter_enabled(&enabled_adapter_ids)?;
    if !config.be_enabled
        && !active_protocols
            .adapter_ids_for_transport(TransportKind::Udp)
            .is_empty()
    {
        return Err(RuntimeError::Config(
            "enabled-adapters contains udp adapters but be-enabled=false".to_string(),
        ));
    }

    let default_adapter = active_protocols
        .resolve_adapter(&config.default_adapter)
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "default-adapter `{}` is not active",
                config.default_adapter
            ))
        })?;
    if default_adapter.descriptor().transport != TransportKind::Tcp {
        return Err(RuntimeError::Config(format!(
            "default-adapter `{}` must be a tcp adapter",
            config.default_adapter
        )));
    }

    let default_bedrock_adapter = if config.be_enabled {
        let adapter = active_protocols
            .resolve_adapter(&config.default_bedrock_adapter)
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "default-bedrock-adapter `{}` is not active",
                    config.default_bedrock_adapter
                ))
            })?;
        let descriptor = adapter.descriptor();
        if descriptor.transport != TransportKind::Udp || descriptor.edition != Edition::Be {
            return Err(RuntimeError::Config(format!(
                "default-bedrock-adapter `{}` must be a bedrock udp adapter",
                config.default_bedrock_adapter
            )));
        }
        Some(adapter)
    } else {
        None
    };

    Ok(ActiveProtocols {
        protocols: active_protocols,
        default_adapter,
        default_bedrock_adapter,
    })
}

fn resolve_runtime_profiles(
    config: &ServerConfig,
    plugin_host: &Arc<PluginHost>,
) -> Result<RuntimeProfiles, RuntimeError> {
    plugin_host.activate_runtime_profiles(config)?;
    let storage_profile = plugin_host
        .resolve_storage_profile(&config.storage_profile)
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "unknown storage-profile `{}`",
                config.storage_profile
            ))
        })?;
    let auth_profile = plugin_host
        .resolve_auth_profile(&config.auth_profile)
        .ok_or_else(|| {
            RuntimeError::Config(format!("unknown auth-profile `{}`", config.auth_profile))
        })?;
    let bedrock_auth_profile = if config.be_enabled {
        Some(
            plugin_host
                .resolve_auth_profile(&config.bedrock_auth_profile)
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "unknown bedrock-auth-profile `{}`",
                        config.bedrock_auth_profile
                    ))
                })?,
        )
    } else {
        None
    };

    match (config.online_mode, auth_profile.mode()?) {
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
    if let Some(profile) = &bedrock_auth_profile {
        match profile.mode()? {
            AuthMode::BedrockOffline | AuthMode::BedrockXbl => {}
            mode => {
                return Err(RuntimeError::Config(format!(
                    "bedrock-auth-profile requires a bedrock auth mode, got {mode:?}"
                )));
            }
        }
    }

    let online_auth_keys = if config.online_mode {
        Some(Arc::new(OnlineAuthKeys::generate()?))
    } else {
        None
    };
    let snapshot = storage_profile.load_snapshot(&config.world_dir)?;
    let core_config = CoreConfig {
        level_name: config.level_name.clone(),
        seed: 0,
        max_players: config.max_players,
        view_distance: config.view_distance,
        game_mode: config.game_mode,
        difficulty: config.difficulty,
        ..CoreConfig::default()
    };
    let core = match snapshot {
        Some(snapshot) => ServerCore::from_snapshot(core_config, snapshot),
        None => ServerCore::new(core_config),
    };

    Ok(RuntimeProfiles {
        storage_profile,
        auth_profile,
        bedrock_auth_profile,
        online_auth_keys,
        core,
    })
}

async fn bind_runtime_listeners(
    config: &ServerConfig,
    active_protocols: &ProtocolRegistry,
) -> Result<BoundListeners, RuntimeError> {
    let listener_plans = build_listener_plans(config, active_protocols)?;
    let mut tcp_plan = None;
    let mut udp_plan = None;
    for plan in listener_plans {
        match plan.transport {
            TransportKind::Tcp => tcp_plan = Some(plan),
            TransportKind::Udp => udp_plan = Some(plan),
        }
    }

    let tcp_plan = tcp_plan
        .ok_or_else(|| RuntimeError::Config("no tcp listener plan was generated".to_string()))?;
    let tcp_listener = bind_transport_listener(tcp_plan, config).await?;
    let tcp_local_addr = match &tcp_listener {
        BoundTransportListener::Tcp { listener, .. } => listener.local_addr()?,
        BoundTransportListener::Bedrock { .. } => {
            return Err(RuntimeError::Config(
                "tcp listener plan resolved to a non-tcp listener".to_string(),
            ));
        }
    };

    let mut bound_listeners = vec![tcp_listener];
    if let Some(mut udp_plan) = udp_plan {
        if udp_plan.bind_addr.port() == 0 {
            udp_plan.bind_addr = SocketAddr::new(tcp_local_addr.ip(), tcp_local_addr.port());
        }
        bound_listeners.push(bind_transport_listener(udp_plan, config).await?);
    }

    let listener_bindings = bound_listeners
        .iter()
        .map(BoundTransportListener::listener_binding)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(BoundListeners {
        listener_bindings,
        bound_listeners,
    })
}

fn spawn_listener_workers(
    bound_listeners: Vec<BoundTransportListener>,
    generation_id: TopologyGenerationId,
    accepted_tx: mpsc::UnboundedSender<AcceptedTopologySession>,
) -> Result<HashMap<TransportKind, TopologyListenerWorker>, RuntimeError> {
    let mut workers = HashMap::new();
    for listener in bound_listeners {
        let worker = spawn_listener_worker(listener, generation_id, accepted_tx.clone())?;
        if workers.insert(worker.transport, worker).is_some() {
            return Err(RuntimeError::Config(
                "multiple listener workers for the same transport are not supported".to_string(),
            ));
        }
    }
    Ok(workers)
}

pub(super) fn spawn_listener_worker(
    listener: BoundTransportListener,
    generation_id: TopologyGenerationId,
    accepted_tx: mpsc::UnboundedSender<AcceptedTopologySession>,
) -> Result<TopologyListenerWorker, RuntimeError> {
    let binding = listener.listener_binding()?;
    let transport = binding.transport;
    let (generation_tx, generation_rx) = watch::channel(generation_id);
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let join_handle = match listener {
        BoundTransportListener::Tcp { listener, .. } => tokio::spawn(async move {
            let generation_rx = generation_rx;
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else {
                            break;
                        };
                        let _ = accepted_tx.send(AcceptedTopologySession {
                            topology_generation_id: *generation_rx.borrow(),
                            session: AcceptedTransportSession {
                                transport: TransportKind::Tcp,
                                io: TransportSessionIo::Tcp {
                                    stream,
                                    encryption: Box::default(),
                                },
                            },
                        });
                    }
                }
            }
        }),
        BoundTransportListener::Bedrock { mut listener, .. } => tokio::spawn(async move {
            let generation_rx = generation_rx;
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok(connection) = accepted else {
                            break;
                        };
                        let _ = accepted_tx.send(AcceptedTopologySession {
                            topology_generation_id: *generation_rx.borrow(),
                            session: AcceptedTransportSession {
                                transport: TransportKind::Udp,
                                io: TransportSessionIo::Bedrock {
                                    connection,
                                    compression: None,
                                },
                            },
                        });
                    }
                }
            }
        }),
    };
    Ok(TopologyListenerWorker {
        transport,
        generation_tx,
        shutdown_tx: Some(shutdown_tx),
        join_handle: Some(join_handle),
    })
}

fn spawn_runtime_loop(
    run_server: Arc<RuntimeServer>,
    mut shutdown_rx: oneshot::Receiver<()>,
    mut accepted_rx: mpsc::UnboundedReceiver<AcceptedTopologySession>,
) -> JoinHandle<Result<(), RuntimeError>> {
    tokio::spawn(async move {
        let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
        let mut save_interval = tokio::time::interval(Duration::from_secs(2));
        let mut plugin_reload_interval =
            tokio::time::interval(Duration::from_millis(plugin_reload_poll_interval_ms()));
        let mut topology_reload_interval =
            tokio::time::interval(Duration::from_millis(plugin_reload_poll_interval_ms()));
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    run_server.shutdown_listener_workers().await;
                    run_server.maybe_save().await?;
                    return Ok(());
                }
                maybe_accepted = accepted_rx.recv() => {
                    let Some(accepted) = maybe_accepted else {
                        continue;
                    };
                    run_server
                        .spawn_transport_session(accepted.topology_generation_id, accepted.session)
                        .await;
                }
                _ = tick_interval.tick() => {
                    run_server.tick().await?;
                    run_server.enforce_topology_drains().await?;
                }
                _ = topology_reload_interval.tick(), if run_server.plugin_host.is_some() => {
                    if let Some(plugin_host) = run_server.plugin_host.as_ref()
                        && let Err(error) = run_server.maybe_reload_topology_watch(plugin_host).await
                    {
                        eprintln!("topology reload failed: {error}");
                    }
                }
                _ = plugin_reload_interval.tick(), if run_server.config.plugin_reload_watch && run_server.plugin_host.is_some() => {
                    if let Some(plugin_host) = run_server.plugin_host.as_ref()
                        && let Err(error) = run_server.reload_plugins(plugin_host).await
                    {
                        eprintln!("plugin reload failed: {error}");
                    }
                }
                _ = save_interval.tick() => {
                    run_server.maybe_save().await?;
                }
            }
        }
    })
}
