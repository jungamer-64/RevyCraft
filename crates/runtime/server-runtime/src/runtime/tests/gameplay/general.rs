use super::*;

#[tokio::test]
async fn creative_join_sends_inventory_selected_slot_and_abilities() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        creative_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut stream = connect_tcp(addr).await?;
    write_packet(
        &mut stream,
        &codec,
        &encode_handshake(TestJavaProtocol::Je5.protocol_version(), 2)?,
    )
    .await?;
    write_packet(&mut stream, &codec, &login_start("creative")).await?;
    let mut buffer = BytesMut::new();
    let mut window_items = None;
    let mut held_item = None;
    let mut abilities = None;
    for _ in 0..12 {
        let packet = read_packet(&mut stream, &codec, &mut buffer).await?;
        match packet_id(&packet) {
            0x30 if window_items.is_none() => window_items = Some(packet),
            0x09 if held_item.is_none() => held_item = Some(packet),
            0x39 if abilities.is_none() => abilities = Some(packet),
            _ => {}
        }
        if window_items.is_some() && held_item.is_some() && abilities.is_some() {
            break;
        }
    }
    let window_items = window_items
        .ok_or_else(|| RuntimeError::Config("window items not received".to_string()))?;
    let held_item = held_item
        .ok_or_else(|| RuntimeError::Config("held item change not received".to_string()))?;
    let abilities = abilities
        .ok_or_else(|| RuntimeError::Config("player abilities not received".to_string()))?;

    assert_eq!(
        window_items_slot(TestJavaProtocol::Je5, &window_items, 36)?,
        Some((1, 64, 0))
    );
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je5, &window_items, 44)?,
        Some((45, 64, 0))
    );
    assert_eq!(held_item_from_packet(&held_item)?, 0);
    assert_eq!(player_abilities_flags(&abilities)? & 0x0d, 0x0d);

    server.shutdown().await
}

#[tokio::test]
async fn creative_place_and_break_broadcast_block_changes() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        creative_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut first, mut first_buffer, _) = login_legacy(addr, "alpha").await?;
    let (mut second, mut second_buffer, _) = login_legacy(addr, "beta").await?;
    let _ = read_until_java_packet(
        &mut first,
        &codec,
        &mut first_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;

    write_packet(
        &mut first,
        &codec,
        &player_block_placement(2, 3, 0, 1, Some((1, 64, 0))),
    )
    .await?;
    let place_change = read_until_java_packet(
        &mut second,
        &codec,
        &mut second_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::BlockChange,
    )
    .await?;
    assert_eq!(block_change_from_packet(&place_change)?, (2, 4, 0, 1, 0));

    write_packet(&mut first, &codec, &player_digging(0, 2, 4, 0, 1)).await?;
    let break_change = read_until_java_packet(
        &mut second,
        &codec,
        &mut second_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::BlockChange,
    )
    .await?;
    assert_eq!(block_change_from_packet(&break_change)?, (2, 4, 0, 0, 0));

    server.shutdown().await
}

#[tokio::test]
async fn creative_inventory_and_selected_slot_persist_across_restart() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let codec = MinecraftWireCodec;

    let server = build_test_server(
        creative_server_config(world_dir.clone()),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);

    let (mut stream, mut buffer, _) = login_legacy(addr, "alpha").await?;
    let _ = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je5, 36, 20, 64, 0),
    )
    .await?;
    let set_slot = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::SetSlot,
    )
    .await?;
    assert_java_set_slot(TestJavaProtocol::Je5, &set_slot, 36, Some((20, 64, 0)))?;

    write_packet(&mut stream, &codec, &held_item_change(4)).await?;
    let held_slot_packet = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;
    assert_eq!(held_item_from_packet(&held_slot_packet)?, 4);

    server.shutdown().await?;

    let restarted = build_test_server(
        creative_server_config(world_dir),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&restarted);
    let (mut stream, mut buffer, window_items) = login_legacy(addr, "alpha").await?;
    let held_item = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;

    assert_eq!(
        window_items_slot(TestJavaProtocol::Je5, &window_items, 36)?,
        Some((20, 64, 0))
    );
    assert_eq!(held_item_from_packet(&held_item)?, 4);

    restarted.shutdown().await
}

#[tokio::test]
async fn plugin_backed_storage_and_auth_profiles_boot_and_persist() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let mut config = loopback_server_config(world_dir.clone());
    config.bootstrap.storage_profile = JE_1_7_10_STORAGE_PROFILE_ID.into();
    config.profiles.auth = OFFLINE_AUTH_PROFILE_ID.into();

    let server = build_test_server(config, plugin_test_registries_tcp_only()?).await?;

    let addr = listener_addr(&server);
    let _ = login_java_client_with_packet(
        addr,
        TestJavaProtocol::Je5,
        "alpha",
        TestJavaPacket::LoginSuccess,
    )
    .await?;

    server.shutdown().await?;

    assert!(world_dir.join("level.dat").exists());
    assert!(fs::read_dir(world_dir.join("playerdata"))?.next().is_some());
    Ok(())
}

#[tokio::test]
async fn unsupported_creative_inventory_action_is_corrected() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        creative_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut stream, mut buffer, _) = login_legacy(addr, "alpha").await?;
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je5, 36, 999, 64, 0),
    )
    .await?;
    let set_slot = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::SetSlot,
    )
    .await?;
    assert_java_set_slot(TestJavaProtocol::Je5, &set_slot, 36, Some((1, 64, 0)))?;

    server.shutdown().await
}

#[tokio::test]
async fn survival_place_and_break_sync_for_java_1_12_and_consume_held_slot()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = survival_server_config(temp_dir.path().join("world"));
    config.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into(), JE_340_ADAPTER_ID.into()]);
    let server = build_test_server(
        config,
        plugin_test_registries_with_allowlist(&[JE_5_ADAPTER_ID, JE_340_ADAPTER_ID])?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut actor, mut actor_buffer, _) = login_modern_1_12(addr, "alpha").await?;
    let (mut observer, mut observer_buffer, _) = login_legacy(addr, "beta").await?;
    write_packet(
        &mut actor,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let block_change = read_until_java_packet(
        &mut observer,
        &codec,
        &mut observer_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::BlockChange,
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

    assert_eq!(block_change_from_packet(&block_change)?, (2, 4, 0, 1, 0));
    assert_eq!(
        decode_set_slot(TestJavaProtocol::Je340, &set_slot)?,
        (0, 36, Some((1, 63, 0)))
    );

    write_packet(&mut actor, &codec, &player_digging_1_12(0, 2, 4, 0, 1)).await?;
    let break_change = read_until_java_packet(
        &mut observer,
        &codec,
        &mut observer_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::BlockChange,
    )
    .await?;
    assert_eq!(block_change_from_packet(&break_change)?, (2, 4, 0, 0, 0));

    server.shutdown().await
}

#[tokio::test]
async fn two_players_can_see_movement_and_restart_persists_position() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let codec = MinecraftWireCodec;

    let server = build_test_server(
        loopback_server_config(world_dir.clone()),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);

    let (mut first, mut first_buffer, _) = login_legacy_with_position(addr, "alpha").await?;
    let (mut second, _, _) = login_legacy_with_position(addr, "beta").await?;
    let spawn_packet = read_until_java_packet(
        &mut first,
        &codec,
        &mut first_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;
    assert_eq!(packet_id(&spawn_packet), 0x0c);

    write_packet(
        &mut second,
        &codec,
        &player_position_look(32.5, 4.0, 0.5, 90.0, 0.0),
    )
    .await?;
    let mut saw_teleport = false;
    for _ in 0..4 {
        let packet = read_packet(&mut first, &codec, &mut first_buffer).await?;
        if packet_id(&packet) == 0x18 {
            saw_teleport = true;
            break;
        }
    }
    assert!(saw_teleport);
    second.shutdown().await.ok();
    first.shutdown().await.ok();
    server.shutdown().await?;

    let restarted = build_test_server(
        loopback_server_config(world_dir),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&restarted);
    let (_, _, position_packet) = login_legacy_with_position(addr, "beta").await?;
    assert_eq!(packet_id(&position_packet), 0x08);
    let mut reader = PacketReader::new(&position_packet);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x08);
    let x = reader.read_f64().expect("x should decode");
    let _y = reader.read_f64().expect("y should decode");
    let _z = reader.read_f64().expect("z should decode");
    assert!(x >= 32.0);

    restarted.shutdown().await
}
