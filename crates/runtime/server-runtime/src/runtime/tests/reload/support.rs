use super::*;

pub(crate) async fn spawn_protocol_reload_server(
    temp_dir: &tempfile::TempDir,
    scenario: &str,
) -> Result<
    (
        ReloadableRunningServer,
        std::path::PathBuf,
        std::path::PathBuf,
        PluginGenerationId,
    ),
    RuntimeError,
> {
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .scoped_target_dir(scenario);
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .install_protocol_plugin(
            "mc-plugin-proto-je-1_7_10-reload-test",
            JE_1_7_10_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-v1",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.game_mode = 1;
    config.bootstrap.plugins_dir = dist_dir.clone();
    let server = build_reloadable_test_server(
        config,
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

pub(crate) async fn spawn_online_auth_reload_server(
    temp_dir: &tempfile::TempDir,
) -> Result<
    (
        ReloadableRunningServer,
        std::path::PathBuf,
        std::path::PathBuf,
        PluginGenerationId,
    ),
    RuntimeError,
> {
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .scoped_target_dir("auth-online-reload");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID],
        &["storage-je-anvil-1_7_10", ONLINE_STUB_AUTH_PLUGIN_ID],
    )?;
    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .install_auth_plugin(
            "mc-plugin-auth-online-stub",
            ONLINE_STUB_AUTH_PLUGIN_ID,
            &dist_dir,
            &target_dir,
            "online-auth-v1",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.online_mode = true;
    config.profiles.auth = ONLINE_STUB_AUTH_PROFILE_ID.to_string();
    config.bootstrap.plugins_dir = dist_dir.clone();
    let server = build_reloadable_test_server(
        config,
        plugin_test_registries_from_dist_with_supporting_plugins(
            dist_dir.clone(),
            &[JE_1_7_10_ADAPTER_ID],
            &["storage-je-anvil-1_7_10", ONLINE_STUB_AUTH_PLUGIN_ID],
        )?,
    )
    .await?;
    let auth_before = loaded_plugins_snapshot(&server)
        .await
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

pub(crate) async fn begin_online_auth_handshake(
    server: &ReloadableRunningServer,
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

pub(crate) fn encrypt_online_login_challenge_response(
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
