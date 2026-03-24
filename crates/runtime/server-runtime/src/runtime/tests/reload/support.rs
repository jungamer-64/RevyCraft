use super::*;
use crate::runtime::RunningServer;
use rsa::rand_core::{OsRng, RngCore};

fn plugin_host_status(server: &RunningServer) -> mc_plugin_host::host::PluginHostStatusSnapshot {
    server
        .runtime
        .reload_host
        .as_ref()
        .expect("reloadable test server should retain a plugin reload host")
        .status()
}

pub(crate) fn protocol_build_tag(server: &RunningServer, plugin_id: &str) -> Option<String> {
    plugin_host_status(server)
        .protocols
        .iter()
        .find(|plugin| plugin.plugin_id == plugin_id)
        .and_then(|plugin| plugin.build_tag.as_ref())
        .map(|tag| tag.as_str().to_string())
}

pub(crate) fn gameplay_build_tag(server: &RunningServer, profile_id: &str) -> Option<String> {
    plugin_host_status(server)
        .gameplay
        .iter()
        .find(|plugin| plugin.profile_id.as_str() == profile_id)
        .and_then(|plugin| plugin.build_tag.as_ref())
        .map(|tag| tag.as_str().to_string())
}

pub(crate) fn storage_build_tag(server: &RunningServer, profile_id: &str) -> Option<String> {
    plugin_host_status(server)
        .storage
        .iter()
        .find(|plugin| plugin.profile_id.as_str() == profile_id)
        .and_then(|plugin| plugin.build_tag.as_ref())
        .map(|tag| tag.as_str().to_string())
}

pub(crate) fn auth_build_tag(server: &RunningServer, profile_id: &str) -> Option<String> {
    plugin_host_status(server)
        .auth
        .iter()
        .find(|plugin| plugin.profile_id.as_str() == profile_id)
        .and_then(|plugin| plugin.build_tag.as_ref())
        .map(|tag| tag.as_str().to_string())
}

pub(crate) async fn spawn_protocol_reload_server(
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
    let target_dir = PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .scoped_target_dir(scenario);
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
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
    config.bootstrap.plugins_dir = dist_dir.clone();
    let server = build_reloadable_test_server(
        config,
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let adapter = active_protocol_registry(&server)
        .resolve_adapter(JE_5_ADAPTER_ID)
        .expect("runtime should resolve the reload-test adapter");
    let before_generation = adapter
        .plugin_generation_id()
        .expect("reload-test adapter should report generation");
    assert_eq!(
        protocol_build_tag(&server, JE_5_ADAPTER_ID).as_deref(),
        Some("protocol-reload-v1")
    );
    Ok((server, dist_dir, target_dir, before_generation))
}

pub(crate) async fn spawn_online_auth_reload_server(
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
    let target_dir = PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .scoped_target_dir("auth-online-reload");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_5_ADAPTER_ID],
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
    config.profiles.auth = ONLINE_STUB_AUTH_PROFILE_ID.into();
    config.bootstrap.plugins_dir = dist_dir.clone();
    let server = build_reloadable_test_server(
        config,
        plugin_test_registries_from_dist_with_supporting_plugins(
            dist_dir.clone(),
            &[JE_5_ADAPTER_ID],
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
    assert_eq!(
        auth_build_tag(&server, ONLINE_STUB_AUTH_PROFILE_ID).as_deref(),
        Some("online-auth-v1")
    );
    Ok((server, dist_dir, target_dir, before_generation))
}

pub(crate) async fn begin_online_auth_handshake(
    server: &RunningServer,
) -> Result<(tokio::net::TcpStream, BytesMut, RsaPublicKey, Vec<u8>), RuntimeError> {
    let addr = listener_addr(server);
    let codec = MinecraftWireCodec;
    let mut alpha = connect_tcp(addr).await?;
    write_packet(
        &mut alpha,
        &codec,
        &encode_handshake(TestJavaProtocol::Je5.protocol_version(), 2)?,
    )
    .await?;
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
    OsRng.fill_bytes(&mut shared_secret);
    let shared_secret_encrypted = public_key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, &shared_secret)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt shared secret: {error}"))
        })?;
    let verify_token_encrypted = public_key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, verify_token)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt verify token: {error}"))
        })?;
    Ok((
        shared_secret,
        shared_secret_encrypted,
        verify_token_encrypted,
    ))
}
