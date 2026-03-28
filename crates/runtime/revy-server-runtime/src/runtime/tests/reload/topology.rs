use super::*;
use crate::runtime::{AcceptedGenerationSession, GenerationId, QueuedAcceptGuard, RunningServer};

fn topology_reload_server_config(world_dir: PathBuf, dist_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.plugins_dir = dist_dir;
    config
}

async fn reload_server_with_queued_old_accept(
    drain_grace_secs: u64,
) -> Result<
    (
        tempfile::TempDir,
        RunningServer,
        GenerationId,
        QueuedAcceptGuard,
    ),
    RuntimeError,
> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_5_ADAPTER_ID.into();
    initial.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into(), JE_47_ADAPTER_ID.into()]);
    initial.topology.drain_grace_secs = drain_grace_secs;
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID])?,
    )
    .await?;
    let old_generation = server.runtime.active_generation().generation_id;
    let queued_accept = server
        .runtime
        .sessions
        .queued_accepts()
        .track(old_generation);

    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_47_ADAPTER_ID.into();
    write_server_toml_for_reload(&config_path, &updated)?;
    let result = server.reload_runtime_topology().await?;
    assert_ne!(result.activated_generation_id, old_generation);

    Ok((temp_dir, server, old_generation, queued_accept))
}

async fn synthetic_tcp_transport_session() -> Result<
    (
        tokio::net::TcpStream,
        crate::transport::AcceptedTransportSession,
    ),
    RuntimeError,
> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let client = tokio::spawn(async move { tokio::net::TcpStream::connect(addr).await });
    let (stream, _) = listener.accept().await?;
    let client = client.await??;
    Ok((
        client,
        crate::transport::AcceptedTransportSession {
            transport: TransportKind::Tcp,
            io: crate::transport::TransportSessionIo::Tcp {
                stream,
                encryption: Box::default(),
            },
        },
    ))
}

#[tokio::test]
async fn topology_reload_manual_inline_updates_protocol_topology() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("topology-inline-manual");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            JE_5_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-v1",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let server = build_reloadable_test_server(
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let before_generation = server.runtime.active_generation();
    let before_adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_5_ADAPTER_ID)
        .expect("runtime should resolve the initial adapter");
    let before_protocol_number = before_adapter.descriptor().protocol_number;

    harness
        .install_protocol_plugin_for_reload(
            "mc-plugin-proto-je-5-reload-test",
            JE_5_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "reload-incompatible",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let result = server.reload_runtime_topology().await?;
    assert_ne!(
        result.activated_generation_id,
        before_generation.generation_id
    );
    assert!(
        result
            .reconfigured_adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_5_ADAPTER_ID)
    );

    let after_adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_5_ADAPTER_ID)
        .expect("runtime should resolve the updated adapter");
    assert_eq!(
        after_adapter.descriptor().protocol_number,
        before_protocol_number + 1
    );

    server.shutdown().await
}

#[tokio::test]
async fn topology_reload_toml_source_reads_updated_server_toml() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID, BE_924_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_5_ADAPTER_ID.into();
    initial.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into(), JE_47_ADAPTER_ID.into()]);
    initial.plugins.buffer_limits.protocol_response_bytes = 4096;
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(
            dist_dir.clone(),
            &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID, BE_924_ADAPTER_ID],
        )?,
    )
    .await?;
    let before_generation = server.runtime.active_generation().generation_id;
    assert_eq!(server.listener_bindings().len(), 1);

    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_47_ADAPTER_ID.into();
    updated.topology.be_enabled = true;
    updated.topology.default_bedrock_adapter = BE_924_ADAPTER_ID.into();
    updated.topology.enabled_bedrock_adapters = Some(vec![BE_924_ADAPTER_ID.into()]);
    updated.plugins.buffer_limits.protocol_response_bytes = 8192;
    write_server_toml_for_reload(&config_path, &updated)?;

    let result = server.reload_runtime_topology().await?;
    assert_ne!(result.activated_generation_id, before_generation);
    assert!(result.applied_config_change);
    let bindings = server.listener_bindings();
    assert_eq!(bindings.len(), 2);
    assert!(
        bindings
            .iter()
            .any(|binding| binding.transport == TransportKind::Udp)
    );
    assert_eq!(
        server
            .runtime
            .active_generation()
            .default_adapter
            .descriptor()
            .adapter_id,
        JE_47_ADAPTER_ID
    );
    assert_eq!(
        server
            .runtime
            .selection_state()
            .await
            .config
            .plugins
            .buffer_limits
            .protocol_response_bytes,
        4096
    );

    server.shutdown().await
}

#[tokio::test]
async fn config_reload_rotates_generation_for_protocol_buffer_limit_changes()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_5_ADAPTER_ID.into();
    initial.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into()]);
    initial.plugins.buffer_limits.protocol_response_bytes = 4096;
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let before_generation = server.runtime.active_generation().generation_id;
    let before_bindings = server.listener_bindings();

    let mut updated = initial.clone();
    updated.plugins.buffer_limits.protocol_response_bytes = 8192;
    updated.profiles.default_gameplay = "readonly".into();
    updated.admin.ui_profile = "console-v2".into();
    write_server_toml_for_reload(&config_path, &updated)?;

    let result = server.reload_runtime_full().await?;
    assert_ne!(result.topology.activated_generation_id, before_generation);
    assert!(!result.topology.applied_config_change);
    assert_eq!(server.listener_bindings(), before_bindings);
    assert_eq!(
        server
            .runtime
            .active_generation()
            .config
            .plugins
            .buffer_limits
            .protocol_response_bytes,
        8192
    );
    assert_eq!(
        server
            .runtime
            .selection_state()
            .await
            .config
            .plugins
            .buffer_limits
            .protocol_response_bytes,
        8192
    );

    server.shutdown().await
}

#[tokio::test]
async fn generation_reload_ignores_pending_protocol_buffer_limit_changes()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_5_ADAPTER_ID.into();
    initial.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into()]);
    initial.plugins.buffer_limits.protocol_response_bytes = 4096;
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let before_generation = server.runtime.active_generation().generation_id;
    let before_bindings = server.listener_bindings();

    let mut updated = initial.clone();
    updated.plugins.buffer_limits.protocol_response_bytes = 8192;
    write_server_toml_for_reload(&config_path, &updated)?;

    let result = server.reload_runtime_topology().await?;
    assert_eq!(result.activated_generation_id, before_generation);
    assert!(!result.changed(before_generation));
    assert_eq!(server.listener_bindings(), before_bindings);
    assert_eq!(
        server
            .runtime
            .active_generation()
            .config
            .plugins
            .buffer_limits
            .protocol_response_bytes,
        4096
    );
    assert_eq!(
        server
            .runtime
            .selection_state()
            .await
            .config
            .plugins
            .buffer_limits
            .protocol_response_bytes,
        4096
    );
    assert_eq!(
        server
            .runtime
            .active_generation()
            .config
            .profiles
            .default_gameplay,
        initial.profiles.default_gameplay
    );
    assert_eq!(
        server.runtime.active_generation().config.admin.ui_profile,
        initial.admin.ui_profile
    );
    assert_eq!(
        server
            .runtime
            .selection_state()
            .await
            .config
            .profiles
            .default_gameplay,
        initial.profiles.default_gameplay
    );
    assert_eq!(
        server
            .runtime
            .selection_state()
            .await
            .config
            .admin
            .ui_profile,
        initial.admin.ui_profile
    );

    server.shutdown().await
}

#[tokio::test]
async fn plugin_reload_ignores_pending_generation_config_changes() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.network.motd = "plugin-reload-before".to_string();
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir, &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let before_generation = server.runtime.active_generation().generation_id;
    let before_bindings = server.listener_bindings();
    let before_status = server.status().await;
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await?;

    let mut updated = initial.clone();
    updated.network.server_port = occupied.local_addr()?.port();
    updated.network.motd = "plugin-reload-after".to_string();
    write_server_toml_for_reload(&config_path, &updated)?;

    let reloaded = server.reload_runtime_artifacts().await?.reloaded_plugin_ids;
    assert!(reloaded.is_empty());

    let after_status = server.status().await;
    assert_eq!(
        server.runtime.active_generation().generation_id,
        before_generation
    );
    assert_eq!(server.listener_bindings(), before_bindings);
    assert_eq!(
        after_status.active_generation.generation_id,
        before_status.active_generation.generation_id
    );
    assert_eq!(after_status.active_generation.motd, "plugin-reload-before");
    assert_eq!(
        after_status.active_generation.listener_bindings,
        before_status.active_generation.listener_bindings
    );

    server.shutdown().await
}

#[tokio::test]
async fn topology_reload_invalid_candidate_keeps_existing_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_5_ADAPTER_ID.into();
    initial.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into()]);
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let before_generation = server.runtime.active_generation().generation_id;

    let mut invalid = initial.clone();
    invalid.topology.default_adapter = "missing-adapter".into();
    invalid.topology.enabled_adapters = Some(vec!["missing-adapter".into()]);
    write_server_toml_for_reload(&config_path, &invalid)?;

    let error = server
        .reload_runtime_topology()
        .await
        .expect_err("invalid topology candidate should fail");
    assert!(matches!(
        error,
        RuntimeError::Config(ref message) if message.contains("unknown default-adapter")
    ));
    assert_eq!(
        server.runtime.active_generation().generation_id,
        before_generation
    );

    server.shutdown().await
}

#[tokio::test]
async fn topology_reload_status_reports_draining_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_5_ADAPTER_ID.into();
    initial.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into(), JE_47_ADAPTER_ID.into()]);
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID])?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let (_stream, _buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "topology-status")
            .await?;
    let before_generation = server.runtime.active_generation().generation_id;

    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_47_ADAPTER_ID.into();
    write_server_toml_for_reload(&config_path, &updated)?;
    let result = server.reload_runtime_topology().await?;
    assert_ne!(result.activated_generation_id, before_generation);

    let status = server.status().await;
    assert_eq!(
        status.active_generation.generation_id,
        result.activated_generation_id
    );
    assert_eq!(
        status.active_generation.state,
        GenerationStatusState::Active
    );
    assert_eq!(status.draining_generations.len(), 1);
    assert_eq!(
        status.draining_generations[0].generation_id,
        before_generation
    );
    assert_eq!(
        status.draining_generations[0].state,
        GenerationStatusState::Draining
    );
    assert!(status.draining_generations[0].drain_deadline_ms.is_some());

    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].generation_id, before_generation);
    assert!(
        status
            .session_summary
            .by_generation
            .iter()
            .any(|entry| entry.generation_id == before_generation && entry.count == 1)
    );

    server.shutdown().await
}

#[tokio::test]
async fn topology_reload_zero_grace_disconnects_old_play_sessions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_5_ADAPTER_ID.into();
    initial.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into(), JE_47_ADAPTER_ID.into()]);
    initial.topology.drain_grace_secs = 0;
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID])?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let (_stream, _buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "topodrain").await?;
    assert_eq!(server.runtime.sessions.len().await, 1);

    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_47_ADAPTER_ID.into();
    write_server_toml_for_reload(&config_path, &updated)?;
    let _ = server.reload_runtime_topology().await?;

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if server.runtime.sessions.is_empty().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("old play session should be drained once grace expires");

    server.shutdown().await
}

#[tokio::test]
async fn topology_reload_retains_draining_generation_while_old_accepts_remain_queued()
-> Result<(), RuntimeError> {
    let (_temp_dir, server, old_generation, queued_accept) =
        reload_server_with_queued_old_accept(30).await?;

    assert!(
        server
            .status()
            .await
            .draining_generations
            .iter()
            .any(|generation| generation.generation_id == old_generation)
    );

    drop(queued_accept);
    let retired = server.runtime.retire_drained_generations().await;
    assert_eq!(retired, vec![old_generation]);

    server.shutdown().await
}

#[tokio::test]
async fn topology_reload_admits_old_generation_accept_during_drain_grace()
-> Result<(), RuntimeError> {
    let (_temp_dir, server, old_generation, queued_accept) =
        reload_server_with_queued_old_accept(30).await?;
    let (_client, accepted_session) = synthetic_tcp_transport_session().await?;
    server
        .runtime
        .spawn_accepted_transport_session(AcceptedGenerationSession::new(
            old_generation,
            accepted_session,
            queued_accept,
        ))
        .await;

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if server
                .session_status()
                .await
                .iter()
                .any(|session| session.generation_id == old_generation)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| RuntimeError::Config("old-generation queued accept was not admitted".into()))?;
    assert!(
        server
            .status()
            .await
            .draining_generations
            .iter()
            .any(|generation| generation.generation_id == old_generation)
    );

    server.shutdown().await
}

#[tokio::test]
async fn topology_reload_drops_old_generation_accept_after_drain_deadline()
-> Result<(), RuntimeError> {
    let (_temp_dir, server, old_generation, queued_accept) =
        reload_server_with_queued_old_accept(0).await?;
    let (_client, accepted_session) = synthetic_tcp_transport_session().await?;
    server
        .runtime
        .spawn_accepted_transport_session(AcceptedGenerationSession::new(
            old_generation,
            accepted_session,
            queued_accept,
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(
        server
            .session_status()
            .await
            .iter()
            .all(|session| session.generation_id != old_generation)
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if server.runtime.generation(old_generation).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| RuntimeError::Config("expired draining generation was not retired".into()))?;

    server.shutdown().await
}
