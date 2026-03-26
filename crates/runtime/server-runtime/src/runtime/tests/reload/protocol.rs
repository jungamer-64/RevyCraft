use super::*;

#[tokio::test]
async fn protocol_reload_updates_generation_and_preserves_live_sessions() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let (server, dist_dir, target_dir, before_generation) =
        spawn_protocol_reload_server(&temp_dir, "protocol-reload-success").await?;
    let codec = MinecraftWireCodec;
    let addr = listener_addr(&server);
    let (mut alpha, mut alpha_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "protohot").await?;
    let _ = read_until_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;

    std::thread::sleep(Duration::from_secs(1));
    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            JE_5_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = server.reload_runtime_artifacts().await?.reloaded_plugin_ids;
    assert!(
        reloaded
            .iter()
            .any(|plugin_id| plugin_id == JE_5_ADAPTER_ID),
        "protocol reload should report a generation swap"
    );

    let adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_5_ADAPTER_ID)
        .expect("runtime should still resolve the adapter after reload");
    assert_ne!(adapter.plugin_generation_id(), Some(before_generation));
    assert_eq!(
        protocol_build_tag(&server, JE_5_ADAPTER_ID).as_deref(),
        Some("protocol-reload-v2")
    );

    write_packet(&mut alpha, &codec, &held_item_change(4)).await?;
    let held_item = read_until_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;
    assert_eq!(held_item_from_packet(&held_item)?, 4);

    server.shutdown().await
}

#[tokio::test]
async fn manual_protocol_reload_waits_for_consistency_readers() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let (server, dist_dir, target_dir, before_generation) =
        spawn_protocol_reload_server(&temp_dir, "protocol-reload-consistency-manual").await?;
    let consistency_guard = server.runtime.reload.read_consistency().await;

    std::thread::sleep(Duration::from_secs(1));
    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            JE_5_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    {
        let reload = server.reload_runtime_artifacts();
        tokio::pin!(reload);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut reload)
                .await
                .is_err(),
            "manual reload should wait for in-flight consistency readers"
        );

        let adapter = active_protocol_registry(&server)
            .resolve_adapter(JE_5_ADAPTER_ID)
            .expect("runtime should still resolve the adapter");
        assert_eq!(adapter.plugin_generation_id(), Some(before_generation));

        drop(consistency_guard);

        let reloaded = tokio::time::timeout(Duration::from_secs(3), &mut reload)
            .await
            .map_err(|_| RuntimeError::Config("manual reload did not resume".to_string()))??;
        assert!(
            reloaded
                .reloaded_plugin_ids
                .iter()
                .any(|plugin_id| plugin_id == JE_5_ADAPTER_ID),
            "manual reload should complete after the consistency reader releases"
        );
    }

    server.shutdown().await
}

#[tokio::test]
async fn consistency_gate_write_lock_blocks_session_commands() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let (server, _dist_dir, _target_dir, _before_generation) =
        spawn_protocol_reload_server(&temp_dir, "protocol-reload-command-block").await?;
    let codec = MinecraftWireCodec;
    let addr = listener_addr(&server);
    let (mut alpha, mut alpha_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "protoblock").await?;
    let _ = read_until_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;

    let consistency_guard = server.runtime.reload.write_consistency().await;
    write_packet(&mut alpha, &codec, &held_item_change(4)).await?;
    assert_no_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;
    drop(consistency_guard);

    let held_item = read_until_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;
    assert_eq!(held_item_from_packet(&held_item)?, 4);

    server.shutdown().await
}

#[tokio::test]
async fn protocol_reload_failure_keeps_existing_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let (server, dist_dir, target_dir, before_generation) =
        spawn_protocol_reload_server(&temp_dir, "protocol-reload-failure").await?;
    let codec = MinecraftWireCodec;
    let addr = listener_addr(&server);
    let (mut alpha, mut alpha_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "protofail").await?;
    let _ = read_until_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;

    std::thread::sleep(Duration::from_secs(1));
    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            JE_5_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-fail",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = server.reload_runtime_artifacts().await?.reloaded_plugin_ids;
    assert!(
        !reloaded
            .iter()
            .any(|plugin_id| plugin_id == JE_5_ADAPTER_ID),
        "failed protocol migration should keep the current generation"
    );

    let adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_5_ADAPTER_ID)
        .expect("runtime should still resolve the adapter after failed reload");
    assert_eq!(adapter.plugin_generation_id(), Some(before_generation));
    assert_eq!(
        protocol_build_tag(&server, JE_5_ADAPTER_ID).as_deref(),
        Some("protocol-reload-v1")
    );

    write_packet(&mut alpha, &codec, &held_item_change(6)).await?;
    let held_item = read_until_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;
    assert_eq!(held_item_from_packet(&held_item)?, 6);

    server.shutdown().await
}

#[tokio::test]
async fn protocol_reload_watch_waits_for_consistency_readers() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("protocol-reload-watch");
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
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.game_mode = 1;
    config.plugins.reload_watch = true;
    config.bootstrap.plugins_dir = dist_dir.clone();
    let server = build_reloadable_test_server(
        config,
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let before_generation = active_protocol_registry(&server)
        .resolve_adapter(JE_5_ADAPTER_ID)
        .and_then(|adapter| adapter.plugin_generation_id())
        .expect("watch server should report a protocol generation");

    let consistency_guard = server.runtime.reload.read_consistency().await;
    std::thread::sleep(Duration::from_secs(1));
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            JE_5_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    tokio::time::sleep(Duration::from_millis(
        mc_plugin_host::host::plugin_reload_poll_interval_ms() + 200,
    ))
    .await;
    let adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_5_ADAPTER_ID)
        .expect("watch server should still resolve the adapter");
    assert_eq!(
        adapter.plugin_generation_id(),
        Some(before_generation),
        "watch reload should be blocked while a consistency reader is active"
    );

    drop(consistency_guard);

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let adapter = active_protocol_registry(&server)
            .resolve_adapter(JE_5_ADAPTER_ID)
            .expect("watch server should resolve the adapter after reload");
        if adapter.plugin_generation_id() != Some(before_generation) {
            assert_eq!(
                protocol_build_tag(&server, JE_5_ADAPTER_ID).as_deref(),
                Some("protocol-reload-v2")
            );
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(RuntimeError::Config(
                "watch reload did not resume after the consistency reader released".to_string(),
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    server.shutdown().await
}

#[tokio::test]
async fn packaged_online_auth_stub_boot_supports_mixed_versions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("auth-online-packaged");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID, JE_340_ADAPTER_ID],
        &["storage-je-anvil-1_7_10", ONLINE_STUB_AUTH_PLUGIN_ID],
    )?;
    harness
        .install_auth_plugin(
            "mc-plugin-auth-online-stub",
            ONLINE_STUB_AUTH_PLUGIN_ID,
            &dist_dir,
            &target_dir,
            "online-stub-v1",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.online_mode = true;
    config.profiles.auth = ONLINE_STUB_AUTH_PROFILE_ID.into();
    config.topology.enabled_adapters = Some(vec![
        JE_5_ADAPTER_ID.into(),
        JE_47_ADAPTER_ID.into(),
        JE_340_ADAPTER_ID.into(),
    ]);
    config.bootstrap.plugins_dir = dist_dir.clone();
    let server = build_reloadable_test_server(
        config,
        plugin_test_registries_from_dist_with_supporting_plugins(
            dist_dir,
            &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID, JE_340_ADAPTER_ID],
            &["storage-je-anvil-1_7_10", ONLINE_STUB_AUTH_PLUGIN_ID],
        )?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    for (protocol, username) in [
        (TestJavaProtocol::Je5, "packaged-legacy"),
        (TestJavaProtocol::Je47, "packaged-middle"),
        (TestJavaProtocol::Je340, "packaged-latest"),
    ] {
        let mut stream = connect_tcp(addr).await?;
        let (mut encryption, mut buffer) =
            perform_online_login(&mut stream, &codec, protocol, username).await?;
        let login_success = read_until_java_packet_encrypted(
            &mut stream,
            &codec,
            &mut buffer,
            protocol,
            TestJavaPacket::LoginSuccess,
            8,
            &mut encryption,
        )
        .await?;
        assert_eq!(packet_id(&login_success), 0x02);

        let bootstrap = read_until_java_packet_encrypted(
            &mut stream,
            &codec,
            &mut buffer,
            protocol,
            TestJavaPacket::WindowItems,
            24,
            &mut encryption,
        )
        .await?;
        assert_eq!(packet_id(&bootstrap), protocol.window_items_packet_id());
    }

    server.shutdown().await
}
