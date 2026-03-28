use super::*;

fn multi_version_survival_server_config(world_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.game_mode = 0;
    config.topology.enabled_adapters = Some(vec![
        JE_5_ADAPTER_ID.into(),
        JE_47_ADAPTER_ID.into(),
        JE_340_ADAPTER_ID.into(),
        JE_404_ADAPTER_ID.into(),
    ]);
    config
}

#[tokio::test]
async fn mixed_java_versions_share_login_movement_and_block_sync() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.game_mode = 1;
    config.topology.enabled_adapters = Some(vec![
        JE_5_ADAPTER_ID.into(),
        JE_47_ADAPTER_ID.into(),
        JE_340_ADAPTER_ID.into(),
        JE_404_ADAPTER_ID.into(),
    ]);
    let server = build_test_server(config, plugin_test_registries_all()?).await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "legacy").await?;
    let (mut modern_18, mut modern_18_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je47, "middle").await?;
    let modern_18_player_info = read_until_java_packet(
        &mut modern_18,
        &codec,
        &mut modern_18_buffer,
        TestJavaProtocol::Je47,
        TestJavaPacket::PlayerInfoAdd,
    )
    .await?;
    assert_eq!(packet_id(&modern_18_player_info), 0x38);
    let modern_18_spawn = read_until_java_packet(
        &mut modern_18,
        &codec,
        &mut modern_18_buffer,
        TestJavaProtocol::Je47,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;
    assert_eq!(packet_id(&modern_18_spawn), 0x0c);
    let _ = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;

    let (mut modern_112, mut modern_112_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je340, "latest").await?;
    let modern_112_player_info = read_until_java_packet(
        &mut modern_112,
        &codec,
        &mut modern_112_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::PlayerInfoAdd,
    )
    .await?;
    assert_eq!(packet_id(&modern_112_player_info), 0x2d);
    let modern_112_spawn = read_until_java_packet(
        &mut modern_112,
        &codec,
        &mut modern_112_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;
    assert_eq!(packet_id(&modern_112_spawn), 0x05);
    let _ = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;
    let modern_18_player_info = read_until_java_packet(
        &mut modern_18,
        &codec,
        &mut modern_18_buffer,
        TestJavaProtocol::Je47,
        TestJavaPacket::PlayerInfoAdd,
    )
    .await?;
    assert_eq!(packet_id(&modern_18_player_info), 0x38);
    let modern_18_spawn = read_until_java_packet(
        &mut modern_18,
        &codec,
        &mut modern_18_buffer,
        TestJavaProtocol::Je47,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;
    assert_eq!(packet_id(&modern_18_spawn), 0x0c);

    let (mut modern_113, mut modern_113_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je404, "flattened").await?;
    let modern_113_player_info = read_until_java_packet(
        &mut modern_113,
        &codec,
        &mut modern_113_buffer,
        TestJavaProtocol::Je404,
        TestJavaPacket::PlayerInfoAdd,
    )
    .await?;
    assert_eq!(packet_id(&modern_113_player_info), 0x30);
    let modern_113_spawn = read_until_java_packet(
        &mut modern_113,
        &codec,
        &mut modern_113_buffer,
        TestJavaProtocol::Je404,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;
    assert_eq!(packet_id(&modern_113_spawn), 0x05);

    write_packet(
        &mut modern_18,
        &codec,
        &player_position_look_1_8(32.5, 4.0, 0.5, 90.0, 0.0),
    )
    .await?;
    let legacy_teleport = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::EntityTeleport,
    )
    .await?;
    let modern_112_teleport = read_until_java_packet(
        &mut modern_112,
        &codec,
        &mut modern_112_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::EntityTeleport,
    )
    .await?;
    let modern_113_teleport = read_until_java_packet(
        &mut modern_113,
        &codec,
        &mut modern_113_buffer,
        TestJavaProtocol::Je404,
        TestJavaPacket::EntityTeleport,
    )
    .await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);
    assert_eq!(packet_id(&modern_112_teleport), 0x4c);
    assert_eq!(packet_id(&modern_113_teleport), 0x50);

    write_packet(
        &mut modern_112,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let legacy_block_change = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::BlockChange,
    )
    .await?;
    let modern_18_block_change = read_until_java_packet(
        &mut modern_18,
        &codec,
        &mut modern_18_buffer,
        TestJavaProtocol::Je47,
        TestJavaPacket::BlockChange,
    )
    .await?;
    let modern_113_block_change = read_until_java_packet(
        &mut modern_113,
        &codec,
        &mut modern_113_buffer,
        TestJavaProtocol::Je404,
        TestJavaPacket::BlockChange,
    )
    .await?;
    assert_eq!(
        block_change_from_packet(&legacy_block_change)?,
        (2, 4, 0, 1, 0)
    );
    assert_eq!(
        block_change_from_packet_1_8(&modern_18_block_change)?,
        (2, 4, 0, 16)
    );
    assert_eq!(
        block_change_from_packet_1_12(&modern_113_block_change)?,
        (2, 4, 0, 1)
    );

    server.shutdown().await
}

#[tokio::test]
async fn mixed_java_versions_share_survival_block_sync() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let config = multi_version_survival_server_config(temp_dir.path().join("world"));
    let server = build_test_server(config, plugin_test_registries_all()?).await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "legacy").await?;
    let (mut modern_18, mut modern_18_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je47, "middle").await?;
    let (mut modern_112, mut modern_112_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je340, "latest").await?;
    let (mut modern_113, mut modern_113_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je404, "flattened").await?;

    write_packet(
        &mut modern_112,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
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
    let modern_112_slot = read_until_set_slot(
        &mut modern_112,
        &codec,
        &mut modern_112_buffer,
        TestJavaProtocol::Je340,
        0,
        36,
        16,
    )
    .await?;
    let modern_113_place = read_until_java_packet(
        &mut modern_113,
        &codec,
        &mut modern_113_buffer,
        TestJavaProtocol::Je404,
        TestJavaPacket::BlockChange,
    )
    .await?;

    assert_eq!(block_change_from_packet(&legacy_place)?, (2, 4, 0, 1, 0));
    assert_eq!(
        block_change_from_packet_1_8(&modern_18_place)?,
        (2, 4, 0, 16)
    );
    assert_eq!(
        decode_set_slot(TestJavaProtocol::Je340, &modern_112_slot)?,
        (0, 36, Some((1, 63, 0)))
    );
    assert_eq!(
        block_change_from_packet_1_12(&modern_113_place)?,
        (2, 4, 0, 1)
    );

    write_packet(&mut modern_112, &codec, &player_digging_1_12(0, 2, 2, 0, 1)).await?;
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
    let modern_113_break = read_until_java_packet(
        &mut modern_113,
        &codec,
        &mut modern_113_buffer,
        TestJavaProtocol::Je404,
        TestJavaPacket::BlockChange,
    )
    .await?;

    assert_eq!(block_change_from_packet(&legacy_break)?, (2, 2, 0, 0, 0));
    assert_eq!(
        block_change_from_packet_1_8(&modern_18_break)?,
        (2, 2, 0, 0)
    );
    assert_eq!(
        block_change_from_packet_1_12(&modern_113_break)?,
        (2, 2, 0, 0)
    );

    server.shutdown().await
}

#[tokio::test]
async fn adapter_mapped_gameplay_profiles_can_run_concurrently() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.game_mode = 1;
    config.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into(), JE_340_ADAPTER_ID.into()]);
    config.profiles.default_gameplay = "canonical".into();
    config.profiles.gameplay_map = gameplay_profile_map(&[
        (JE_5_ADAPTER_ID, "readonly"),
        (JE_340_ADAPTER_ID, "canonical"),
    ]);
    let server = build_test_server(config, plugin_test_registries_all()?).await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "legacy-readonly")
            .await?;
    let (mut modern, mut modern_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je340, "modern-canonical")
            .await?;
    let _ = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;

    write_packet(
        &mut modern,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let _ = read_until_java_packet(
        &mut modern,
        &codec,
        &mut modern_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::BlockChange,
    )
    .await?;
    let legacy_block_change = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::BlockChange,
    )
    .await?;
    assert_eq!(
        block_change_from_packet(&legacy_block_change)?,
        (2, 4, 0, 1, 0)
    );

    write_packet(
        &mut legacy,
        &codec,
        &player_block_placement(3, 3, 0, 1, Some((1, 64, 0))),
    )
    .await?;
    assert_no_java_packet(
        &mut modern,
        &codec,
        &mut modern_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::BlockChange,
    )
    .await?;

    write_packet(
        &mut legacy,
        &codec,
        &player_position_look(12.5, 4.0, 0.5, 0.0, 0.0),
    )
    .await?;
    let modern_teleport = read_until_java_packet(
        &mut modern,
        &codec,
        &mut modern_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::EntityTeleport,
    )
    .await?;
    assert_eq!(packet_id(&modern_teleport), 0x4c);

    server.shutdown().await
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn packaged_plugins_support_mixed_versions_and_bedrock_probe() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let registries = plugin_test_registries_all()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.topology.be_enabled = true;
    config.bootstrap.game_mode = 1;
    config.topology.enabled_adapters = Some(vec![
        JE_5_ADAPTER_ID.into(),
        JE_47_ADAPTER_ID.into(),
        JE_340_ADAPTER_ID.into(),
        BE_PLACEHOLDER_ADAPTER_ID.into(),
    ]);
    let server = build_test_server(config, registries).await?;

    let udp_addr = udp_listener_addr(&server);
    let udp_client = UdpSocket::bind("127.0.0.1:0").await?;
    udp_client
        .send_to(&raknet_unconnected_ping(), udp_addr)
        .await?;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let mut status_stream = connect_tcp(addr).await?;
    write_packet(
        &mut status_stream,
        &codec,
        &encode_handshake(TestJavaProtocol::Je5.protocol_version(), 1)?,
    )
    .await?;
    write_packet(&mut status_stream, &codec, &[0x00]).await?;
    let mut status_buffer = BytesMut::new();
    let status = read_packet(&mut status_stream, &codec, &mut status_buffer).await?;
    assert_eq!(packet_id(&status), 0x00);

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "legacy").await?;
    let (mut modern_18, mut modern_18_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je47, "middle").await?;
    let _ = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;

    let (mut modern_112, mut modern_112_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je340, "latest").await?;
    let _ = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;
    let _ = read_until_java_packet(
        &mut modern_18,
        &codec,
        &mut modern_18_buffer,
        TestJavaProtocol::Je47,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;

    write_packet(
        &mut modern_18,
        &codec,
        &player_position_look_1_8(32.5, 4.0, 0.5, 90.0, 0.0),
    )
    .await?;
    let legacy_teleport = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::EntityTeleport,
    )
    .await?;
    let modern_112_teleport = read_until_java_packet(
        &mut modern_112,
        &codec,
        &mut modern_112_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::EntityTeleport,
    )
    .await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);
    assert_eq!(packet_id(&modern_112_teleport), 0x4c);

    write_packet(
        &mut modern_112,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let legacy_block_change = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::BlockChange,
    )
    .await?;
    let modern_18_block_change = read_until_java_packet(
        &mut modern_18,
        &codec,
        &mut modern_18_buffer,
        TestJavaProtocol::Je47,
        TestJavaPacket::BlockChange,
    )
    .await?;
    assert_eq!(
        block_change_from_packet(&legacy_block_change)?,
        (2, 4, 0, 1, 0)
    );
    assert_eq!(
        block_change_from_packet_1_8(&modern_18_block_change)?,
        (2, 4, 0, 16)
    );

    server.shutdown().await
}

#[tokio::test]
async fn mixed_java_versions_keep_window_zero_crafting_isolated() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.game_mode = 1;
    config.topology.enabled_adapters = Some(vec![
        JE_5_ADAPTER_ID.into(),
        JE_47_ADAPTER_ID.into(),
        JE_340_ADAPTER_ID.into(),
    ]);
    let server = build_test_server(config, plugin_test_registries_all()?).await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "legacy-craft").await?;
    let (mut modern, mut modern_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je340, "modern-craft")
            .await?;
    let _ = read_until_java_packet(
        &mut legacy,
        &codec,
        &mut legacy_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::NamedEntitySpawn,
    )
    .await?;

    write_packet(
        &mut modern,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 36, 17, 1, 0),
    )
    .await?;
    let _ = read_until_set_slot(
        &mut modern,
        &codec,
        &mut modern_buffer,
        TestJavaProtocol::Je340,
        0,
        36,
        16,
    )
    .await?;
    write_packet(
        &mut modern,
        &codec,
        &click_window(TestJavaProtocol::Je340, 36, 0, 1, Some((17, 1, 0))),
    )
    .await?;
    let reject_ack = read_until_confirm_transaction(
        &mut modern,
        &codec,
        &mut modern_buffer,
        TestJavaProtocol::Je340,
        0,
        1,
        16,
    )
    .await?;
    assert_eq!(
        decode_confirm_transaction(TestJavaProtocol::Je340, &reject_ack)?,
        (0, 1, false)
    );
    let modern_resync = read_until_java_packet(
        &mut modern,
        &codec,
        &mut modern_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::WindowItems,
    )
    .await?;
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je340, &modern_resync, 36)?,
        None
    );
    let _ = read_until_set_slot(
        &mut modern,
        &codec,
        &mut modern_buffer,
        TestJavaProtocol::Je340,
        0,
        36,
        16,
    )
    .await?;
    let _ = read_until_set_slot(
        &mut modern,
        &codec,
        &mut modern_buffer,
        TestJavaProtocol::Je340,
        -1,
        -1,
        16,
    )
    .await?;

    write_packet(
        &mut modern,
        &codec,
        &click_window(TestJavaProtocol::Je340, 1, 0, 2, Some((17, 1, 0))),
    )
    .await?;
    assert_no_java_packet(
        &mut modern,
        &codec,
        &mut modern_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::ConfirmTransaction,
    )
    .await?;

    let result_preview = {
        write_packet(&mut legacy, &codec, &held_item_change(4)).await?;
        let held_item = read_until_java_packet(
            &mut legacy,
            &codec,
            &mut legacy_buffer,
            TestJavaProtocol::Je5,
            TestJavaPacket::HeldItemChange,
        )
        .await?;
        assert_eq!(held_item_from_packet(&held_item)?, 4);

        write_packet(
            &mut modern,
            &codec,
            &confirm_transaction_ack(TestJavaProtocol::Je340, 0, 1, false),
        )
        .await?;
        write_packet(
            &mut modern,
            &codec,
            &click_window(TestJavaProtocol::Je340, 1, 0, 3, Some((17, 1, 0))),
        )
        .await?;
        let accept_ack = read_until_confirm_transaction(
            &mut modern,
            &codec,
            &mut modern_buffer,
            TestJavaProtocol::Je340,
            0,
            3,
            16,
        )
        .await?;
        assert_eq!(
            decode_confirm_transaction(TestJavaProtocol::Je340, &accept_ack)?,
            (0, 3, true)
        );
        read_until_set_slot(
            &mut modern,
            &codec,
            &mut modern_buffer,
            TestJavaProtocol::Je340,
            0,
            0,
            16,
        )
        .await?
    };
    assert_eq!(
        decode_set_slot(TestJavaProtocol::Je340, &result_preview)?,
        (0, 0, Some((5, 4, 0)))
    );

    server.shutdown().await
}
