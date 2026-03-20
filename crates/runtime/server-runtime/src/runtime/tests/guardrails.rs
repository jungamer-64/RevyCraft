use super::*;

async fn assert_spawn_fails_with_message(
    config: ServerConfig,
    expected_fragment: &str,
) -> Result<(), RuntimeError> {
    let result = spawn_server(config, plugin_test_registries_tcp_only()?).await;
    let Err(error) = result else {
        panic!("spawn_server should have failed");
    };
    assert!(
        matches!(error, RuntimeError::Config(ref message) if message.contains(expected_fragment)),
        "unexpected error: {error:?}"
    );
    Ok(())
}

fn loopback_server_config(world_dir: PathBuf) -> ServerConfig {
    ServerConfig {
        server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
        server_port: 0,
        world_dir,
        ..ServerConfig::default()
    }
}

#[tokio::test]
async fn unknown_default_adapter_fails_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    assert_spawn_fails_with_message(
        ServerConfig {
            default_adapter: "missing".to_string(),
            ..loopback_server_config(temp_dir.path().join("world"))
        },
        "unknown default-adapter",
    )
    .await
}

#[tokio::test]
async fn unknown_gameplay_profile_fails_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    assert_spawn_fails_with_message(
        ServerConfig {
            default_gameplay_profile: "missing".to_string(),
            ..loopback_server_config(temp_dir.path().join("world"))
        },
        "unknown gameplay profile",
    )
    .await
}

#[tokio::test]
async fn unknown_storage_profile_fails_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    assert_spawn_fails_with_message(
        ServerConfig {
            storage_profile: "missing".to_string(),
            ..loopback_server_config(temp_dir.path().join("world"))
        },
        "unknown storage profile",
    )
    .await
}

#[tokio::test]
async fn unknown_auth_profile_fails_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    assert_spawn_fails_with_message(
        ServerConfig {
            auth_profile: "missing".to_string(),
            ..loopback_server_config(temp_dir.path().join("world"))
        },
        "unknown auth profile",
    )
    .await
}

#[tokio::test]
async fn unmatched_probe_closes_without_response() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &[0x01]).await?;

    let mut bytes = [0_u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut bytes))
        .await
        .map_err(|_| RuntimeError::Config("probe mismatch did not close".to_string()))??;
    assert_eq!(read, 0);

    server.shutdown().await
}

#[tokio::test]
async fn unsupported_login_protocol_receives_disconnect() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(47, 2)?).await?;
    let mut buffer = BytesMut::new();
    let disconnect = read_packet(&mut stream, &codec, &mut buffer).await?;
    let mut reader = PacketReader::new(&disconnect);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x00);
    let reason = reader
        .read_string(32767)
        .expect("disconnect reason should decode");
    assert!(reason.contains("Unsupported protocol 47"));
    assert!(reason.contains("1.7.10"));

    server.shutdown().await
}
