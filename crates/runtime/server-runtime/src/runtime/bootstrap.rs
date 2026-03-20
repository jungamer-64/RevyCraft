use super::{OnlineAuthKeys, RunningServer, RuntimeServer, RuntimeState};
use crate::RuntimeError;
use crate::config::ServerConfig;
use crate::host::plugin_reload_poll_interval_ms;
use crate::registry::RuntimeRegistries;
use crate::transport::{
    AcceptedTransportSession, BoundTransportListener, TransportSessionIo, bind_transport_listener,
    build_listener_plans,
};
use mc_core::{CoreConfig, ServerCore};
use mc_plugin_api::AuthMode;
use mc_proto_common::{Edition, TransportKind};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, oneshot};

/// # Errors
///
/// Returns [`RuntimeError`] when the server cannot bind, load its persisted
/// world state, or starts with unsupported configuration such as
/// an auth profile mode mismatch.
pub async fn spawn_server(
    config: ServerConfig,
    registries: RuntimeRegistries,
) -> Result<RunningServer, RuntimeError> {
    let plugin_host = registries.plugin_host().ok_or_else(|| {
        RuntimeError::Config(format!(
            "no plugins discovered under `{}`",
            config.plugins_dir.display()
        ))
    })?;
    if registries
        .protocols()
        .resolve_adapter(&config.default_adapter)
        .is_none()
    {
        return Err(RuntimeError::Config(format!(
            "unknown default-adapter `{}`",
            config.default_adapter
        )));
    }
    if config.be_enabled
        && registries
            .protocols()
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
    let active_protocols = registries
        .protocols()
        .filter_enabled(&enabled_adapter_ids)?;
    plugin_host.activate_runtime_profiles(&config)?;
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
    let auth_mode = auth_profile.mode()?;
    match (config.online_mode, auth_mode) {
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
    let listener_plans = build_listener_plans(&config, &active_protocols)?;
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
    let tcp_listener = bind_transport_listener(tcp_plan, &config).await?;
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
        bound_listeners.push(bind_transport_listener(udp_plan, &config).await?);
    }

    let listener_bindings = bound_listeners
        .iter()
        .map(BoundTransportListener::listener_binding)
        .collect::<Result<Vec<_>, _>>()?;
    let mut tcp_listener = None;
    let mut bedrock_listener = None;
    for listener in bound_listeners {
        match listener {
            BoundTransportListener::Tcp { listener, .. } => {
                if tcp_listener.replace(listener).is_some() {
                    return Err(RuntimeError::Config(
                        "multiple tcp listeners are not supported".to_string(),
                    ));
                }
            }
            BoundTransportListener::Bedrock { listener, .. } => {
                if bedrock_listener.replace(listener).is_some() {
                    return Err(RuntimeError::Config(
                        "multiple udp listeners are not supported".to_string(),
                    ));
                }
            }
        }
    }
    let tcp_listener = tcp_listener
        .ok_or_else(|| RuntimeError::Config("no tcp transport listeners were bound".to_string()))?;

    let server = Arc::new(RuntimeServer {
        config,
        protocol_registry: active_protocols,
        plugin_host: Some(plugin_host.clone()),
        default_adapter,
        default_bedrock_adapter,
        auth_profile,
        bedrock_auth_profile,
        online_auth_keys,
        storage_profile,
        state: Mutex::new(RuntimeState { core, dirty: false }),
        sessions: Mutex::new(HashMap::new()),
        next_connection_id: Mutex::new(1),
    });

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let run_server = Arc::clone(&server);
    let join_handle = tokio::spawn(async move {
        let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
        let mut save_interval = tokio::time::interval(Duration::from_secs(2));
        let mut plugin_reload_interval =
            tokio::time::interval(Duration::from_millis(plugin_reload_poll_interval_ms()));
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    run_server.maybe_save().await?;
                    return Ok(());
                }
                accepted = tcp_listener.accept() => {
                    let (stream, _) = accepted?;
                    let session = AcceptedTransportSession {
                        transport: TransportKind::Tcp,
                        io: TransportSessionIo::Tcp {
                            stream,
                            encryption: None,
                        },
                    };
                    run_server.spawn_session(session).await;
                }
                accepted = async {
                    bedrock_listener
                        .as_mut()
                        .expect("bedrock listener branch should only run when configured")
                        .accept()
                        .await
                }, if bedrock_listener.is_some() => {
                    match accepted {
                        Ok(connection) => {
                            run_server.spawn_bedrock_session(connection).await;
                        }
                        Err(error) => {
                            eprintln!("bedrock accept failed: {error}");
                        }
                    }
                }
                _ = tick_interval.tick() => {
                    run_server.tick().await?;
                }
                _ = plugin_reload_interval.tick(), if run_server.config.plugin_reload_watch && run_server.plugin_host.is_some() => {
                    if let Some(plugin_host) = &run_server.plugin_host
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
    });

    Ok(RunningServer {
        listener_bindings,
        plugin_host: Some(plugin_host),
        runtime: server,
        shutdown_tx: Some(shutdown_tx),
        join_handle,
    })
}
