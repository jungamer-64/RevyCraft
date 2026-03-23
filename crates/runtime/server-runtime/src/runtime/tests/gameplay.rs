use super::*;

fn creative_server_config(world_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.game_mode = 1;
    config
}

fn multi_version_creative_server_config(world_dir: PathBuf) -> ServerConfig {
    let mut config = creative_server_config(world_dir);
    config.topology.enabled_adapters = Some(vec![
        JE_1_7_10_ADAPTER_ID.to_string(),
        JE_1_8_X_ADAPTER_ID.to_string(),
        JE_1_12_2_ADAPTER_ID.to_string(),
    ]);
    config
}

async fn login_legacy(
    addr: SocketAddr,
    username: &str,
) -> Result<(tokio::net::TcpStream, BytesMut, Vec<u8>), RuntimeError> {
    login_java_client_with_packet(addr, 5, username, 0x30, 12).await
}

async fn login_legacy_with_position(
    addr: SocketAddr,
    username: &str,
) -> Result<(tokio::net::TcpStream, BytesMut, Vec<u8>), RuntimeError> {
    login_java_client_with_packet(addr, 5, username, 0x08, 8).await
}

async fn login_modern_1_8(
    addr: SocketAddr,
    username: &str,
) -> Result<(tokio::net::TcpStream, BytesMut, Vec<u8>), RuntimeError> {
    login_java_client_with_packet(addr, 47, username, 0x30, 24).await
}

async fn login_modern_1_12(
    addr: SocketAddr,
    username: &str,
) -> Result<(tokio::net::TcpStream, BytesMut, Vec<u8>), RuntimeError> {
    login_java_client_with_packet(addr, 340, username, 0x14, 24).await
}

async fn login_java_client_with_packet(
    addr: SocketAddr,
    protocol_version: i32,
    username: &str,
    expected_packet_id: i32,
    max_packets: usize,
) -> Result<(tokio::net::TcpStream, BytesMut, Vec<u8>), RuntimeError> {
    let codec = MinecraftWireCodec;
    let mut stream = connect_tcp(addr).await?;
    write_packet(&mut stream, &codec, &encode_handshake(protocol_version, 2)?).await?;
    write_packet(&mut stream, &codec, &login_start(username)).await?;
    let mut buffer = BytesMut::new();
    let packet = read_until_packet_id(
        &mut stream,
        &codec,
        &mut buffer,
        expected_packet_id,
        max_packets,
    )
    .await?;
    Ok((stream, buffer, packet))
}

async fn craft_log_into_planks_legacy(
    stream: &mut tokio::net::TcpStream,
    buffer: &mut BytesMut,
    codec: &MinecraftWireCodec,
) -> Result<(), RuntimeError> {
    write_packet(stream, codec, &creative_inventory_action(36, 17, 1, 0)).await?;
    let hotbar_update = read_until_set_slot(stream, codec, buffer, 0x2f, 0, 36, 16).await?;
    assert_eq!(
        decode_set_slot(&hotbar_update, 0x2f)?,
        (0, 36, Some((17, 1, 0)))
    );

    write_packet(stream, codec, &click_window(36, 0)).await?;
    let hotbar_pickup = read_until_set_slot(stream, codec, buffer, 0x2f, 0, 36, 16).await?;
    assert_eq!(decode_set_slot(&hotbar_pickup, 0x2f)?, (0, 36, None));
    let cursor_pickup = read_until_set_slot(stream, codec, buffer, 0x2f, -1, -1, 16).await?;
    assert_eq!(
        decode_set_slot(&cursor_pickup, 0x2f)?,
        (-1, -1, Some((17, 1, 0)))
    );

    write_packet(stream, codec, &click_window(1, 0)).await?;
    let result_preview = read_until_set_slot(stream, codec, buffer, 0x2f, 0, 0, 16).await?;
    assert_eq!(
        decode_set_slot(&result_preview, 0x2f)?,
        (0, 0, Some((5, 4, 0)))
    );
    let craft_input = read_until_set_slot(stream, codec, buffer, 0x2f, 0, 1, 16).await?;
    assert_eq!(
        decode_set_slot(&craft_input, 0x2f)?,
        (0, 1, Some((17, 1, 0)))
    );
    let cursor_cleared = read_until_set_slot(stream, codec, buffer, 0x2f, -1, -1, 16).await?;
    assert_eq!(decode_set_slot(&cursor_cleared, 0x2f)?, (-1, -1, None));

    write_packet(stream, codec, &click_window(0, 0)).await?;
    let result_taken = read_until_set_slot(stream, codec, buffer, 0x2f, 0, 0, 16).await?;
    assert_eq!(decode_set_slot(&result_taken, 0x2f)?, (0, 0, None));
    let input_consumed = read_until_set_slot(stream, codec, buffer, 0x2f, 0, 1, 16).await?;
    assert_eq!(decode_set_slot(&input_consumed, 0x2f)?, (0, 1, None));
    let cursor_result = read_until_set_slot(stream, codec, buffer, 0x2f, -1, -1, 16).await?;
    assert_eq!(
        decode_set_slot(&cursor_result, 0x2f)?,
        (-1, -1, Some((5, 4, 0)))
    );
    Ok(())
}

async fn craft_log_into_planks_1_12(
    stream: &mut tokio::net::TcpStream,
    buffer: &mut BytesMut,
    codec: &MinecraftWireCodec,
) -> Result<(), RuntimeError> {
    write_packet(stream, codec, &creative_inventory_action_1_12(36, 17, 1, 0)).await?;
    let hotbar_update = read_until_set_slot(stream, codec, buffer, 0x16, 0, 36, 16).await?;
    assert_eq!(
        decode_set_slot(&hotbar_update, 0x16)?,
        (0, 36, Some((17, 1, 0)))
    );

    write_packet(stream, codec, &click_window_1_12(36, 0)).await?;
    let hotbar_pickup = read_until_set_slot(stream, codec, buffer, 0x16, 0, 36, 16).await?;
    assert_eq!(decode_set_slot(&hotbar_pickup, 0x16)?, (0, 36, None));
    let cursor_pickup = read_until_set_slot(stream, codec, buffer, 0x16, -1, -1, 16).await?;
    assert_eq!(
        decode_set_slot(&cursor_pickup, 0x16)?,
        (-1, -1, Some((17, 1, 0)))
    );

    write_packet(stream, codec, &click_window_1_12(1, 0)).await?;
    let result_preview = read_until_set_slot(stream, codec, buffer, 0x16, 0, 0, 16).await?;
    assert_eq!(
        decode_set_slot(&result_preview, 0x16)?,
        (0, 0, Some((5, 4, 0)))
    );
    let craft_input = read_until_set_slot(stream, codec, buffer, 0x16, 0, 1, 16).await?;
    assert_eq!(
        decode_set_slot(&craft_input, 0x16)?,
        (0, 1, Some((17, 1, 0)))
    );
    let cursor_cleared = read_until_set_slot(stream, codec, buffer, 0x16, -1, -1, 16).await?;
    assert_eq!(decode_set_slot(&cursor_cleared, 0x16)?, (-1, -1, None));

    write_packet(stream, codec, &click_window_1_12(0, 0)).await?;
    let result_taken = read_until_set_slot(stream, codec, buffer, 0x16, 0, 0, 16).await?;
    assert_eq!(decode_set_slot(&result_taken, 0x16)?, (0, 0, None));
    let input_consumed = read_until_set_slot(stream, codec, buffer, 0x16, 0, 1, 16).await?;
    assert_eq!(decode_set_slot(&input_consumed, 0x16)?, (0, 1, None));
    let cursor_result = read_until_set_slot(stream, codec, buffer, 0x16, -1, -1, 16).await?;
    assert_eq!(
        decode_set_slot(&cursor_result, 0x16)?,
        (-1, -1, Some((5, 4, 0)))
    );
    Ok(())
}

fn assert_legacy_set_slot(
    packet: &[u8],
    expected_slot: i16,
    expected_item: Option<(i16, u8, i16)>,
) -> Result<(), RuntimeError> {
    let mut reader = PacketReader::new(packet);
    assert_eq!(reader.read_varint()?, 0x2f);
    assert_eq!(reader.read_i8()?, 0);
    assert_eq!(reader.read_i16()?, expected_slot);
    assert_eq!(read_slot(&mut reader)?, expected_item);
    Ok(())
}

#[tokio::test]
async fn modern_offhand_persists_without_leaking_legacy_slots() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let codec = MinecraftWireCodec;

    let server = build_test_server(
        multi_version_creative_server_config(world_dir.clone()),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);

    let (mut modern, mut modern_buffer, _) = login_modern_1_12(addr, "alpha").await?;
    let set_slot = {
        write_packet(
            &mut modern,
            &codec,
            &creative_inventory_action_1_12(45, 20, 64, 0),
        )
        .await?;
        read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x16, 8).await?
    };
    assert_eq!(set_slot_slot(&set_slot, 0x16)?, 45);

    server.shutdown().await?;

    let restarted = build_test_server(
        multi_version_creative_server_config(world_dir),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&restarted);

    let (_, _, window_items) = login_modern_1_12(addr, "alpha").await?;
    assert_eq!(
        window_items_slot_with_packet_id(&window_items, 0x14, 45)?,
        Some((20, 64, 0))
    );

    let (_, _, legacy_window_items) = login_modern_1_8(addr, "beta").await?;
    assert!(window_items_slot(&legacy_window_items, 45).is_err());

    restarted.shutdown().await
}

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
    write_packet(&mut stream, &codec, &encode_handshake(5, 2)?).await?;
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

    assert_eq!(window_items_slot(&window_items, 36)?, Some((1, 64, 0)));
    assert_eq!(window_items_slot(&window_items, 44)?, Some((45, 64, 0)));
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
    let _ = read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x0c, 12).await?;

    write_packet(
        &mut first,
        &codec,
        &player_block_placement(2, 3, 0, 1, Some((1, 64, 0))),
    )
    .await?;
    let place_change =
        read_until_packet_id(&mut second, &codec, &mut second_buffer, 0x23, 8).await?;
    assert_eq!(block_change_from_packet(&place_change)?, (2, 4, 0, 1, 0));

    write_packet(&mut first, &codec, &player_digging(0, 2, 4, 0, 1)).await?;
    let break_change =
        read_until_packet_id(&mut second, &codec, &mut second_buffer, 0x23, 8).await?;
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
    let _ = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x09, 12).await?;

    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(36, 20, 64, 0),
    )
    .await?;
    let set_slot = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x2f, 8).await?;
    assert_legacy_set_slot(&set_slot, 36, Some((20, 64, 0)))?;

    write_packet(&mut stream, &codec, &held_item_change(4)).await?;
    let held_slot_packet = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x09, 8).await?;
    assert_eq!(held_item_from_packet(&held_slot_packet)?, 4);

    server.shutdown().await?;

    let restarted = build_test_server(
        creative_server_config(world_dir),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&restarted);
    let (mut stream, mut buffer, window_items) = login_legacy(addr, "alpha").await?;
    let held_item = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x09, 12).await?;

    assert_eq!(window_items_slot(&window_items, 36)?, Some((20, 64, 0)));
    assert_eq!(held_item_from_packet(&held_item)?, 4);

    restarted.shutdown().await
}

#[tokio::test]
async fn plugin_backed_storage_and_auth_profiles_boot_and_persist() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let mut config = loopback_server_config(world_dir.clone());
    config.bootstrap.storage_profile = JE_1_7_10_STORAGE_PROFILE_ID.to_string();
    config.profiles.auth = OFFLINE_AUTH_PROFILE_ID.to_string();

    let server = build_test_server(config, plugin_test_registries_tcp_only()?).await?;

    let addr = listener_addr(&server);
    let _ = login_java_client_with_packet(addr, 5, "alpha", 0x02, 8).await?;

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
        &creative_inventory_action(36, 999, 64, 0),
    )
    .await?;
    let set_slot = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x2f, 8).await?;
    assert_legacy_set_slot(&set_slot, 36, Some((1, 64, 0)))?;

    server.shutdown().await
}

#[tokio::test]
async fn legacy_window_zero_crafting_round_trips_authoritative_slot_updates()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        creative_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut stream, mut buffer, _) = login_legacy(addr, "craft-legacy").await?;
    craft_log_into_planks_legacy(&mut stream, &mut buffer, &codec).await?;

    server.shutdown().await
}

#[tokio::test]
async fn modern_1_8_window_zero_crafting_round_trips_authoritative_slot_updates()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        multi_version_creative_server_config(temp_dir.path().join("world")),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut stream, mut buffer, _) = login_modern_1_8(addr, "craft-18").await?;
    craft_log_into_planks_legacy(&mut stream, &mut buffer, &codec).await?;

    server.shutdown().await
}

#[tokio::test]
async fn modern_1_12_window_zero_crafting_round_trips_authoritative_slot_updates()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        multi_version_creative_server_config(temp_dir.path().join("world")),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut stream, mut buffer, _) = login_modern_1_12(addr, "craft-1122").await?;
    craft_log_into_planks_1_12(&mut stream, &mut buffer, &codec).await?;

    server.shutdown().await
}

#[tokio::test]
async fn survival_place_is_rejected_with_block_and_inventory_correction() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let server = build_test_server(
        loopback_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut stream, mut buffer, _) = login_legacy(addr, "alpha").await?;
    write_packet(
        &mut stream,
        &codec,
        &player_block_placement(2, 3, 0, 1, Some((1, 64, 0))),
    )
    .await?;
    let block_change = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x23, 8).await?;
    let set_slot = read_until_packet_id(&mut stream, &codec, &mut buffer, 0x2f, 8).await?;

    assert_eq!(block_change_from_packet(&block_change)?, (2, 4, 0, 0, 0));
    assert_legacy_set_slot(&set_slot, 36, Some((1, 64, 0)))?;

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
    let spawn_packet = read_until_packet_id(&mut first, &codec, &mut first_buffer, 0x0c, 8).await?;
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
