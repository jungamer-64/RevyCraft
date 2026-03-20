use super::*;
use mc_proto_common::ConnectionPhase;

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
async fn running_server_status_exposes_topology_and_plugin_snapshot() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = spawn_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;

    let status = server.status().await;
    assert_eq!(status.active_topology.state, TopologyStatusState::Active);
    assert_eq!(
        status.active_topology.default_adapter_id,
        JE_1_7_10_ADAPTER_ID
    );
    assert!(status.active_topology.default_bedrock_adapter_id.is_none());
    assert_eq!(status.listener_bindings, server.listener_bindings());
    assert_eq!(status.session_summary.total, 0);

    let plugin_host = status
        .plugin_host
        .as_ref()
        .expect("runtime status should expose the plugin host snapshot");
    assert_eq!(plugin_host.protocols.len(), 1);
    assert_eq!(plugin_host.gameplay.len(), 1);
    assert_eq!(plugin_host.storage.len(), 1);
    assert_eq!(plugin_host.auth.len(), 1);
    assert_eq!(plugin_host.protocols[0].adapter_id, JE_1_7_10_ADAPTER_ID);
    assert_eq!(
        plugin_host.failure_matrix.protocol,
        PluginFailureMatrix::default().protocol
    );

    let summary = format_runtime_status_summary(&status);
    assert_eq!(
        summary,
        concat!(
            "runtime active-topology=1 draining-topologies=0 listeners=1 sessions=0 dirty=false\n",
            "topology tcp-default=je-1_7_10 tcp-enabled=je-1_7_10 udp-default=- udp-enabled=- max-players=20 motd=\"Multi-version Rust server\"\n",
            "session-summary transport=tcp:0,udp:0 phase=handshaking:0,status:0,login:0,play:0\n",
            "plugins protocol=1 gameplay=1 storage=1 auth=1 active-quarantines=0 artifact-quarantines=0 pending-fatal=none"
        )
    );
    let serialized = toml::to_string(&status).expect("runtime status snapshot should serialize");
    assert!(serialized.contains("active_topology"));
    assert!(serialized.contains("plugin_host"));

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
async fn running_server_session_status_reports_live_sessions() -> Result<(), RuntimeError> {
    #[derive(serde::Serialize)]
    struct SessionStatusList<'a> {
        sessions: &'a [crate::runtime::SessionStatusSnapshot],
    }

    let temp_dir = tempdir()?;
    let server = spawn_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let (_stream, _buffer) =
        connect_and_login_java_client(addr, &codec, 5, "status-observer", 0x26, 8).await?;

    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.transport, TransportKind::Tcp);
    assert_eq!(session.phase, ConnectionPhase::Play);
    assert_eq!(session.adapter_id.as_deref(), Some(JE_1_7_10_ADAPTER_ID));
    assert_eq!(session.gameplay_profile.as_deref(), Some("canonical"));
    assert!(session.player_id.is_some());
    assert!(session.entity_id.is_some());
    assert!(session.protocol_generation.is_some());
    assert!(session.gameplay_generation.is_some());

    let status = server.status().await;
    assert_eq!(status.session_summary.total, 1);
    assert!(
        status
            .session_summary
            .by_transport
            .iter()
            .any(|entry| entry.transport == TransportKind::Tcp && entry.count == 1)
    );
    assert!(
        status
            .session_summary
            .by_phase
            .iter()
            .any(|entry| entry.phase == ConnectionPhase::Play && entry.count == 1)
    );
    assert!(
        status
            .session_summary
            .by_adapter_id
            .iter()
            .any(|entry| entry.value.as_deref() == Some(JE_1_7_10_ADAPTER_ID) && entry.count == 1)
    );
    let serialized = toml::to_string(&SessionStatusList {
        sessions: &sessions,
    })
    .expect("session status snapshot list should serialize");
    assert!(serialized.contains("connection_id"));
    assert!(serialized.contains("protocol_generation"));

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
