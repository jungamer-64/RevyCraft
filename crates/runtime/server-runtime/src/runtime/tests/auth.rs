use super::*;

fn online_auth_server_config(world_dir: PathBuf, enabled_adapters: &[&str]) -> ServerConfig {
    ServerConfig {
        server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
        server_port: 0,
        online_mode: true,
        auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
        enabled_adapters: Some(
            enabled_adapters
                .iter()
                .map(|id| (*id).to_string())
                .collect(),
        ),
        world_dir,
        ..ServerConfig::default()
    }
}

async fn assert_spawn_fails_with_message(
    config: ServerConfig,
    registries: RuntimeRegistries,
    expected_fragment: &str,
) -> Result<(), RuntimeError> {
    let result = spawn_server(config, registries).await;
    let Err(error) = result else {
        panic!("spawn_server should have failed");
    };
    assert!(
        matches!(error, RuntimeError::Config(ref message) if message.contains(expected_fragment)),
        "unexpected error: {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn online_mode_requires_online_auth_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    assert_spawn_fails_with_message(
        ServerConfig {
            online_mode: true,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        plugin_test_registries_tcp_only()?,
        "requires an online auth profile",
    )
    .await
}

#[tokio::test]
async fn offline_mode_rejects_online_auth_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    assert_spawn_fails_with_message(
        ServerConfig {
            auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        in_process_online_auth_registries(&[JE_1_7_10_ADAPTER_ID])?,
        "requires an offline auth profile",
    )
    .await
}

#[tokio::test]
async fn online_auth_supports_encrypted_login_across_java_versions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let enabled_adapters = [
        JE_1_7_10_ADAPTER_ID,
        JE_1_8_X_ADAPTER_ID,
        JE_1_12_2_ADAPTER_ID,
    ];
    let server = spawn_server(
        online_auth_server_config(temp_dir.path().join("world"), &enabled_adapters),
        in_process_online_auth_registries(&enabled_adapters)?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    for (protocol_version, username, expected_packet_id) in [
        (5, "legacy-online", 0x30),
        (47, "middle-online", 0x30),
        (340, "latest-online", 0x14),
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

#[tokio::test]
async fn encrypted_play_packets_are_processed_after_online_login() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        online_auth_server_config(temp_dir.path().join("world"), &[JE_1_7_10_ADAPTER_ID]),
        in_process_online_auth_registries(&[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut stream = connect_tcp(addr).await?;
    let (mut encryption, mut buffer) =
        perform_online_login(&mut stream, &codec, 5, "encrypted-alpha").await?;
    let _ =
        read_until_packet_id_encrypted(&mut stream, &codec, &mut buffer, 0x30, 16, &mut encryption)
            .await?;
    let _ =
        read_until_packet_id_encrypted(&mut stream, &codec, &mut buffer, 0x09, 16, &mut encryption)
            .await?;

    write_packet_encrypted(&mut stream, &codec, &held_item_change(4), &mut encryption).await?;
    let held_item =
        read_until_packet_id_encrypted(&mut stream, &codec, &mut buffer, 0x09, 8, &mut encryption)
            .await?;
    assert_eq!(held_item_from_packet(&held_item)?, 4);

    server.shutdown().await
}

#[tokio::test]
async fn verify_token_mismatch_disconnects_in_online_mode() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        online_auth_server_config(temp_dir.path().join("world"), &[JE_1_7_10_ADAPTER_ID]),
        in_process_online_auth_registries(&[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start("mismatch")).await?;
    let mut buffer = BytesMut::new();
    let request = read_packet(&mut stream, &codec, &mut buffer).await?;
    let (_server_id, public_key_der, _verify_token) = parse_encryption_request(&request)?;
    let public_key = RsaPublicKey::from_public_key_der(&public_key_der)
        .map_err(|error| RuntimeError::Config(format!("invalid test public key: {error}")))?;
    let mut shared_secret = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut shared_secret);
    let shared_secret_encrypted = public_key
        .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, &shared_secret)
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt shared secret: {error}"))
        })?;
    let verify_token_encrypted = public_key
        .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, &[9, 9, 9, 9])
        .map_err(|error| {
            RuntimeError::Config(format!("failed to encrypt verify token: {error}"))
        })?;
    let response = login_encryption_response(&shared_secret_encrypted, &verify_token_encrypted)?;
    write_packet(&mut stream, &codec, &response).await?;

    let mut encryption = TestClientEncryptionState::new(shared_secret);
    let disconnect =
        read_until_packet_id_encrypted(&mut stream, &codec, &mut buffer, 0x00, 4, &mut encryption)
            .await?;
    assert_eq!(packet_id(&disconnect), 0x00);

    server.shutdown().await
}
