use super::*;

fn topology_reload_server_config(world_dir: PathBuf, dist_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.plugins_dir = dist_dir;
    config
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
    let before_topology = server.runtime.active_topology();
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

    let result = server.reload_topology().await?;
    assert_ne!(
        result.activated_generation_id,
        before_topology.generation_id
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
    let before_generation = server.runtime.active_topology().generation_id;
    assert_eq!(server.listener_bindings().len(), 1);

    std::thread::sleep(Duration::from_secs(1));
    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_1_8_X_ADAPTER_ID.to_string();
    updated.topology.be_enabled = true;
    updated.topology.default_bedrock_adapter = BE_26_3_ADAPTER_ID.to_string();
    updated.topology.enabled_bedrock_adapters = Some(vec![BE_26_3_ADAPTER_ID.to_string()]);
    write_server_toml(&config_path, &updated)?;

    let result = server.reload_topology().await?;
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
            .active_topology()
            .default_adapter
            .descriptor()
            .adapter_id,
        JE_1_8_X_ADAPTER_ID
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
    let before_generation = server.runtime.active_topology().generation_id;

    std::thread::sleep(Duration::from_secs(1));
    let mut invalid = initial.clone();
    invalid.topology.default_adapter = "missing-adapter".to_string();
    invalid.topology.enabled_adapters = Some(vec!["missing-adapter".to_string()]);
    write_server_toml(&config_path, &invalid)?;

    let error = server
        .reload_topology()
        .await
        .expect_err("invalid topology candidate should fail");
    assert!(matches!(
        error,
        RuntimeError::Config(ref message) if message.contains("unknown default-adapter")
    ));
    assert_eq!(
        server.runtime.active_topology().generation_id,
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
        connect_and_login_java_client(addr, &codec, 5, "topology-status", 0x30, 12).await?;
    let before_generation = server.runtime.active_topology().generation_id;

    std::thread::sleep(Duration::from_secs(1));
    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_1_8_X_ADAPTER_ID.to_string();
    write_server_toml(&config_path, &updated)?;
    let result = server.reload_topology().await?;
    assert_ne!(result.activated_generation_id, before_generation);

    let status = server.status().await;
    assert_eq!(
        status.active_topology.generation_id,
        result.activated_generation_id
    );
    assert_eq!(status.active_topology.state, TopologyStatusState::Active);
    assert_eq!(status.draining_topologies.len(), 1);
    assert_eq!(
        status.draining_topologies[0].generation_id,
        before_generation
    );
    assert_eq!(
        status.draining_topologies[0].state,
        TopologyStatusState::Draining
    );
    assert!(status.draining_topologies[0].drain_deadline_ms.is_some());

    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].topology_generation_id, before_generation);
    assert!(
        status
            .session_summary
            .by_topology_generation
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
        connect_and_login_java_client(addr, &codec, 5, "topodrain", 0x30, 12).await?;
    assert_eq!(server.runtime.sessions.lock().await.len(), 1);

    std::thread::sleep(Duration::from_secs(1));
    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_1_8_X_ADAPTER_ID.to_string();
    write_server_toml(&config_path, &updated)?;
    let _ = server.reload_topology().await?;

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
