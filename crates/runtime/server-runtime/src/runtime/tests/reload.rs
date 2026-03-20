use super::*;
use crate::runtime::RunningServer;
use mc_core::PluginGenerationId;

type EncryptedLoginChallenge = ([u8; 16], Vec<u8>, Vec<u8>);

fn write_topology_properties(
    path: &Path,
    plugins_dir: &Path,
    lines: &[&str],
) -> Result<(), RuntimeError> {
    let mut contents = format!(
        "server-ip=127.0.0.1\nserver-port=0\nlevel-name=world\nplugins-dir={}\n",
        plugins_dir.display()
    );
    for line in lines {
        contents.push_str(line);
        contents.push('\n');
    }
    fs::write(path, contents)?;
    Ok(())
}

async fn spawn_protocol_reload_server(
    temp_dir: &tempfile::TempDir,
    scenario: &str,
) -> Result<
    (
        RunningServer,
        std::path::PathBuf,
        std::path::PathBuf,
        PluginGenerationId,
    ),
    RuntimeError,
> {
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir(scenario);
    seed_runtime_plugins(&dist_dir, &[JE_1_7_10_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    package_single_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        JE_1_7_10_ADAPTER_ID,
        "protocol",
        &dist_dir,
        &target_dir,
        "protocol-reload-v1",
    )?;
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
    let adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_1_7_10_ADAPTER_ID)
        .expect("runtime should resolve the reload-test adapter");
    let before_generation = adapter
        .plugin_generation_id()
        .expect("reload-test adapter should report generation");
    assert!(
        adapter
            .capability_set()
            .contains("build-tag:protocol-reload-v1")
    );
    Ok((server, dist_dir, target_dir, before_generation))
}

async fn spawn_online_auth_reload_server(
    temp_dir: &tempfile::TempDir,
) -> Result<
    (
        RunningServer,
        std::path::PathBuf,
        std::path::PathBuf,
        PluginGenerationId,
    ),
    RuntimeError,
> {
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("auth-online-reload");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID],
        &["storage-je-anvil-1_7_10", ONLINE_STUB_AUTH_PLUGIN_ID],
    )?;
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
    Ok((server, dist_dir, target_dir, before_generation))
}

async fn begin_online_auth_handshake(
    server: &RunningServer,
) -> Result<(tokio::net::TcpStream, BytesMut, RsaPublicKey, Vec<u8>), RuntimeError> {
    let addr = listener_addr(server);
    let codec = MinecraftWireCodec;
    let mut alpha = connect_tcp(addr).await?;
    write_packet(&mut alpha, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut alpha, &codec, &login_start("alpha-online")).await?;
    let mut alpha_buffer = BytesMut::new();
    let request = read_packet(&mut alpha, &codec, &mut alpha_buffer).await?;
    let (_server_id, public_key_der, verify_token) = parse_encryption_request(&request)?;
    let public_key = RsaPublicKey::from_public_key_der(&public_key_der)
        .map_err(|error| RuntimeError::Config(format!("invalid test public key: {error}")))?;
    Ok((alpha, alpha_buffer, public_key, verify_token))
}

fn encrypt_online_login_challenge_response(
    public_key: &RsaPublicKey,
    verify_token: &[u8],
) -> Result<EncryptedLoginChallenge, RuntimeError> {
    let mut shared_secret = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut shared_secret);
    let shared_secret_encrypted = public_key
        .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, &shared_secret)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt shared secret: {error}"))
        })?;
    let verify_token_encrypted = public_key
        .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, verify_token)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt verify token: {error}"))
        })?;
    Ok((
        shared_secret,
        shared_secret_encrypted,
        verify_token_encrypted,
    ))
}

#[cfg(target_os = "linux")]
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
    package_single_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        JE_1_7_10_ADAPTER_ID,
        "protocol",
        &dist_dir,
        &target_dir,
        "protocol-reload-v2",
    )?;

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

#[cfg(target_os = "linux")]
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
    package_single_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        JE_1_7_10_ADAPTER_ID,
        "protocol",
        &dist_dir,
        &target_dir,
        "protocol-reload-fail",
    )?;

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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn gameplay_reload_updates_target_profile_generation_only() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("gameplay-reload-success");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID, JE_1_12_2_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
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
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID, JE_1_12_2_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
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
    seed_runtime_plugins(&dist_dir, &[JE_1_7_10_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
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
    seed_runtime_plugins(&dist_dir, &[JE_1_7_10_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    let config = ServerConfig {
        server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
        server_port: 0,
        plugins_dir: dist_dir.clone(),
        plugin_allowlist: Some(plugin_allowlist_with_supporting_plugins(
            &[JE_1_7_10_ADAPTER_ID],
            STORAGE_AND_AUTH_PLUGIN_IDS,
        )),
        plugin_failure_policy_storage: PluginFailureAction::Skip,
        world_dir: temp_dir.path().join("world"),
        ..ServerConfig::default()
    };
    let server = spawn_server(config.clone(), plugin_test_registries_from_config(&config)?).await?;
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
    seed_runtime_plugins(&dist_dir, &[JE_1_7_10_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
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
    seed_runtime_plugins(
        &dist_dir,
        &[
            JE_1_7_10_ADAPTER_ID,
            JE_1_8_X_ADAPTER_ID,
            JE_1_12_2_ADAPTER_ID,
        ],
        &["storage-je-anvil-1_7_10", ONLINE_STUB_AUTH_PLUGIN_ID],
    )?;
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
    let (server, dist_dir, target_dir, before_generation) =
        spawn_online_auth_reload_server(&temp_dir).await?;
    let plugin_host = server
        .plugin_host
        .as_ref()
        .expect("runtime should keep plugin host");
    let codec = MinecraftWireCodec;
    let (mut alpha, mut alpha_buffer, public_key, verify_token) =
        begin_online_auth_handshake(&server).await?;
    let addr = listener_addr(&server);

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

    let (shared_secret, shared_secret_encrypted, verify_token_encrypted) =
        encrypt_online_login_challenge_response(&public_key, &verify_token)?;
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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn topology_reload_manual_inline_updates_protocol_topology() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("topology-inline-manual");
    seed_runtime_plugins(&dist_dir, &[JE_1_7_10_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    package_single_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        JE_1_7_10_ADAPTER_ID,
        "protocol",
        &dist_dir,
        &target_dir,
        "protocol-reload-v1",
    )?;
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
    let before_topology = server.runtime.active_topology();
    let before_adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_1_7_10_ADAPTER_ID)
        .expect("runtime should resolve the initial adapter");
    let before_protocol_number = before_adapter.descriptor().protocol_number;

    std::thread::sleep(Duration::from_secs(1));
    package_single_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        JE_1_7_10_ADAPTER_ID,
        "protocol",
        &dist_dir,
        &target_dir,
        "reload-incompatible",
    )?;

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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn topology_reload_properties_source_reads_updated_server_properties()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let properties_path = temp_dir.path().join("server.properties");
    seed_runtime_plugins(
        &dist_dir,
        &[
            JE_1_7_10_ADAPTER_ID,
            JE_1_8_X_ADAPTER_ID,
            BE_26_3_ADAPTER_ID,
        ],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    write_topology_properties(
        &properties_path,
        &dist_dir,
        &[
            "default-adapter=je-1_7_10",
            "enabled-adapters=je-1_7_10,je-1_8_x",
            "be-enabled=false",
            "topology-reload-watch=false",
            "topology-drain-grace-secs=30",
        ],
    )?;
    let server = spawn_server_from_source(
        ServerConfigSource::Properties(properties_path.clone()),
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
    write_topology_properties(
        &properties_path,
        &dist_dir,
        &[
            "default-adapter=je-1_8_x",
            "enabled-adapters=je-1_7_10,je-1_8_x",
            "be-enabled=true",
            "default-bedrock-adapter=be-26_3",
            "enabled-bedrock-adapters=be-26_3",
            "topology-reload-watch=false",
            "topology-drain-grace-secs=30",
        ],
    )?;

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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn topology_reload_invalid_candidate_keeps_existing_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let properties_path = temp_dir.path().join("server.properties");
    seed_runtime_plugins(&dist_dir, &[JE_1_7_10_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    write_topology_properties(
        &properties_path,
        &dist_dir,
        &[
            "default-adapter=je-1_7_10",
            "enabled-adapters=je-1_7_10",
            "topology-reload-watch=false",
            "topology-drain-grace-secs=30",
        ],
    )?;
    let server = spawn_server_from_source(
        ServerConfigSource::Properties(properties_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let before_generation = server.runtime.active_topology().generation_id;

    std::thread::sleep(Duration::from_secs(1));
    write_topology_properties(
        &properties_path,
        &dist_dir,
        &[
            "default-adapter=missing-adapter",
            "enabled-adapters=missing-adapter",
            "topology-reload-watch=false",
            "topology-drain-grace-secs=30",
        ],
    )?;

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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn topology_reload_status_reports_draining_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let properties_path = temp_dir.path().join("server.properties");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID, JE_1_8_X_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    write_topology_properties(
        &properties_path,
        &dist_dir,
        &[
            "default-adapter=je-1_7_10",
            "enabled-adapters=je-1_7_10,je-1_8_x",
            "topology-reload-watch=false",
            "topology-drain-grace-secs=30",
        ],
    )?;
    let server = spawn_server_from_source(
        ServerConfigSource::Properties(properties_path.clone()),
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
    write_topology_properties(
        &properties_path,
        &dist_dir,
        &[
            "default-adapter=je-1_8_x",
            "enabled-adapters=je-1_7_10,je-1_8_x",
            "topology-reload-watch=false",
            "topology-drain-grace-secs=30",
        ],
    )?;
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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn topology_reload_zero_grace_disconnects_old_play_sessions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let properties_path = temp_dir.path().join("server.properties");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID, JE_1_8_X_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    write_topology_properties(
        &properties_path,
        &dist_dir,
        &[
            "default-adapter=je-1_7_10",
            "enabled-adapters=je-1_7_10,je-1_8_x",
            "topology-reload-watch=false",
            "topology-drain-grace-secs=0",
        ],
    )?;
    let server = spawn_server_from_source(
        ServerConfigSource::Properties(properties_path.clone()),
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
    write_topology_properties(
        &properties_path,
        &dist_dir,
        &[
            "default-adapter=je-1_8_x",
            "enabled-adapters=je-1_7_10,je-1_8_x",
            "topology-reload-watch=false",
            "topology-drain-grace-secs=0",
        ],
    )?;
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
