use super::*;
use bedrockrs_proto::V924;

const BEDROCK_STONE_RUNTIME_ID: u32 = 2_532;
const BEDROCK_AIR_RUNTIME_ID: u32 = 12_530;

#[tokio::test]
async fn bedrock_login_receives_start_game_and_chunk_bootstrap() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = creative_server_config(temp_dir.path().join("world"));
    config.topology.be_enabled = true;
    config.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into()]);
    config.topology.default_bedrock_adapter = BE_924_ADAPTER_ID.into();
    config.topology.enabled_bedrock_adapters = Some(vec![BE_924_ADAPTER_ID.into()]);
    config.profiles.bedrock_auth = BEDROCK_OFFLINE_AUTH_PROFILE_ID.into();

    let server = build_test_server(
        config,
        plugin_test_registries_with_allowlist(&[JE_5_ADAPTER_ID, BE_924_ADAPTER_ID])?,
    )
    .await?;
    let addr = udp_listener_addr(&server);
    let mut client = BedrockTestClient::connect(addr).await?;
    client.login("builder").await?;

    let _ = read_until_bedrock_packet(&mut client, TestBedrockPacket::StartGame, 32).await?;
    let chunk = read_until_bedrock_packet(&mut client, TestBedrockPacket::LevelChunk, 64).await?;

    match chunk {
        V924::LevelChunkPacket(packet) => {
            assert!(!packet.serialized_chunk_data.is_empty());
            assert!(packet.sub_chunk_count > 0);
        }
        other => panic!("expected level chunk packet, got {other:?}"),
    }

    server.shutdown().await
}

#[tokio::test]
async fn java_block_changes_are_broadcast_to_bedrock_clients() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = creative_server_config(temp_dir.path().join("world"));
    config.topology.be_enabled = true;
    config.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into()]);
    config.topology.default_bedrock_adapter = BE_924_ADAPTER_ID.into();
    config.topology.enabled_bedrock_adapters = Some(vec![BE_924_ADAPTER_ID.into()]);
    config.profiles.bedrock_auth = BEDROCK_OFFLINE_AUTH_PROFILE_ID.into();

    let server = build_test_server(
        config,
        plugin_test_registries_with_allowlist(&[JE_5_ADAPTER_ID, BE_924_ADAPTER_ID])?,
    )
    .await?;
    let udp_addr = udp_listener_addr(&server);
    let tcp_addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut bedrock = BedrockTestClient::connect(udp_addr).await?;
    bedrock.login("builder").await?;
    let _ = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::StartGame, 32).await?;
    let _ = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::LevelChunk, 64).await?;

    let (mut java, mut _java_buffer, _) = login_legacy(tcp_addr, "alpha").await?;

    write_packet(
        &mut java,
        &codec,
        &player_block_placement(2, 3, 0, 1, Some((1, 64, 0))),
    )
    .await?;

    let place = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::UpdateBlock, 32).await?;
    match place {
        V924::UpdateBlockPacket(packet) => {
            assert_eq!(packet.block_position.x, 2);
            assert_eq!(packet.block_position.y, 4);
            assert_eq!(packet.block_position.z, 0);
            assert_eq!(packet.block_runtime_id, BEDROCK_STONE_RUNTIME_ID);
        }
        other => panic!("expected update block packet, got {other:?}"),
    }

    write_packet(&mut java, &codec, &player_digging(0, 2, 4, 0, 1)).await?;
    let break_update =
        read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::UpdateBlock, 32).await?;
    match break_update {
        V924::UpdateBlockPacket(packet) => {
            assert_eq!(packet.block_position.x, 2);
            assert_eq!(packet.block_position.y, 4);
            assert_eq!(packet.block_position.z, 0);
            assert_eq!(packet.block_runtime_id, BEDROCK_AIR_RUNTIME_ID);
        }
        other => panic!("expected update block packet, got {other:?}"),
    }

    server.shutdown().await
}

#[tokio::test]
async fn bedrock_block_changes_are_broadcast_to_mixed_java_clients() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = multi_version_creative_server_config(temp_dir.path().join("world"));
    config.topology.be_enabled = true;
    config.topology.default_bedrock_adapter = BE_924_ADAPTER_ID.into();
    config.topology.enabled_bedrock_adapters = Some(vec![BE_924_ADAPTER_ID.into()]);
    config.profiles.bedrock_auth = BEDROCK_OFFLINE_AUTH_PROFILE_ID.into();

    let server = build_test_server(
        config,
        plugin_test_registries_with_allowlist(&[
            JE_5_ADAPTER_ID,
            JE_47_ADAPTER_ID,
            JE_340_ADAPTER_ID,
            BE_924_ADAPTER_ID,
        ])?,
    )
    .await?;
    let udp_addr = udp_listener_addr(&server);
    let tcp_addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer, _) = login_legacy(tcp_addr, "legacy").await?;
    let (mut modern_18, mut modern_18_buffer, _) = login_modern_1_8(tcp_addr, "middle").await?;
    let (mut modern_112, mut modern_112_buffer, _) = login_modern_1_12(tcp_addr, "latest").await?;

    let mut bedrock = BedrockTestClient::connect(udp_addr).await?;
    bedrock.login("builder").await?;
    let _ = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::StartGame, 32).await?;
    let _ = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::LevelChunk, 64).await?;

    bedrock
        .place_block(mc_core::BlockPos::new(2, 3, 0), 1)
        .await?;

    let legacy_place = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::BlockChange,
    )
    .await?;
    let modern_18_place = read_until_java_packet(
        &mut modern_18,
        &codec,
        &mut modern_18_buffer,
        TestJavaProtocol::Je47,
        TestJavaPacket::BlockChange,
    )
    .await?;
    let modern_112_place = read_until_java_packet(
        &mut modern_112,
        &codec,
        &mut modern_112_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::BlockChange,
    )
    .await?;

    assert_eq!(block_change_from_packet(&legacy_place)?, (2, 4, 0, 1, 0));
    assert_eq!(
        block_change_from_packet_1_8(&modern_18_place)?,
        (2, 4, 0, 16)
    );
    assert_eq!(
        block_change_from_packet_1_12(&modern_112_place)?,
        (2, 4, 0, 16)
    );

    bedrock
        .break_block(mc_core::BlockPos::new(2, 4, 0), 1)
        .await?;

    let legacy_break = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::BlockChange,
    )
    .await?;
    let modern_18_break = read_until_java_packet(
        &mut modern_18,
        &codec,
        &mut modern_18_buffer,
        TestJavaProtocol::Je47,
        TestJavaPacket::BlockChange,
    )
    .await?;
    let modern_112_break = read_until_java_packet(
        &mut modern_112,
        &codec,
        &mut modern_112_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::BlockChange,
    )
    .await?;

    assert_eq!(block_change_from_packet(&legacy_break)?, (2, 4, 0, 0, 0));
    assert_eq!(
        block_change_from_packet_1_8(&modern_18_break)?,
        (2, 4, 0, 0)
    );
    assert_eq!(
        block_change_from_packet_1_12(&modern_112_break)?,
        (2, 4, 0, 0)
    );

    server.shutdown().await
}
