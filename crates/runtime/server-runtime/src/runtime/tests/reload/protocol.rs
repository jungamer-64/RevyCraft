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
        connect_and_login_java_client(addr, &codec, 5, "protohot", 0x30, 12).await?;
    let _ = read_until_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x09, 12).await?;

    std::thread::sleep(Duration::from_secs(1));
    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .install_protocol_plugin(
            "mc-plugin-proto-je-1_7_10-reload-test",
            JE_1_7_10_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        reloaded
            .iter()
            .any(|plugin_id| plugin_id == JE_1_7_10_ADAPTER_ID),
        "protocol reload should report a generation swap"
    );

    let adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_1_7_10_ADAPTER_ID)
        .expect("runtime should still resolve the adapter after reload");
    assert_ne!(adapter.plugin_generation_id(), Some(before_generation));
    assert!(
        adapter
            .capability_set()
            .contains("build-tag:protocol-reload-v2")
    );

    write_packet(&mut alpha, &codec, &held_item_change(4)).await?;
    let held_item = read_until_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x09, 8).await?;
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
        connect_and_login_java_client(addr, &codec, 5, "protofail", 0x30, 12).await?;
    let _ = read_until_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x09, 12).await?;

    std::thread::sleep(Duration::from_secs(1));
    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .install_protocol_plugin(
            "mc-plugin-proto-je-1_7_10-reload-test",
            JE_1_7_10_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-fail",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        !reloaded
            .iter()
            .any(|plugin_id| plugin_id == JE_1_7_10_ADAPTER_ID),
        "failed protocol migration should keep the current generation"
    );

    let adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_1_7_10_ADAPTER_ID)
        .expect("runtime should still resolve the adapter after failed reload");
    assert_eq!(adapter.plugin_generation_id(), Some(before_generation));
    assert!(
        adapter
            .capability_set()
            .contains("build-tag:protocol-reload-v1")
    );

    write_packet(&mut alpha, &codec, &held_item_change(6)).await?;
    let held_item = read_until_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x09, 8).await?;
    assert_eq!(held_item_from_packet(&held_item)?, 6);

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
        &[
            JE_1_7_10_ADAPTER_ID,
            JE_1_8_X_ADAPTER_ID,
            JE_1_12_2_ADAPTER_ID,
        ],
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
    let server = build_reloadable_test_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            online_mode: true,
            auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_8_X_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_from_dist_with_supporting_plugins(
            dist_dir,
            &[
                JE_1_7_10_ADAPTER_ID,
                JE_1_8_X_ADAPTER_ID,
                JE_1_12_2_ADAPTER_ID,
            ],
            &["storage-je-anvil-1_7_10", ONLINE_STUB_AUTH_PLUGIN_ID],
        )?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    for (protocol_version, username, expected_packet_id) in [
        (5, "packaged-legacy", 0x30),
        (47, "packaged-middle", 0x30),
        (340, "packaged-latest", 0x14),
    ] {
        let mut stream = connect_tcp(addr).await?;
        let (mut encryption, mut buffer) =
            perform_online_login(&mut stream, &codec, protocol_version, username).await?;
        let login_success = read_until_packet_id_encrypted(
            &mut stream,
            &codec,
            &mut buffer,
            0x02,
            8,
            &mut encryption,
        )
        .await?;
        assert_eq!(packet_id(&login_success), 0x02);

        let bootstrap = read_until_packet_id_encrypted(
            &mut stream,
            &codec,
            &mut buffer,
            expected_packet_id,
            24,
            &mut encryption,
        )
        .await?;
        assert_eq!(packet_id(&bootstrap), expected_packet_id);
    }

    server.shutdown().await
}
