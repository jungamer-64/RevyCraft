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

#[tokio::test]
async fn bedrock_survival_block_changes_and_inventory_decrement_are_broadcast_to_mixed_java_clients()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = multi_version_survival_server_config(temp_dir.path().join("world"));
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
    let bedrock_slot =
        read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::InventorySlot, 64).await?;

    assert_eq!(block_change_from_packet(&legacy_place)?, (2, 4, 0, 1, 0));
    assert_eq!(
        block_change_from_packet_1_8(&modern_18_place)?,
        (2, 4, 0, 16)
    );
    assert_eq!(
        block_change_from_packet_1_12(&modern_112_place)?,
        (2, 4, 0, 16)
    );
    match bedrock_slot {
        V924::InventorySlotPacket(packet) => {
            assert_eq!(packet.container_id, 0);
            assert_eq!(packet.slot, 27);
            let (item_id, count, _aux) = bedrock_stack_descriptor_summary(&packet.item)?;
            assert_ne!(item_id, 0);
            assert_eq!(count, 63);
        }
        other => panic!("expected inventory slot packet, got {other:?}"),
    }

    bedrock
        .start_break_block(mc_core::BlockPos::new(2, 2, 0), 1)
        .await?;
    let cracking =
        read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::LevelEvent, 64).await?;
    let (event_id, x, y, z, data) = bedrock_level_event_from_packet(&cracking)?;
    assert_eq!(event_id, 3600);
    assert_eq!((x, y, z), (2.0, 2.0, 0.0));
    assert!(data > 0);
    assert_no_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::BlockChange,
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(850)).await;

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

    assert_eq!(block_change_from_packet(&legacy_break)?, (2, 2, 0, 0, 0));
    assert_eq!(
        block_change_from_packet_1_8(&modern_18_break)?,
        (2, 2, 0, 0)
    );
    assert_eq!(
        block_change_from_packet_1_12(&modern_112_break)?,
        (2, 2, 0, 0)
    );

    server.shutdown().await
}

#[tokio::test]
async fn bedrock_survival_abort_cancels_block_break_and_emits_stop_cracking()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = survival_server_config(temp_dir.path().join("world"));
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

    let (mut observer, mut observer_buffer, _) = login_legacy(tcp_addr, "observer").await?;
    let mut bedrock = BedrockTestClient::connect(udp_addr).await?;
    bedrock.login("builder").await?;
    let _ = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::StartGame, 32).await?;
    let _ = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::LevelChunk, 64).await?;

    bedrock
        .start_break_block(mc_core::BlockPos::new(2, 2, 0), 1)
        .await?;
    let start = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::LevelEvent, 64).await?;
    assert_eq!(bedrock_level_event_from_packet(&start)?.0, 3600);

    bedrock
        .abort_break_block(mc_core::BlockPos::new(2, 2, 0), 1)
        .await?;
    let stop = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::LevelEvent, 64).await?;
    assert_eq!(bedrock_level_event_from_packet(&stop)?.0, 3601);

    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_no_java_packet(
        &mut observer,
        &codec,
        &mut observer_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::BlockChange,
    )
    .await?;

    server.shutdown().await
}

#[tokio::test]
async fn survival_world_drop_is_visible_to_bedrock_observers_and_despawns_after_pickup()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = survival_server_config(temp_dir.path().join("world"));
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
    bedrock.login("watcher").await?;
    let _ = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::StartGame, 32).await?;
    let _ = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::LevelChunk, 64).await?;

    let (mut java, mut java_buffer, _) = login_legacy(tcp_addr, "alpha").await?;

    write_packet(
        &mut java,
        &codec,
        &player_block_placement(1, 3, 0, 1, Some((1, 64, 0))),
    )
    .await?;
    let _ = read_until_java_packet(
        &mut java,
        &codec,
        &mut java_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::SetSlot,
    )
    .await?;

    write_packet(&mut java, &codec, &player_digging(0, 1, 4, 0, 1)).await?;
    let add_item =
        read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::AddItemActor, 64).await?;
    let remove_item =
        read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::RemoveActor, 256).await?;

    let spawned_actor_id = match add_item {
        V924::AddItemActorPacket(packet) => {
            let (item_id, count, _aux) = bedrock_stack_descriptor_summary(&packet.item)?;
            assert_ne!(item_id, 0);
            assert_eq!(count, 1);
            assert_eq!(packet.position.x, 1.5);
            assert_eq!(packet.position.y, 4.5);
            assert_eq!(packet.position.z, 0.5);
            packet.target_actor_id.0
        }
        other => panic!("expected add item actor packet, got {other:?}"),
    };

    match remove_item {
        V924::RemoveActorPacket(packet) => {
            assert_eq!(packet.target_actor_id.0, spawned_actor_id);
        }
        other => panic!("expected remove actor packet, got {other:?}"),
    }

    server.shutdown().await
}

#[tokio::test]
async fn survival_dropped_items_do_not_persist_across_restart_for_bedrock_late_joiners()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let mut config = survival_server_config(world_dir.clone());
    config.topology.be_enabled = true;
    config.topology.default_adapter = JE_340_ADAPTER_ID.into();
    config.topology.enabled_adapters = Some(vec![JE_340_ADAPTER_ID.into()]);
    config.topology.default_bedrock_adapter = BE_924_ADAPTER_ID.into();
    config.topology.enabled_bedrock_adapters = Some(vec![BE_924_ADAPTER_ID.into()]);
    config.profiles.bedrock_auth = BEDROCK_OFFLINE_AUTH_PROFILE_ID.into();

    let server = build_test_server(
        config.clone(),
        plugin_test_registries_with_allowlist(&[JE_340_ADAPTER_ID, BE_924_ADAPTER_ID])?,
    )
    .await?;
    let udp_addr = udp_listener_addr(&server);
    let tcp_addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut actor, mut actor_buffer, _) = login_modern_1_12(tcp_addr, "alpha").await?;
    let mut watcher = BedrockTestClient::connect(udp_addr).await?;
    watcher.login("watcher").await?;
    let _ = read_until_bedrock_packet(&mut watcher, TestBedrockPacket::StartGame, 32).await?;
    let _ = read_until_bedrock_packet(&mut watcher, TestBedrockPacket::LevelChunk, 64).await?;

    write_packet(
        &mut actor,
        &codec,
        &player_block_placement_1_12(4, 3, 0, 1, 0),
    )
    .await?;
    let set_slot = read_until_java_packet(
        &mut actor,
        &codec,
        &mut actor_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::SetSlot,
    )
    .await?;
    assert_eq!(
        decode_set_slot(TestJavaProtocol::Je340, &set_slot)?,
        (0, 36, Some((1, 63, 0)))
    );

    write_packet(&mut actor, &codec, &player_digging_1_12(0, 4, 4, 0, 1)).await?;
    let add_item =
        read_until_bedrock_packet(&mut watcher, TestBedrockPacket::AddItemActor, 64).await?;
    match add_item {
        V924::AddItemActorPacket(packet) => {
            let (item_id, count, _aux) = bedrock_stack_descriptor_summary(&packet.item)?;
            assert_ne!(item_id, 0);
            assert_eq!(count, 1);
            assert_eq!(packet.position.x, 4.5);
            assert_eq!(packet.position.y, 4.5);
            assert_eq!(packet.position.z, 0.5);
        }
        other => panic!("expected add item actor packet, got {other:?}"),
    }

    watcher.disconnect().await?;
    drop(watcher);
    actor.shutdown().await?;
    drop(actor);

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let udp_sessions = server
                .session_status()
                .await
                .into_iter()
                .filter(|session| session.transport == TransportKind::Udp)
                .count();
            if udp_sessions == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| RuntimeError::Config("bedrock session did not drain before shutdown".into()))?;

    tokio::time::timeout(Duration::from_secs(1), server.shutdown())
        .await
        .map_err(|_| RuntimeError::Config("shutdown timed out before restart".into()))??;

    let restarted = build_test_server(
        config,
        plugin_test_registries_with_allowlist(&[JE_340_ADAPTER_ID, BE_924_ADAPTER_ID])?,
    )
    .await?;
    let udp_addr = udp_listener_addr(&restarted);
    let mut late_joiner = BedrockTestClient::connect(udp_addr).await?;
    late_joiner.login("late").await?;
    let _ = read_until_bedrock_packet(&mut late_joiner, TestBedrockPacket::StartGame, 32).await?;
    let _ = read_until_bedrock_packet(&mut late_joiner, TestBedrockPacket::LevelChunk, 64).await?;

    assert_no_bedrock_packet(&mut late_joiner, TestBedrockPacket::AddItemActor).await?;
    assert_no_bedrock_packet(&mut late_joiner, TestBedrockPacket::RemoveActor).await?;

    restarted.shutdown().await
}
