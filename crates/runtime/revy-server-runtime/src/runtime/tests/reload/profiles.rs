use super::*;

fn gameplay_reload_server_config(world_dir: PathBuf, dist_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.game_mode = 1;
    config.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into(), JE_340_ADAPTER_ID.into()]);
    config.profiles.default_gameplay = "canonical".into();
    config.profiles.gameplay_map = gameplay_profile_map(&[
        (JE_5_ADAPTER_ID, "readonly"),
        (JE_340_ADAPTER_ID, "canonical"),
    ]);
    config.bootstrap.plugins_dir = dist_dir;
    config
}

fn packaged_reload_server_config(world_dir: PathBuf, dist_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.game_mode = 1;
    config.bootstrap.plugins_dir = dist_dir;
    config
}

#[tokio::test]
async fn gameplay_reload_updates_target_profile_generation_only() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("gameplay-reload-success");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_5_ADAPTER_ID, JE_340_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let registries =
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID, JE_340_ADAPTER_ID])?;
    let server = build_reloadable_test_server(
        gameplay_reload_server_config(temp_dir.path().join("world"), dist_dir.clone()),
        registries,
    )
    .await?;
    let canonical_before = loaded_plugins_snapshot(&server)
        .await
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should resolve");
    let readonly_before = loaded_plugins_snapshot(&server)
        .await
        .resolve_gameplay_profile("readonly")
        .expect("readonly gameplay profile should resolve");
    let canonical_generation = canonical_before
        .plugin_generation_id()
        .expect("canonical profile should report generation");
    let readonly_generation = readonly_before
        .plugin_generation_id()
        .expect("readonly profile should report generation");
    assert_eq!(
        gameplay_build_tag(&server, "canonical").as_deref(),
        Some(PACKAGED_PLUGIN_TEST_HARNESS_TAG)
    );

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "legacy-observer")
            .await?;
    let (mut modern, _modern_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je340, "modern-reload")
            .await?;
    let _ = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;

    harness
        .install_gameplay_plugin_for_reload(
            "mc-plugin-gameplay-canonical",
            "gameplay-canonical",
            &dist_dir,
            &target_dir,
            "gameplay-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = server.reload_runtime_artifacts().await?.reloaded_plugin_ids;
    assert!(
        reloaded
            .iter()
            .any(|plugin_id| plugin_id == "gameplay-canonical"),
        "gameplay reload should report canonical plugin reload"
    );

    let canonical_after = loaded_plugins_snapshot(&server)
        .await
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should still resolve");
    let readonly_after = loaded_plugins_snapshot(&server)
        .await
        .resolve_gameplay_profile("readonly")
        .expect("readonly gameplay profile should still resolve");
    assert_ne!(
        canonical_after.plugin_generation_id(),
        Some(canonical_generation)
    );
    assert_eq!(
        readonly_after.plugin_generation_id(),
        Some(readonly_generation)
    );
    assert_eq!(
        gameplay_build_tag(&server, "canonical").as_deref(),
        Some("gameplay-reload-v2")
    );

    write_packet(
        &mut modern,
        &codec,
        &player_position_look_1_12(18.5, 4.0, 0.5, 30.0, 0.0),
    )
    .await?;
    let legacy_teleport = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::EntityTeleport,
    )
    .await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);

    server.shutdown().await
}

#[tokio::test]
async fn gameplay_reload_failure_keeps_existing_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("gameplay-reload-failure");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_5_ADAPTER_ID, JE_340_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let registries =
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID, JE_340_ADAPTER_ID])?;
    let server = build_reloadable_test_server(
        gameplay_reload_server_config(temp_dir.path().join("world"), dist_dir.clone()),
        registries,
    )
    .await?;
    let canonical_before = loaded_plugins_snapshot(&server)
        .await
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should resolve");
    let before_generation = canonical_before
        .plugin_generation_id()
        .expect("canonical profile should report generation");
    assert_eq!(
        gameplay_build_tag(&server, "canonical").as_deref(),
        Some(PACKAGED_PLUGIN_TEST_HARNESS_TAG)
    );

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "legacy-failure")
            .await?;
    let (mut modern, _modern_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je340, "modern-failure")
            .await?;
    let _ = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;

    harness
        .install_gameplay_plugin_for_reload(
            "mc-plugin-gameplay-canonical",
            "gameplay-canonical",
            &dist_dir,
            &target_dir,
            "gameplay-reload-fail",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = server.reload_runtime_artifacts().await?.reloaded_plugin_ids;
    assert!(
        !reloaded
            .iter()
            .any(|plugin_id| plugin_id == "gameplay-canonical"),
        "failed gameplay migration should not swap the canonical generation"
    );

    let canonical_after = loaded_plugins_snapshot(&server)
        .await
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should still resolve");
    assert_eq!(
        canonical_after.plugin_generation_id(),
        Some(before_generation)
    );
    assert_eq!(
        gameplay_build_tag(&server, "canonical").as_deref(),
        Some(PACKAGED_PLUGIN_TEST_HARNESS_TAG)
    );

    write_packet(
        &mut modern,
        &codec,
        &player_position_look_1_12(22.5, 4.0, 0.5, 45.0, 0.0),
    )
    .await?;
    let legacy_teleport = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::EntityTeleport,
    )
    .await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);

    server.shutdown().await
}

#[tokio::test]
async fn storage_reload_updates_generation_and_preserves_persistence() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("storage-reload-success");
    let world_dir = temp_dir.path().join("world");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    let registries = plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?;
    let server = build_reloadable_test_server(
        packaged_reload_server_config(world_dir.clone(), dist_dir.clone()),
        registries,
    )
    .await?;
    let storage_before = loaded_plugins_snapshot(&server)
        .await
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should resolve");
    let before_generation = storage_before
        .plugin_generation_id()
        .expect("storage profile should report generation");
    assert_eq!(
        storage_build_tag(&server, JE_1_7_10_STORAGE_PROFILE_ID).as_deref(),
        Some(PACKAGED_PLUGIN_TEST_HARNESS_TAG)
    );

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let (mut stream, mut buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "alpha").await?;
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je5, 36, 20, 64, 0),
    )
    .await?;
    let _ = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::SetSlot,
    )
    .await?;

    harness
        .install_storage_plugin_for_reload(
            "mc-plugin-storage-je-anvil-1_7_10",
            "storage-je-anvil-1_7_10",
            &dist_dir,
            &target_dir,
            "storage-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = server.reload_runtime_artifacts().await?.reloaded_plugin_ids;
    assert!(
        reloaded
            .iter()
            .any(|plugin_id| plugin_id == "storage-je-anvil-1_7_10"),
        "storage reload should report generation swap"
    );

    let storage_after = loaded_plugins_snapshot(&server)
        .await
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should still resolve");
    assert_ne!(
        storage_after.plugin_generation_id(),
        Some(before_generation)
    );
    assert_eq!(
        storage_build_tag(&server, JE_1_7_10_STORAGE_PROFILE_ID).as_deref(),
        Some("storage-reload-v2")
    );

    server.shutdown().await?;

    let restarted = build_test_server(
        packaged_reload_server_config(world_dir.clone(), dist_dir.clone()),
        plugin_test_registries_from_dist(dist_dir, &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let addr = listener_addr(&restarted);
    let mut stream = connect_tcp(addr).await?;
    write_packet(
        &mut stream,
        &codec,
        &encode_handshake(TestJavaProtocol::Je5.protocol_version(), 2)?,
    )
    .await?;
    write_packet(&mut stream, &codec, &login_start("alpha")).await?;
    let mut buffer = BytesMut::new();
    let window_items = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::WindowItems,
    )
    .await?;
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je5, &window_items, 36)?,
        Some((20, 64, 0))
    );

    restarted.shutdown().await
}

#[tokio::test]
async fn storage_reload_failure_keeps_existing_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("storage-reload-failure");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.plugins_dir = dist_dir.clone();
    config.plugins.allowlist = Some(plugin_allowlist_with_supporting_plugins(
        &[JE_5_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    ));
    config.plugins.failure_policy.storage = PluginFailureAction::Skip;
    let server =
        build_reloadable_test_server(config.clone(), plugin_test_registries_from_config(&config)?)
            .await?;
    let storage_before = loaded_plugins_snapshot(&server)
        .await
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should resolve");
    let before_generation = storage_before
        .plugin_generation_id()
        .expect("storage profile should report generation");

    harness
        .install_storage_plugin_for_reload(
            "mc-plugin-storage-je-anvil-1_7_10",
            "storage-je-anvil-1_7_10",
            &dist_dir,
            &target_dir,
            "storage-reload-fail",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = server.reload_runtime_artifacts().await?.reloaded_plugin_ids;
    assert!(
        !reloaded
            .iter()
            .any(|plugin_id| plugin_id == "storage-je-anvil-1_7_10"),
        "failed storage migration should not swap the storage generation"
    );

    let storage_after = loaded_plugins_snapshot(&server)
        .await
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should still resolve");
    assert_eq!(
        storage_after.plugin_generation_id(),
        Some(before_generation)
    );
    assert_eq!(
        storage_build_tag(&server, JE_1_7_10_STORAGE_PROFILE_ID).as_deref(),
        Some(PACKAGED_PLUGIN_TEST_HARNESS_TAG)
    );

    server.shutdown().await
}

#[tokio::test]
async fn config_reload_updates_storage_generation_for_buffer_limit_changes()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    let mut initial =
        packaged_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.plugins.buffer_limits.storage_response_bytes = 4096;
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let before_protocol_generation = server.runtime.active_generation().generation_id;
    let storage_before = loaded_plugins_snapshot(&server)
        .await
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should resolve");
    let before_storage_generation = storage_before
        .plugin_generation_id()
        .expect("storage profile should report generation");

    let mut updated = initial.clone();
    updated.plugins.buffer_limits.storage_response_bytes = 8192;
    write_server_toml_for_reload(&config_path, &updated)?;

    let result = server.reload_runtime_full().await?;
    assert_eq!(
        result.topology.activated_generation_id,
        before_protocol_generation
    );
    let storage_after = loaded_plugins_snapshot(&server)
        .await
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should still resolve");
    assert_ne!(
        storage_after.plugin_generation_id(),
        Some(before_storage_generation)
    );
    assert_eq!(
        server
            .runtime
            .selection_state()
            .await
            .config
            .plugins
            .buffer_limits
            .storage_response_bytes,
        8192
    );

    server.shutdown().await
}

#[tokio::test]
async fn auth_reload_updates_generation_for_new_logins_only() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("auth-reload-offline");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    let server = build_reloadable_test_server(
        packaged_reload_server_config(temp_dir.path().join("world"), dist_dir.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let auth_before = loaded_plugins_snapshot(&server)
        .await
        .resolve_auth_profile(OFFLINE_AUTH_PROFILE_ID)
        .expect("auth profile should resolve");
    let before_generation = auth_before
        .plugin_generation_id()
        .expect("auth profile should report generation");
    assert_eq!(
        auth_build_tag(&server, OFFLINE_AUTH_PROFILE_ID).as_deref(),
        Some(PACKAGED_PLUGIN_TEST_HARNESS_TAG)
    );

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let (mut alpha, mut alpha_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "alpha").await?;
    let _ = read_until_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;

    harness
        .install_auth_plugin_for_reload(
            "mc-plugin-auth-offline",
            "auth-offline",
            &dist_dir,
            &target_dir,
            "auth-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = server.reload_runtime_artifacts().await?.reloaded_plugin_ids;
    assert!(
        reloaded.iter().any(|plugin_id| plugin_id == "auth-offline"),
        "auth reload should report generation swap"
    );

    let auth_after = loaded_plugins_snapshot(&server)
        .await
        .resolve_auth_profile(OFFLINE_AUTH_PROFILE_ID)
        .expect("auth profile should still resolve");
    assert_ne!(auth_after.plugin_generation_id(), Some(before_generation));
    assert_eq!(
        auth_build_tag(&server, OFFLINE_AUTH_PROFILE_ID).as_deref(),
        Some("auth-reload-v2")
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

    let mut beta = connect_tcp(addr).await?;
    write_packet(
        &mut beta,
        &codec,
        &encode_handshake(TestJavaProtocol::Je5.protocol_version(), 2)?,
    )
    .await?;
    write_packet(&mut beta, &codec, &login_start("beta")).await?;
    let mut beta_buffer = BytesMut::new();
    let login_success = read_until_java_packet(
        &mut beta,
        &codec,
        &mut beta_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::LoginSuccess,
    )
    .await?;
    assert_eq!(packet_id(&login_success), 0x02);

    server.shutdown().await
}
