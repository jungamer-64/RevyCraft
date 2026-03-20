use super::*;

#[cfg(target_os = "linux")]
#[tokio::test]
async fn gameplay_reload_updates_target_profile_generation_only() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("gameplay-reload-success");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    let registries = plugin_test_registries_from_dist(
        dist_dir.clone(),
        &[JE_1_7_10_ADAPTER_ID, JE_1_12_2_ADAPTER_ID],
    )?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            default_gameplay_profile: "canonical".to_string(),
            gameplay_profile_map: gameplay_profile_map(&[
                (JE_1_7_10_ADAPTER_ID, "readonly"),
                (JE_1_12_2_ADAPTER_ID, "canonical"),
            ]),
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        registries,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let canonical_before = plugin_host
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should resolve");
    let readonly_before = plugin_host
        .resolve_gameplay_profile("readonly")
        .expect("readonly gameplay profile should resolve");
    let canonical_generation = canonical_before
        .plugin_generation_id()
        .expect("canonical profile should report generation");
    let readonly_generation = readonly_before
        .plugin_generation_id()
        .expect("readonly profile should report generation");
    assert!(canonical_before.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, 5, "legacy-observer", 0x30, 12).await?;
    let (mut modern, _modern_buffer) =
        connect_and_login_java_client(addr, &codec, 340, "modern-reload", 0x14, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

    std::thread::sleep(Duration::from_secs(1));
    package_single_gameplay_plugin(
        "mc-plugin-gameplay-canonical",
        "gameplay-canonical",
        &dist_dir,
        &target_dir,
        "gameplay-reload-v2",
    )?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        reloaded
            .iter()
            .any(|plugin_id| plugin_id == "gameplay-canonical"),
        "gameplay reload should report canonical plugin reload"
    );

    let canonical_after = plugin_host
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should still resolve");
    let readonly_after = plugin_host
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
    assert!(
        canonical_after
            .capability_set()
            .contains("build-tag:gameplay-reload-v2")
    );

    write_packet(
        &mut modern,
        &codec,
        &player_position_look_1_12(18.5, 4.0, 0.5, 30.0, 0.0),
    )
    .await?;
    let legacy_teleport =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x18, 16).await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn gameplay_reload_failure_keeps_existing_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("gameplay-reload-failure");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    let registries = plugin_test_registries_from_dist(
        dist_dir.clone(),
        &[JE_1_7_10_ADAPTER_ID, JE_1_12_2_ADAPTER_ID],
    )?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            default_gameplay_profile: "canonical".to_string(),
            gameplay_profile_map: gameplay_profile_map(&[
                (JE_1_7_10_ADAPTER_ID, "readonly"),
                (JE_1_12_2_ADAPTER_ID, "canonical"),
            ]),
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        registries,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let canonical_before = plugin_host
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should resolve");
    let before_generation = canonical_before
        .plugin_generation_id()
        .expect("canonical profile should report generation");
    assert!(canonical_before.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, 5, "legacy-failure", 0x30, 12).await?;
    let (mut modern, _modern_buffer) =
        connect_and_login_java_client(addr, &codec, 340, "modern-failure", 0x14, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

    std::thread::sleep(Duration::from_secs(1));
    package_single_gameplay_plugin(
        "mc-plugin-gameplay-canonical",
        "gameplay-canonical",
        &dist_dir,
        &target_dir,
        "gameplay-reload-fail",
    )?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        !reloaded
            .iter()
            .any(|plugin_id| plugin_id == "gameplay-canonical"),
        "failed gameplay migration should not swap the canonical generation"
    );

    let canonical_after = plugin_host
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should still resolve");
    assert_eq!(
        canonical_after.plugin_generation_id(),
        Some(before_generation)
    );
    assert!(canonical_after.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    write_packet(
        &mut modern,
        &codec,
        &player_position_look_1_12(22.5, 4.0, 0.5, 45.0, 0.0),
    )
    .await?;
    let legacy_teleport =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x18, 16).await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn storage_reload_updates_generation_and_preserves_persistence() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("storage-reload-success");
    let world_dir = temp_dir.path().join("world");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    let registries = plugin_test_registries_from_dist(dist_dir.clone(), &[JE_1_7_10_ADAPTER_ID])?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            plugins_dir: dist_dir.clone(),
            world_dir: world_dir.clone(),
            ..ServerConfig::default()
        },
        registries,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let storage_before = plugin_host
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should resolve");
    let before_generation = storage_before
        .plugin_generation_id()
        .expect("storage profile should report generation");
    assert!(storage_before.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let (mut stream, mut buffer) =
        connect_and_login_java_client(addr, &codec, 5, "alpha", 0x30, 12).await?;
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(36, 20, 64, 0),
    )
    .await?;
    let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x2f, 8).await?;

    std::thread::sleep(Duration::from_secs(1));
    package_single_plugin(
        "mc-plugin-storage-je-anvil-1_7_10",
        "storage-je-anvil-1_7_10",
        "storage",
        &dist_dir,
        &target_dir,
        "storage-reload-v2",
    )?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        reloaded
            .iter()
            .any(|plugin_id| plugin_id == "storage-je-anvil-1_7_10"),
        "storage reload should report generation swap"
    );

    let storage_after = plugin_host
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should still resolve");
    assert_ne!(
        storage_after.plugin_generation_id(),
        Some(before_generation)
    );
    assert!(
        storage_after
            .capability_set()
            .contains("build-tag:storage-reload-v2")
    );

    server.shutdown().await?;

    let restarted = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            plugins_dir: dist_dir.clone(),
            world_dir: world_dir.clone(),
            ..ServerConfig::default()
        },
        plugin_test_registries_from_dist(dist_dir, &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let addr = listener_addr(&restarted);
    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start("alpha")).await?;
    let mut buffer = BytesMut::new();
    let window_items = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x30, 12).await?;
    assert_eq!(window_items_slot(&window_items, 36)?, Some((20, 64, 0)));

    restarted.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn storage_reload_failure_keeps_existing_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("storage-reload-failure");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let storage_before = plugin_host
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should resolve");
    let before_generation = storage_before
        .plugin_generation_id()
        .expect("storage profile should report generation");

    std::thread::sleep(Duration::from_secs(1));
    package_single_plugin(
        "mc-plugin-storage-je-anvil-1_7_10",
        "storage-je-anvil-1_7_10",
        "storage",
        &dist_dir,
        &target_dir,
        "storage-reload-fail",
    )?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        !reloaded
            .iter()
            .any(|plugin_id| plugin_id == "storage-je-anvil-1_7_10"),
        "failed storage migration should not swap the storage generation"
    );

    let storage_after = plugin_host
        .resolve_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID)
        .expect("storage profile should still resolve");
    assert_eq!(
        storage_after.plugin_generation_id(),
        Some(before_generation)
    );
    assert!(storage_after.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn auth_reload_updates_generation_for_new_logins_only() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("auth-reload-offline");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            game_mode: 1,
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let auth_before = plugin_host
        .resolve_auth_profile(OFFLINE_AUTH_PROFILE_ID)
        .expect("auth profile should resolve");
    let before_generation = auth_before
        .plugin_generation_id()
        .expect("auth profile should report generation");
    assert!(auth_before.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let (mut alpha, mut alpha_buffer) =
        connect_and_login_java_client(addr, &codec, 5, "alpha", 0x30, 12).await?;
    let _ = read_until_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x09, 12).await?;

    std::thread::sleep(Duration::from_secs(1));
    package_single_plugin(
        "mc-plugin-auth-offline",
        "auth-offline",
        "auth",
        &dist_dir,
        &target_dir,
        "auth-reload-v2",
    )?;

    let reloaded = server.reload_plugins().await?;
    assert!(
        reloaded.iter().any(|plugin_id| plugin_id == "auth-offline"),
        "auth reload should report generation swap"
    );

    let auth_after = plugin_host
        .resolve_auth_profile(OFFLINE_AUTH_PROFILE_ID)
        .expect("auth profile should still resolve");
    assert_ne!(auth_after.plugin_generation_id(), Some(before_generation));
    assert!(
        auth_after
            .capability_set()
            .contains("build-tag:auth-reload-v2")
    );

    write_packet(&mut alpha, &codec, &held_item_change(4)).await?;
    let held_item = read_until_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x09, 8).await?;
    assert_eq!(held_item_from_packet(&held_item)?, 4);

    let mut beta = connect_tcp(addr).await?;
    write_packet(&mut beta, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut beta, &codec, &login_start("beta")).await?;
    let mut beta_buffer = BytesMut::new();
    let login_success = read_until_packet_id(&mut beta, &codec, &mut beta_buffer, 0x02, 8).await?;
    assert_eq!(packet_id(&login_success), 0x02);

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn packaged_online_auth_stub_boot_supports_mixed_versions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("auth-online-packaged");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    package_single_plugin(
        "mc-plugin-auth-online-stub",
        ONLINE_STUB_AUTH_PLUGIN_ID,
        "auth",
        &dist_dir,
        &target_dir,
        "online-stub-v1",
    )?;
    let server = spawn_server(
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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn online_auth_reload_keeps_existing_challenge_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("auth-online-reload");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;
    package_single_plugin(
        "mc-plugin-auth-online-stub",
        ONLINE_STUB_AUTH_PLUGIN_ID,
        "auth",
        &dist_dir,
        &target_dir,
        "online-auth-v1",
    )?;
    let server = spawn_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            online_mode: true,
            auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
            plugins_dir: dist_dir.clone(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_from_dist_with_supporting_plugins(
            dist_dir.clone(),
            &[JE_1_7_10_ADAPTER_ID],
            &["storage-je-anvil-1_7_10", ONLINE_STUB_AUTH_PLUGIN_ID],
        )?,
    )
    .await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let auth_before = plugin_host
        .resolve_auth_profile(ONLINE_STUB_AUTH_PROFILE_ID)
        .expect("online auth profile should resolve");
    let before_generation = auth_before
        .plugin_generation_id()
        .expect("online auth profile should report generation");
    assert!(
        auth_before
            .capability_set()
            .contains("build-tag:online-auth-v1")
    );

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut alpha = connect_tcp(addr).await?;
    write_packet(&mut alpha, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut alpha, &codec, &login_start("alpha-online")).await?;
    let mut alpha_buffer = BytesMut::new();
    let request = read_packet(&mut alpha, &codec, &mut alpha_buffer).await?;
    let (_server_id, public_key_der, verify_token) = parse_encryption_request(&request)?;
    let public_key = RsaPublicKey::from_public_key_der(&public_key_der)
        .map_err(|error| RuntimeError::Config(format!("invalid test public key: {error}")))?;

    std::thread::sleep(Duration::from_secs(1));
    package_single_plugin(
        "mc-plugin-auth-online-stub",
        ONLINE_STUB_AUTH_PLUGIN_ID,
        "auth",
        &dist_dir,
        &target_dir,
        "online-auth-v2",
    )?;
    let reloaded = server.reload_plugins().await?;
    assert!(
        reloaded
            .iter()
            .any(|plugin_id| plugin_id == ONLINE_STUB_AUTH_PLUGIN_ID)
    );

    let auth_after = plugin_host
        .resolve_auth_profile(ONLINE_STUB_AUTH_PROFILE_ID)
        .expect("online auth profile should still resolve");
    assert_ne!(auth_after.plugin_generation_id(), Some(before_generation));
    assert!(
        auth_after
            .capability_set()
            .contains("build-tag:online-auth-v2")
    );

    let mut shared_secret = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut shared_secret);
    let shared_secret_encrypted = public_key
        .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, &shared_secret)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt shared secret: {error}"))
        })?;
    let verify_token_encrypted = public_key
        .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, &verify_token)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt verify token: {error}"))
        })?;
    let response = login_encryption_response(&shared_secret_encrypted, &verify_token_encrypted)?;
    write_packet(&mut alpha, &codec, &response).await?;

    let mut alpha_encryption = TestClientEncryptionState::new(shared_secret);
    let login_success = read_until_packet_id_encrypted(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        0x02,
        8,
        &mut alpha_encryption,
    )
    .await?;
    assert_eq!(packet_id(&login_success), 0x02);

    let mut beta = connect_tcp(addr).await?;
    let (mut beta_encryption, mut beta_buffer) =
        perform_online_login(&mut beta, &codec, 5, "beta-online").await?;
    let beta_login_success = read_until_packet_id_encrypted(
        &mut beta,
        &codec,
        &mut beta_buffer,
        0x02,
        8,
        &mut beta_encryption,
    )
    .await?;
    assert_eq!(packet_id(&beta_login_success), 0x02);

    server.shutdown().await
}
