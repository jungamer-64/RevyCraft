use super::*;

#[tokio::test]
async fn online_auth_reload_keeps_existing_challenge_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let (server, dist_dir, target_dir, before_generation) =
        spawn_online_auth_reload_server(&temp_dir).await?;
    let codec = MinecraftWireCodec;
    let (mut alpha, mut alpha_buffer, public_key, verify_token) =
        begin_online_auth_handshake(&server).await?;
    let addr = listener_addr(&server);

    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .install_auth_plugin_for_reload(
            "mc-plugin-auth-online-stub",
            ONLINE_STUB_AUTH_PLUGIN_ID,
            &dist_dir,
            &target_dir,
            "online-auth-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let reloaded = server.reload_runtime_artifacts().await?.reloaded_plugin_ids;
    assert!(
        reloaded
            .iter()
            .any(|plugin_id| plugin_id == ONLINE_STUB_AUTH_PLUGIN_ID)
    );

    let auth_after = loaded_plugins_snapshot(&server)
        .await
        .resolve_auth_profile(ONLINE_STUB_AUTH_PROFILE_ID)
        .expect("online auth profile should still resolve");
    assert_ne!(auth_after.plugin_generation_id(), Some(before_generation));
    assert_eq!(
        auth_build_tag(&server, ONLINE_STUB_AUTH_PROFILE_ID).as_deref(),
        Some("online-auth-v2")
    );

    let (shared_secret, shared_secret_encrypted, verify_token_encrypted) =
        encrypt_online_login_challenge_response(&public_key, &verify_token)?;
    let response = login_encryption_response(&shared_secret_encrypted, &verify_token_encrypted)?;
    write_packet(&mut alpha, &codec, &response).await?;

    let mut alpha_encryption = TestClientEncryptionState::new(shared_secret);
    let login_success = read_until_java_packet_encrypted(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::LoginSuccess,
        8,
        &mut alpha_encryption,
    )
    .await?;
    assert_eq!(packet_id(&login_success), 0x02);

    let mut beta = connect_tcp(addr).await?;
    let (mut beta_encryption, mut beta_buffer) =
        perform_online_login(&mut beta, &codec, TestJavaProtocol::Je5, "beta-online").await?;
    let beta_login_success = read_until_java_packet_encrypted(
        &mut beta,
        &codec,
        &mut beta_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::LoginSuccess,
        8,
        &mut beta_encryption,
    )
    .await?;
    assert_eq!(packet_id(&beta_login_success), 0x02);

    server.shutdown().await
}
