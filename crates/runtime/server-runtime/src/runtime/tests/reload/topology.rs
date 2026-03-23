use super::*;
use crate::runtime::{GenerationId, RunningServer};

fn topology_reload_server_config(world_dir: PathBuf, dist_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.plugins_dir = dist_dir;
    config
}

async fn reload_server_with_queued_old_accept(
    drain_grace_secs: u64,
) -> Result<(tempfile::TempDir, RunningServer, GenerationId), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID, JE_1_8_X_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_1_7_10_ADAPTER_ID.to_string();
    initial.topology.enabled_adapters = Some(vec![
        JE_1_7_10_ADAPTER_ID.to_string(),
        JE_1_8_X_ADAPTER_ID.to_string(),
    ]);
    initial.topology.drain_grace_secs = drain_grace_secs;
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(
            dist_dir.clone(),
            &[JE_1_7_10_ADAPTER_ID, JE_1_8_X_ADAPTER_ID],
        )?,
    )
    .await?;
    let old_generation = server.runtime.active_generation().generation_id;
    server.runtime.queued_accepts.increment(old_generation);

    std::thread::sleep(Duration::from_secs(1));
    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_1_8_X_ADAPTER_ID.to_string();
    write_server_toml(&config_path, &updated)?;
    let result = server.reload_generation().await?;
    assert_ne!(result.activated_generation_id, old_generation);

    Ok((temp_dir, server, old_generation))
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
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-1_7_10-reload-test",
            JE_1_7_10_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-v1",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let server = build_reloadable_test_server(
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let before_generation = server.runtime.active_generation();
    let before_adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_1_7_10_ADAPTER_ID)
        .expect("runtime should resolve the initial adapter");
    let before_protocol_number = before_adapter.descriptor().protocol_number;

    std::thread::sleep(Duration::from_secs(1));
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-1_7_10-reload-test",
            JE_1_7_10_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "reload-incompatible",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let result = server.reload_generation().await?;
    assert_ne!(
        result.activated_generation_id,
        before_generation.generation_id
    );
    assert!(
        result
            .reconfigured_adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_1_7_10_ADAPTER_ID)
    );

    let after_adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_1_7_10_ADAPTER_ID)
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
        &[
            JE_1_7_10_ADAPTER_ID,
            JE_1_8_X_ADAPTER_ID,
            BE_26_3_ADAPTER_ID,
        ],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_1_7_10_ADAPTER_ID.to_string();
    initial.topology.enabled_adapters = Some(vec![
        JE_1_7_10_ADAPTER_ID.to_string(),
        JE_1_8_X_ADAPTER_ID.to_string(),
    ]);
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(
            dist_dir.clone(),
            &[
                JE_1_7_10_ADAPTER_ID,
                JE_1_8_X_ADAPTER_ID,
                BE_26_3_ADAPTER_ID,
            ],
        )?,
    )
    .await?;
    let before_generation = server.runtime.active_generation().generation_id;
    assert_eq!(server.listener_bindings().len(), 1);

    std::thread::sleep(Duration::from_secs(1));
    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_1_8_X_ADAPTER_ID.to_string();
    updated.topology.be_enabled = true;
    updated.topology.default_bedrock_adapter = BE_26_3_ADAPTER_ID.to_string();
    updated.topology.enabled_bedrock_adapters = Some(vec![BE_26_3_ADAPTER_ID.to_string()]);
    write_server_toml(&config_path, &updated)?;

    let result = server.reload_generation().await?;
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
        JE_1_8_X_ADAPTER_ID
    );

    server.shutdown().await
}

#[tokio::test]
async fn plugin_reload_ignores_pending_generation_config_changes() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.network.motd = "plugin-reload-before".to_string();
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir, &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let before_generation = server.runtime.active_generation().generation_id;
    let before_bindings = server.listener_bindings();
    let before_status = server.status().await;
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await?;

    let mut updated = initial.clone();
    updated.network.server_port = occupied.local_addr()?.port();
    updated.network.motd = "plugin-reload-after".to_string();
    write_server_toml(&config_path, &updated)?;

    let reloaded = server.reload_plugins().await?;
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
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_1_7_10_ADAPTER_ID.to_string();
    initial.topology.enabled_adapters = Some(vec![JE_1_7_10_ADAPTER_ID.to_string()]);
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let before_generation = server.runtime.active_generation().generation_id;

    std::thread::sleep(Duration::from_secs(1));
    let mut invalid = initial.clone();
    invalid.topology.default_adapter = "missing-adapter".to_string();
    invalid.topology.enabled_adapters = Some(vec!["missing-adapter".to_string()]);
    write_server_toml(&config_path, &invalid)?;

    let error = server
        .reload_generation()
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
        &[JE_1_7_10_ADAPTER_ID, JE_1_8_X_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_1_7_10_ADAPTER_ID.to_string();
    initial.topology.enabled_adapters = Some(vec![
        JE_1_7_10_ADAPTER_ID.to_string(),
        JE_1_8_X_ADAPTER_ID.to_string(),
    ]);
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(
            dist_dir.clone(),
            &[JE_1_7_10_ADAPTER_ID, JE_1_8_X_ADAPTER_ID],
        )?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let (_stream, _buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je1710, "topology-status")
            .await?;
    let before_generation = server.runtime.active_generation().generation_id;

    std::thread::sleep(Duration::from_secs(1));
    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_1_8_X_ADAPTER_ID.to_string();
    write_server_toml(&config_path, &updated)?;
    let result = server.reload_generation().await?;
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
        &[JE_1_7_10_ADAPTER_ID, JE_1_8_X_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut initial =
        topology_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_1_7_10_ADAPTER_ID.to_string();
    initial.topology.enabled_adapters = Some(vec![
        JE_1_7_10_ADAPTER_ID.to_string(),
        JE_1_8_X_ADAPTER_ID.to_string(),
    ]);
    initial.topology.drain_grace_secs = 0;
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(
            dist_dir.clone(),
            &[JE_1_7_10_ADAPTER_ID, JE_1_8_X_ADAPTER_ID],
        )?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let (_stream, _buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je1710, "topodrain").await?;
    assert_eq!(server.runtime.sessions.lock().await.len(), 1);

    std::thread::sleep(Duration::from_secs(1));
    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_1_8_X_ADAPTER_ID.to_string();
    write_server_toml(&config_path, &updated)?;
    let _ = server.reload_generation().await?;

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if server.runtime.sessions.lock().await.is_empty() {
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
    let (_temp_dir, server, old_generation) = reload_server_with_queued_old_accept(30).await?;

    assert!(
        server
            .status()
            .await
            .draining_generations
            .iter()
            .any(|generation| generation.generation_id == old_generation)
    );

    server.runtime.queued_accepts.decrement(old_generation);
    let retired = server.runtime.retire_drained_generations().await;
    assert_eq!(retired, vec![old_generation]);

    server.shutdown().await
}

#[tokio::test]
async fn topology_reload_admits_old_generation_accept_during_drain_grace()
-> Result<(), RuntimeError> {
    let (_temp_dir, server, old_generation) = reload_server_with_queued_old_accept(30).await?;

    server.runtime.queued_accepts.decrement(old_generation);
    let (_client, accepted_session) = synthetic_tcp_transport_session().await?;
    server
        .runtime
        .spawn_transport_session(old_generation, accepted_session)
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

    server.shutdown().await
}

#[tokio::test]
async fn topology_reload_drops_old_generation_accept_after_drain_deadline()
-> Result<(), RuntimeError> {
    let (_temp_dir, server, old_generation) = reload_server_with_queued_old_accept(0).await?;

    server.runtime.queued_accepts.decrement(old_generation);
    let (_client, accepted_session) = synthetic_tcp_transport_session().await?;
    server
        .runtime
        .spawn_transport_session(old_generation, accepted_session)
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
