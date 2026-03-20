use super::*;

fn loopback_server_config(world_dir: PathBuf) -> ServerConfig {
    ServerConfig {
        server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
        server_port: 0,
        world_dir,
        ..ServerConfig::default()
    }
}

async fn assert_spawn_fails_with_message(
    config: ServerConfig,
    expected_fragment: &str,
) -> Result<(), RuntimeError> {
    let result = spawn_server(config, plugin_test_registries_all()?).await;
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
async fn running_server_exposes_listener_bindings() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;

    let binding = server
        .listener_bindings()
        .first()
        .expect("tcp listener binding should exist")
        .clone();
    assert_eq!(binding.transport, TransportKind::Tcp);
    assert!(binding.local_addr.port() > 0);
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_1_7_10_ADAPTER_ID)
    );

    server.shutdown().await
}

#[tokio::test]
async fn running_server_exposes_udp_listener_binding_when_enabled() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            be_enabled: true,
            ..loopback_server_config(temp_dir.path().join("world"))
        },
        plugin_test_registries_all()?,
    )
    .await?;

    assert_eq!(server.listener_bindings().len(), 2);
    let bindings = server.listener_bindings();
    let binding = bindings
        .iter()
        .find(|binding| binding.transport == TransportKind::Udp)
        .expect("udp listener binding should exist");
    assert!(binding.local_addr.port() > 0);
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == BE_26_3_ADAPTER_ID)
    );

    server.shutdown().await
}

#[tokio::test]
async fn default_bedrock_adapter_requires_listener_metadata() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    assert_spawn_fails_with_message(
        ServerConfig {
            be_enabled: true,
            default_bedrock_adapter: BE_PLACEHOLDER_ADAPTER_ID.to_string(),
            enabled_bedrock_adapters: Some(vec![BE_PLACEHOLDER_ADAPTER_ID.to_string()]),
            ..loopback_server_config(temp_dir.path().join("world"))
        },
        "must provide bedrock listener metadata",
    )
    .await
}

#[tokio::test]
async fn placeholder_bedrock_adapter_can_remain_enabled_when_not_default()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            be_enabled: true,
            default_bedrock_adapter: BE_26_3_ADAPTER_ID.to_string(),
            enabled_bedrock_adapters: Some(vec![
                BE_26_3_ADAPTER_ID.to_string(),
                BE_PLACEHOLDER_ADAPTER_ID.to_string(),
            ]),
            ..loopback_server_config(temp_dir.path().join("world"))
        },
        plugin_test_registries_all()?,
    )
    .await?;

    let bindings = server.listener_bindings();
    let binding = bindings
        .iter()
        .find(|binding| binding.transport == TransportKind::Udp)
        .expect("udp listener binding should exist");
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == BE_PLACEHOLDER_ADAPTER_ID)
    );

    server.shutdown().await
}

#[tokio::test]
async fn tcp_listener_binding_reports_enabled_java_versions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_8_X_ADAPTER_ID.to_string(),
                JE_1_12_2_ADAPTER_ID.to_string(),
            ]),
            ..loopback_server_config(temp_dir.path().join("world"))
        },
        plugin_test_registries_all()?,
    )
    .await?;

    let bindings = server.listener_bindings();
    let binding = bindings
        .iter()
        .find(|binding| binding.transport == TransportKind::Tcp)
        .expect("tcp listener binding should exist");
    assert_eq!(binding.adapter_ids.len(), 3);
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_1_7_10_ADAPTER_ID)
    );
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_1_8_X_ADAPTER_ID)
    );
    assert!(
        binding
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_1_12_2_ADAPTER_ID)
    );

    server.shutdown().await
}

#[tokio::test]
async fn status_ping_login_and_initial_world_work() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut status_stream = connect_tcp(addr).await?;
    write_packet(&mut status_stream, &codec, &encode_handshake(5, 1)?).await?;
    write_packet(&mut status_stream, &codec, &status_request()).await?;
    let mut buffer = BytesMut::new();
    let status_response = read_packet(&mut status_stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&status_response), 0x00);
    write_packet(&mut status_stream, &codec, &status_ping(42)).await?;
    let pong = read_packet(&mut status_stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&pong), 0x01);

    let mut login_stream = connect_tcp(addr).await?;
    write_packet(&mut login_stream, &codec, &encode_handshake(5, 2)?).await?;
    write_packet(&mut login_stream, &codec, &login_start("alpha")).await?;
    let mut login_buffer = BytesMut::new();
    let login_success = read_packet(&mut login_stream, &codec, &mut login_buffer).await?;
    assert_eq!(packet_id(&login_success), 0x02);
    let join_game = read_packet(&mut login_stream, &codec, &mut login_buffer).await?;
    assert_eq!(packet_id(&join_game), 0x01);
    let chunk_bulk =
        read_until_packet_id(&mut login_stream, &codec, &mut login_buffer, 0x26, 8).await?;
    assert_eq!(packet_id(&chunk_bulk), 0x26);

    server.shutdown().await
}

#[tokio::test]
async fn unsupported_status_protocol_receives_server_list_response() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(47, 1)?).await?;
    write_packet(&mut stream, &codec, &status_request()).await?;
    let mut buffer = BytesMut::new();
    let status_response = read_packet(&mut stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&status_response), 0x00);
    let mut reader = PacketReader::new(&status_response);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x00);
    let payload = reader
        .read_string(32767)
        .expect("status json should decode");
    assert!(payload.contains("\"protocol\":5"));
    assert!(payload.contains("\"name\":\"1.7.10\""));

    write_packet(&mut stream, &codec, &status_ping(99)).await?;
    let pong = read_packet(&mut stream, &codec, &mut buffer).await?;
    assert_eq!(packet_id(&pong), 0x01);

    server.shutdown().await
}

#[test]
fn udp_bedrock_probe_classifies_placeholder_datagram() -> Result<(), RuntimeError> {
    let registry = plugin_test_registries_all()?;
    let action = classify_udp_datagram(registry.protocols(), &raknet_unconnected_ping())?;
    assert_eq!(action, UdpDatagramAction::UnsupportedBedrock);
    Ok(())
}

#[test]
fn udp_unknown_datagram_is_ignored() -> Result<(), RuntimeError> {
    let registry = plugin_test_registries_all()?;
    let action = classify_udp_datagram(registry.protocols(), &[0xde, 0xad, 0xbe, 0xef])?;
    assert_eq!(action, UdpDatagramAction::Ignore);
    Ok(())
}

#[tokio::test]
async fn udp_bedrock_probe_does_not_block_je_status() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        ServerConfig {
            be_enabled: true,
            ..loopback_server_config(temp_dir.path().join("world"))
        },
        plugin_test_registries_all()?,
    )
    .await?;

    let udp_addr = udp_listener_addr(&server);
    let udp_client = UdpSocket::bind("127.0.0.1:0").await?;
    udp_client
        .send_to(&raknet_unconnected_ping(), udp_addr)
        .await?;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(5, 1)?).await?;
    write_packet(&mut stream, &codec, &status_request()).await?;
    let mut buffer = BytesMut::new();
    let status_response = read_packet(&mut stream, &codec, &mut buffer).await?;
    let mut reader = PacketReader::new(&status_response);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x00);
    let payload = reader
        .read_string(32767)
        .expect("status json should decode");
    assert!(payload.contains("\"online\":0"));

    server.shutdown().await
}
