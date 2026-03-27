use super::*;
use mc_core::{CoreEvent, EventTarget, PlayerId, TargetedEvent};
use mc_proto_common::ConnectionPhase;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use uuid::Uuid;

fn ipv6_loopback_available() -> bool {
    std::net::TcpListener::bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0)).is_ok()
}

async fn assert_spawn_fails_with_message(
    config: ServerConfig,
    expected_fragment: &str,
) -> Result<(), RuntimeError> {
    let result = build_test_server(config, plugin_test_registries_all()?).await;
    let Err(error) = result else {
        panic!("build_test_server should have failed");
    };
    assert!(
        matches!(error, RuntimeError::Config(ref message) if message.contains(expected_fragment)),
        "unexpected error: {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn running_server_exposes_listener_bindings() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;

    let binding = server
        .listener_bindings()
        .first()
        .expect("tcp listener binding should exist")
        .clone();
    assert_eq!(binding.transport, TransportKind::Tcp);
    assert!(binding.local_addr.port() > 0);
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_5_ADAPTER_ID)
    );

    server.shutdown().await
}

#[tokio::test]
async fn wildcard_ipv4_listener_accepts_ipv6_loopback_when_available() -> Result<(), RuntimeError> {
    if !ipv6_loopback_available() {
        return Ok(());
    }

    let temp_dir = tempdir()?;
    let mut config = ServerConfig::default();
    config.network.server_ip = Some(Ipv4Addr::UNSPECIFIED.into());
    config.network.server_port = 0;
    config.topology.be_enabled = false;
    config.bootstrap.world_dir = temp_dir.path().join("world");
    let server = build_test_server(config, plugin_test_registries_tcp_only()?).await?;

    let port = listener_addr(&server).port();
    let ipv4_stream = connect_tcp(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)).await?;
    drop(ipv4_stream);
    let ipv6_stream = connect_tcp(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port)).await?;
    drop(ipv6_stream);

    server.shutdown().await
}

#[tokio::test]
async fn wildcard_ipv4_dual_stack_listener_is_reused_on_noop_reload() -> Result<(), RuntimeError> {
    if !ipv6_loopback_available() {
        return Ok(());
    }

    let temp_dir = tempdir()?;
    let mut config = ServerConfig::default();
    config.network.server_ip = Some(Ipv4Addr::UNSPECIFIED.into());
    config.network.server_port = 0;
    config.topology.be_enabled = false;
    config.bootstrap.world_dir = temp_dir.path().join("world");
    let server = build_reloadable_test_server(config, plugin_test_registries_tcp_only()?).await?;

    let before = listener_addr(&server);
    let _reload = server.reload_runtime_topology().await?;
    let after = listener_addr(&server);
    assert_eq!(after, before);

    server.shutdown().await
}

#[tokio::test]
async fn running_server_status_exposes_topology_and_plugin_snapshot() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;

    let status = server.status().await;
    assert_eq!(
        status.active_generation.state,
        GenerationStatusState::Active
    );
    assert_eq!(status.active_generation.default_adapter_id, JE_5_ADAPTER_ID);
    assert!(
        status
            .active_generation
            .default_bedrock_adapter_id
            .is_none()
    );
    assert_eq!(status.listener_bindings, server.listener_bindings());
    assert_eq!(status.session_summary.total, 0);

    let plugin_host = status
        .plugin_host
        .as_ref()
        .expect("runtime status should expose the plugin host snapshot");
    assert_eq!(plugin_host.protocol_count, 5);
    assert_eq!(plugin_host.gameplay_count, 1);
    assert_eq!(plugin_host.storage_count, 1);
    assert_eq!(plugin_host.auth_count, 1);
    assert_eq!(plugin_host.admin_ui_count, 1);
    assert_eq!(
        plugin_host.failure_matrix.protocol,
        crate::PluginFailureAction::Quarantine
    );

    let summary = format_runtime_status_summary(&status);
    assert_eq!(
        summary,
        concat!(
            "runtime active-generation=1 draining-generations=0 listeners=1 sessions=0 dirty=false\n",
            "generation tcp-default=je-5 tcp-enabled=je-5 udp-default=- udp-enabled=- max-players=20 motd=\"Multi-version Rust server\"\n",
            "session-summary transport=tcp:0,udp:0 phase=handshaking:0,status:0,login:0,play:0\n",
            "plugins protocol=5 gameplay=1 storage=1 auth=1 admin-ui=1 active-quarantines=0 artifact-quarantines=0 pending-fatal=none"
        )
    );
    let serialized = toml::to_string(&status).expect("runtime status snapshot should serialize");
    assert!(serialized.contains("active_generation"));
    assert!(serialized.contains("plugin_host"));

    server.shutdown().await
}

#[tokio::test]
async fn supervisor_boot_keeps_listener_and_admin_bind_snapshot_after_toml_changes()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");

    let mut initial = loopback_server_config(temp_dir.path().join("world"));
    initial.network.server_port = 0;
    let _ops_token = seed_runtime_plugins_with_loopback_admin(
        &mut initial,
        &dist_dir,
        &[JE_5_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
        temp_dir.path(),
        "ops",
        "ops-token",
        vec![crate::config::AdminPermission::Status],
        "127.0.0.1:50051"
            .parse()
            .expect("loopback admin grpc addr should parse"),
    )?;
    write_server_toml(&config_path, &initial)?;

    let server =
        crate::runtime::ServerSupervisor::boot(ServerConfigSource::Toml(config_path.clone()))
            .await?;
    let expected_admin_grpc = server.admin_grpc_bind_addr();
    let expected_listener_bindings = server.listener_bindings();

    let mut updated = initial;
    updated.network.server_port = 25565;
    updated.admin.grpc.bind_addr = "127.0.0.1:50052"
        .parse()
        .expect("loopback admin grpc addr should parse");
    write_server_toml(&config_path, &updated)?;

    assert_eq!(server.admin_grpc_bind_addr(), expected_admin_grpc);
    assert_eq!(server.listener_bindings(), expected_listener_bindings);

    server.shutdown().await
}

#[tokio::test]
async fn shutdown_waits_for_active_handshaking_sessions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let _stream = connect_tcp(listener_addr(&server)).await?;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if server.session_status().await.len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| RuntimeError::Config("session did not become visible before shutdown".into()))?;

    tokio::time::timeout(Duration::from_secs(1), server.shutdown())
        .await
        .map_err(|_| RuntimeError::Config("shutdown timed out with an active session".into()))?
}

#[tokio::test]
async fn runtime_upgrade_hold_blocks_tick_and_save() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = crate::runtime::ServerSupervisor {
        running: build_test_server(
            loopback_server_config(temp_dir.path().join("world")),
            plugin_test_registries_tcp_only()?,
        )
        .await?,
    };
    server.running.runtime.kernel.set_dirty(true).await;

    let guard = server.begin_runtime_upgrade().await?;
    let tick = tokio::spawn({
        let runtime = std::sync::Arc::clone(&server.running.runtime);
        async move { runtime.tick().await }
    });
    let save = tokio::spawn({
        let runtime = std::sync::Arc::clone(&server.running.runtime);
        async move { runtime.maybe_save().await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !tick.is_finished(),
        "tick should block while upgrade hold is active"
    );
    assert!(
        !save.is_finished(),
        "maybe_save should block while upgrade hold is active"
    );

    let status = server.status().await;
    let upgrade = status
        .upgrade
        .ok_or_else(|| RuntimeError::Config("upgrade status should be visible".to_string()))?;
    assert_eq!(upgrade.role, crate::RuntimeUpgradeRole::Parent);
    assert_eq!(
        upgrade.phase,
        crate::RuntimeUpgradePhase::ParentWaitingChildReady
    );

    guard.rollback().await?;
    tick.await.map_err(RuntimeError::from)??;
    save.await.map_err(RuntimeError::from)??;

    server.shutdown().await
}

#[tokio::test]
async fn frozen_sessions_do_not_consume_tcp_bytes_until_resumed() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = crate::runtime::ServerSupervisor {
        running: build_test_server(
            loopback_server_config(temp_dir.path().join("world")),
            plugin_test_registries_tcp_only()?,
        )
        .await?,
    };
    let codec = MinecraftWireCodec;
    let protocol = TestJavaProtocol::Je5;
    let addr = listener_addr(&server.running);
    let (mut alpha, mut alpha_buffer) =
        connect_and_login_java_client(addr, &codec, protocol, "freeze-check").await?;
    let _ = read_until_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
    )
    .await?;

    let frozen = server
        .running
        .runtime
        .freeze_live_sessions_for_upgrade()
        .await?;
    write_packet(&mut alpha, &codec, &held_item_change(7)).await?;
    assert_no_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
    )
    .await?;

    server
        .running
        .runtime
        .resume_frozen_live_sessions_after_upgrade_rollback(frozen)
        .await?;
    let held_item = read_until_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        protocol,
        TestJavaPacket::HeldItemChange,
    )
    .await?;
    assert_eq!(held_item_from_packet(&held_item)?, 7);

    server.shutdown().await
}

#[tokio::test]
async fn runtime_loop_storage_error_shuts_down_listeners_and_sessions() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.storage_profile = failing_storage_plugin::PROFILE_ID.into();
    config.plugins.failure_policy.storage = PluginFailureAction::Quarantine;
    let LoadedPluginTestEnvironment {
        loaded_plugins,
        plugin_host,
    } = in_process_failing_storage_registries(PluginFailureAction::Quarantine)?;
    let server = build_test_server(
        config,
        LoadedPluginTestEnvironment {
            loaded_plugins,
            plugin_host,
        },
    )
    .await?;
    let addr = listener_addr(&server);
    let _stream = connect_tcp(addr).await?;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if server.session_status().await.len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| {
        RuntimeError::Config("session did not become visible before save failure".into())
    })?;

    server.runtime.kernel.set_dirty(true).await;

    tokio::time::timeout(Duration::from_secs(3), server.wait_for_runtime_completion())
        .await
        .map_err(|_| {
            RuntimeError::Config("runtime loop did not exit after save failure".into())
        })??;

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if server.session_status().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| {
        RuntimeError::Config("sessions were not torn down after runtime failure".into())
    })?;

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if connect_tcp(addr).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| RuntimeError::Config("listener stayed reachable after runtime failure".into()))?;

    let error = server
        .shutdown()
        .await
        .expect_err("runtime save failure should surface during shutdown");
    assert!(matches!(error, RuntimeError::Storage(_)));
    Ok(())
}

#[tokio::test]
async fn running_server_exposes_udp_listener_binding_when_enabled() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.topology.be_enabled = true;
    let server = build_test_server(config, plugin_test_registries_all()?).await?;

    assert_eq!(server.listener_bindings().len(), 2);
    let bindings = server.listener_bindings();
    let binding = bindings
        .iter()
        .find(|binding| binding.transport == TransportKind::Udp)
        .expect("udp listener binding should exist");
    assert!(binding.local_addr.port() > 0);
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == BE_924_ADAPTER_ID)
    );

    server.shutdown().await
}

#[tokio::test]
async fn running_server_session_status_reports_live_sessions() -> Result<(), RuntimeError> {
    #[derive(serde::Serialize)]
    struct SessionStatusList<'a> {
        sessions: &'a [crate::runtime::SessionStatusSnapshot],
    }

    let temp_dir = tempdir()?;
    let server = build_test_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let (_stream, _buffer, _) = connect_and_login_java_client_until(
        addr,
        &codec,
        TestJavaProtocol::Je5,
        "status-observer",
        TestJavaPacket::ChunkData,
    )
    .await?;

    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.transport, TransportKind::Tcp);
    assert_eq!(session.phase, ConnectionPhase::Play);
    assert_eq!(session.adapter_id.as_deref(), Some(JE_5_ADAPTER_ID));
    assert_eq!(session.gameplay_profile.as_deref(), Some("canonical"));
    assert!(session.player_id.is_some());
    assert!(session.entity_id.is_some());
    assert!(session.protocol_generation.is_some());
    assert!(session.gameplay_generation.is_some());

    let status = server.status().await;
    assert_eq!(status.session_summary.total, 1);
    assert!(
        status
            .session_summary
            .by_transport
            .iter()
            .any(|entry| entry.transport == TransportKind::Tcp && entry.count == 1)
    );
    assert!(
        status
            .session_summary
            .by_phase
            .iter()
            .any(|entry| entry.phase == ConnectionPhase::Play && entry.count == 1)
    );
    assert!(
        status
            .session_summary
            .by_adapter_id
            .iter()
            .any(|entry| entry.value.as_deref() == Some(JE_5_ADAPTER_ID) && entry.count == 1)
    );
    let serialized = toml::to_string(&SessionStatusList {
        sessions: &sessions,
    })
    .expect("session status snapshot list should serialize");
    assert!(serialized.contains("connection_id"));
    assert!(serialized.contains("protocol_generation"));

    server.shutdown().await
}

#[tokio::test]
async fn session_status_tracks_handshake_phase_without_registry_sync() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(
        &mut stream,
        &codec,
        &encode_handshake(TestJavaProtocol::Je5.protocol_version(), 1)?,
    )
    .await?;

    let sessions = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let sessions = server.session_status().await;
            if sessions.len() == 1 && sessions[0].phase == ConnectionPhase::Status {
                break sessions;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| RuntimeError::Config("status handshake did not become visible".into()))?;

    assert_eq!(sessions[0].adapter_id.as_deref(), Some(JE_5_ADAPTER_ID));
    assert_eq!(sessions[0].player_id, None);
    assert_eq!(sessions[0].entity_id, None);

    drop(stream);
    server.shutdown().await
}

#[tokio::test]
async fn login_accept_commit_is_owned_by_session_task_until_login_success_commits()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let mut pause = server
        .runtime
        .arm_login_accept_commit_pause_for_test()
        .await;

    let login = tokio::spawn(async move {
        connect_and_login_java_client_until(
            addr,
            &MinecraftWireCodec,
            TestJavaProtocol::Je5,
            "pending-login",
            TestJavaPacket::LoginSuccess,
        )
        .await
    });

    pause.wait_until_reached().await;
    let (mut stream, mut buffer, _) = login.await.map_err(RuntimeError::from)??;
    assert_eq!(
        server
            .runtime
            .sessions
            .pending_login_route_count_for_test()
            .await,
        1
    );

    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].phase, ConnectionPhase::Login);
    assert_eq!(sessions[0].player_id, None);
    assert_eq!(sessions[0].entity_id, None);

    let player_id = server
        .runtime
        .kernel
        .export_core_runtime_state()
        .await
        .blob
        .online_players
        .keys()
        .copied()
        .next()
        .expect("login should create one online player before session commit");

    server
        .runtime
        .dispatch_events(vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::SelectedHotbarSlotChanged { slot: 4 },
        }])
        .await;

    assert_no_java_packet(
        &mut stream,
        &MinecraftWireCodec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;

    pause.release();

    let mut observed_slots = Vec::new();
    for _ in 0..2 {
        let packet = read_until_java_packet(
            &mut stream,
            &MinecraftWireCodec,
            &mut buffer,
            TestJavaProtocol::Je5,
            TestJavaPacket::HeldItemChange,
        )
        .await?;
        observed_slots.push(held_item_from_packet_for_protocol(
            TestJavaProtocol::Je5,
            &packet,
        )?);
        if observed_slots.contains(&4) {
            break;
        }
    }
    assert!(
        observed_slots.contains(&4),
        "pending player-targeted event should flush after login commit, got {observed_slots:?}"
    );

    let sessions = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let sessions = server.session_status().await;
            if sessions.len() == 1
                && sessions[0].phase == ConnectionPhase::Play
                && sessions[0].player_id == Some(player_id)
                && sessions[0].entity_id.is_some()
            {
                break sessions;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| {
        RuntimeError::Config("play phase did not become visible after login commit".into())
    })?;

    assert_eq!(sessions[0].player_id, Some(player_id));
    assert_eq!(
        server
            .runtime
            .sessions
            .pending_login_route_count_for_test()
            .await,
        0
    );

    drop(stream);
    server.shutdown().await
}

#[tokio::test]
async fn pending_login_routes_do_not_duplicate_player_or_broadcast_recipients()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);

    let (stream, _buffer, _) = connect_and_login_java_client_until(
        addr,
        &MinecraftWireCodec,
        TestJavaProtocol::Je5,
        "dedup",
        TestJavaPacket::ChunkData,
    )
    .await?;

    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    let connection_id = sessions[0].connection_id;
    let player_id = sessions[0]
        .player_id
        .expect("committed session should expose a player id");

    server
        .runtime
        .sessions
        .record_pending_login_route(connection_id, player_id)
        .await;

    assert_eq!(
        server
            .runtime
            .sessions
            .pending_login_route_count_for_test()
            .await,
        1
    );

    let player_recipients = server
        .runtime
        .sessions
        .recipients_for_target(EventTarget::Player(player_id))
        .await;
    assert_eq!(player_recipients.len(), 1);

    let everyone_recipients = server
        .runtime
        .sessions
        .recipients_for_target(EventTarget::EveryoneExcept(PlayerId(Uuid::nil())))
        .await;
    assert_eq!(everyone_recipients.len(), 1);

    let excluded_recipients = server
        .runtime
        .sessions
        .recipients_for_target(EventTarget::EveryoneExcept(player_id))
        .await;
    assert!(excluded_recipients.is_empty());

    server
        .runtime
        .sessions
        .clear_pending_login_route(connection_id)
        .await;

    drop(stream);
    server.shutdown().await
}

#[tokio::test]
async fn pending_login_route_is_cleared_when_session_closes() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);

    let (stream, _buffer, _) = connect_and_login_java_client_until(
        addr,
        &MinecraftWireCodec,
        TestJavaProtocol::Je5,
        "cleanup",
        TestJavaPacket::ChunkData,
    )
    .await?;

    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    let connection_id = sessions[0].connection_id;
    let player_id = sessions[0]
        .player_id
        .expect("committed session should expose a player id");

    server
        .runtime
        .sessions
        .record_pending_login_route(connection_id, player_id)
        .await;
    assert_eq!(
        server
            .runtime
            .sessions
            .pending_login_route_count_for_test()
            .await,
        1
    );

    drop(stream);

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if server.session_status().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| RuntimeError::Config("session did not close after dropping client".into()))?;

    assert_eq!(
        server
            .runtime
            .sessions
            .pending_login_route_count_for_test()
            .await,
        0
    );

    server.shutdown().await
}

#[tokio::test]
async fn default_bedrock_adapter_requires_listener_metadata() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.topology.be_enabled = true;
    config.topology.default_bedrock_adapter = BE_PLACEHOLDER_ADAPTER_ID.into();
    config.topology.enabled_bedrock_adapters = Some(vec![BE_PLACEHOLDER_ADAPTER_ID.into()]);
    assert_spawn_fails_with_message(config, "must provide bedrock listener metadata").await
}

#[tokio::test]
async fn placeholder_bedrock_adapter_can_remain_enabled_when_not_default()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.topology.be_enabled = true;
    config.topology.default_bedrock_adapter = BE_924_ADAPTER_ID.into();
    config.topology.enabled_bedrock_adapters = Some(vec![
        BE_924_ADAPTER_ID.into(),
        BE_PLACEHOLDER_ADAPTER_ID.into(),
    ]);
    let server = build_test_server(config, plugin_test_registries_all()?).await?;

    let bindings = server.listener_bindings();
    let binding = bindings
        .iter()
        .find(|binding| binding.transport == TransportKind::Udp)
        .expect("udp listener binding should exist");
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == BE_PLACEHOLDER_ADAPTER_ID)
    );

    server.shutdown().await
}

#[tokio::test]
async fn tcp_listener_binding_reports_enabled_java_versions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.topology.enabled_adapters = Some(vec![
        JE_5_ADAPTER_ID.into(),
        JE_47_ADAPTER_ID.into(),
        JE_340_ADAPTER_ID.into(),
    ]);
    let server = build_test_server(config, plugin_test_registries_all()?).await?;

    let bindings = server.listener_bindings();
    let binding = bindings
        .iter()
        .find(|binding| binding.transport == TransportKind::Tcp)
        .expect("tcp listener binding should exist");
    assert_eq!(binding.adapter_ids.len(), 3);
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_5_ADAPTER_ID)
    );
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_47_ADAPTER_ID)
    );
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_340_ADAPTER_ID)
    );

    server.shutdown().await
}

#[tokio::test]
async fn status_ping_login_and_initial_world_work() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut status_stream = connect_tcp(addr).await?;
    write_packet(
        &mut status_stream,
        &codec,
        &encode_handshake(TestJavaProtocol::Je5.protocol_version(), 1)?,
    )
    .await?;
    write_packet(&mut status_stream, &codec, &status_request()).await?;
    let mut buffer = BytesMut::new();
    let status_response = read_packet(&mut status_stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&status_response), 0x00);
    write_packet(&mut status_stream, &codec, &status_ping(42)).await?;
    let pong = read_packet(&mut status_stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&pong), 0x01);

    let mut login_stream = connect_tcp(addr).await?;
    write_packet(
        &mut login_stream,
        &codec,
        &encode_handshake(TestJavaProtocol::Je5.protocol_version(), 2)?,
    )
    .await?;
    write_packet(&mut login_stream, &codec, &login_start("alpha")).await?;
    let mut login_buffer = BytesMut::new();
    let login_success = read_packet(&mut login_stream, &codec, &mut login_buffer).await?;
    assert_eq!(packet_id(&login_success), 0x02);
    let join_game = read_packet(&mut login_stream, &codec, &mut login_buffer).await?;
    assert_eq!(packet_id(&join_game), 0x01);
    let spawn_position = read_packet(&mut login_stream, &codec, &mut login_buffer).await?;
    assert_eq!(packet_id(&spawn_position), 0x05);
    let mut spawn_reader = PacketReader::new(&spawn_position);
    assert_eq!(
        spawn_reader.read_varint().expect("packet id should decode"),
        0x05
    );
    assert_eq!(spawn_reader.read_i32().expect("x should decode"), 0);
    assert_eq!(spawn_reader.read_i32().expect("y should decode"), 4);
    assert_eq!(spawn_reader.read_i32().expect("z should decode"), 0);
    let chunk_bulk = read_until_java_packet(
        &mut login_stream,
        &codec,
        &mut login_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::ChunkData,
    )
    .await?;
    assert_eq!(packet_id(&chunk_bulk), 0x26);

    server.shutdown().await
}

#[tokio::test]
async fn unsupported_status_protocol_receives_server_list_response() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(
        &mut stream,
        &codec,
        &encode_handshake(TestJavaProtocol::Je47.protocol_version(), 1)?,
    )
    .await?;
    write_packet(&mut stream, &codec, &status_request()).await?;
    let mut buffer = BytesMut::new();
    let status_response = read_packet(&mut stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&status_response), 0x00);
    let mut reader = PacketReader::new(&status_response);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x00);
    let payload = reader
        .read_string(32767)
        .expect("status json should decode");
    assert!(payload.contains("\"protocol\":5"));
    assert!(payload.contains("\"name\":\"1.7.10\""));

    write_packet(&mut stream, &codec, &status_ping(99)).await?;
    let pong = read_packet(&mut stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&pong), 0x01);

    server.shutdown().await
}

#[test]
fn udp_bedrock_probe_classifies_placeholder_datagram() -> Result<(), RuntimeError> {
    let registry = plugin_test_registries_all()?;
    let action = classify_udp_datagram(registry.protocols(), &raknet_unconnected_ping())?;
    assert_eq!(action, UdpDatagramAction::UnsupportedBedrock);
    Ok(())
}

#[test]
fn udp_unknown_datagram_is_ignored() -> Result<(), RuntimeError> {
    let registry = plugin_test_registries_all()?;
    let action = classify_udp_datagram(registry.protocols(), &[0xde, 0xad, 0xbe, 0xef])?;
    assert_eq!(action, UdpDatagramAction::Ignore);
    Ok(())
}

#[tokio::test]
async fn udp_bedrock_probe_does_not_block_je_status() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.topology.be_enabled = true;
    let server = build_test_server(config, plugin_test_registries_all()?).await?;

    let udp_addr = udp_listener_addr(&server);
    let udp_client = UdpSocket::bind("127.0.0.1:0").await?;
    udp_client
        .send_to(&raknet_unconnected_ping(), udp_addr)
        .await?;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut stream = connect_tcp(addr).await?;
    write_packet(
        &mut stream,
        &codec,
        &encode_handshake(TestJavaProtocol::Je5.protocol_version(), 1)?,
    )
    .await?;
    write_packet(&mut stream, &codec, &status_request()).await?;
    let mut buffer = BytesMut::new();
    let status_response = read_packet(&mut stream, &codec, &mut buffer).await?;
    let mut reader = PacketReader::new(&status_response);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x00);
    let payload = reader
        .read_string(32767)
        .expect("status json should decode");
    assert!(payload.contains("\"online\":0"));

    server.shutdown().await
}
