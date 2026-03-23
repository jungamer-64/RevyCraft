use super::*;

#[tokio::test]
async fn mixed_java_versions_share_login_movement_and_block_sync() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.game_mode = 1;
    config.topology.enabled_adapters = Some(vec![
        JE_1_7_10_ADAPTER_ID.to_string(),
        JE_1_8_X_ADAPTER_ID.to_string(),
        JE_1_12_2_ADAPTER_ID.to_string(),
    ]);
    let server = build_test_server(config, plugin_test_registries_all()?).await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, 5, "legacy", 0x30, 12).await?;
    let (mut modern_18, mut modern_18_buffer) =
        connect_and_login_java_client(addr, &codec, 47, "middle", 0x30, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

    let (mut modern_112, mut modern_112_buffer) =
        connect_and_login_java_client(addr, &codec, 340, "latest", 0x14, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;
    let _ = read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x0c, 12).await?;

    write_packet(
        &mut modern_18,
        &codec,
        &player_position_look_1_8(32.5, 4.0, 0.5, 90.0, 0.0),
    )
    .await?;
    let legacy_teleport =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x18, 16).await?;
    let modern_112_teleport =
        read_until_packet_id(&mut modern_112, &codec, &mut modern_112_buffer, 0x4c, 16).await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);
    assert_eq!(packet_id(&modern_112_teleport), 0x4c);

    write_packet(
        &mut modern_112,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let legacy_block_change =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x23, 16).await?;
    let modern_18_block_change =
        read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x23, 16).await?;
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
async fn adapter_mapped_gameplay_profiles_can_run_concurrently() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.game_mode = 1;
    config.topology.enabled_adapters = Some(vec![
        JE_1_7_10_ADAPTER_ID.to_string(),
        JE_1_12_2_ADAPTER_ID.to_string(),
    ]);
    config.profiles.default_gameplay = "canonical".to_string();
    config.profiles.gameplay_map = gameplay_profile_map(&[
        (JE_1_7_10_ADAPTER_ID, "readonly"),
        (JE_1_12_2_ADAPTER_ID, "canonical"),
    ]);
    let server = build_test_server(config, plugin_test_registries_all()?).await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, 5, "legacy-readonly", 0x30, 12).await?;
    let (mut modern, mut modern_buffer) =
        connect_and_login_java_client(addr, &codec, 340, "modern-canonical", 0x14, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

    write_packet(
        &mut modern,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let _ = read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x0b, 16).await?;
    let legacy_block_change =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x23, 16).await?;
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
    assert_no_packet_id(&mut modern, &codec, &mut modern_buffer, 0x0b).await?;

    write_packet(
        &mut legacy,
        &codec,
        &player_position_look(12.5, 4.0, 0.5, 0.0, 0.0),
    )
    .await?;
    let modern_teleport =
        read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x4c, 16).await?;
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
        JE_1_7_10_ADAPTER_ID.to_string(),
        JE_1_8_X_ADAPTER_ID.to_string(),
        JE_1_12_2_ADAPTER_ID.to_string(),
        BE_PLACEHOLDER_ADAPTER_ID.to_string(),
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
    write_packet(&mut status_stream, &codec, &encode_handshake(5, 1)?).await?;
    write_packet(&mut status_stream, &codec, &[0x00]).await?;
    let mut status_buffer = BytesMut::new();
    let status = read_packet(&mut status_stream, &codec, &mut status_buffer).await?;
    assert_eq!(packet_id(&status), 0x00);

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, 5, "legacy", 0x30, 12).await?;
    let (mut modern_18, mut modern_18_buffer) =
        connect_and_login_java_client(addr, &codec, 47, "middle", 0x30, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

    let (mut modern_112, mut modern_112_buffer) =
        connect_and_login_java_client(addr, &codec, 340, "latest", 0x14, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;
    let _ = read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x0c, 12).await?;

    write_packet(
        &mut modern_18,
        &codec,
        &player_position_look_1_8(32.5, 4.0, 0.5, 90.0, 0.0),
    )
    .await?;
    let legacy_teleport =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x18, 16).await?;
    let modern_112_teleport =
        read_until_packet_id(&mut modern_112, &codec, &mut modern_112_buffer, 0x4c, 16).await?;
    assert_eq!(packet_id(&legacy_teleport), 0x18);
    assert_eq!(packet_id(&modern_112_teleport), 0x4c);

    write_packet(
        &mut modern_112,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let legacy_block_change =
        read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x23, 16).await?;
    let modern_18_block_change =
        read_until_packet_id(&mut modern_18, &codec, &mut modern_18_buffer, 0x23, 16).await?;
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
        JE_1_7_10_ADAPTER_ID.to_string(),
        JE_1_8_X_ADAPTER_ID.to_string(),
        JE_1_12_2_ADAPTER_ID.to_string(),
    ]);
    let server = build_test_server(config, plugin_test_registries_all()?).await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut legacy, mut legacy_buffer) =
        connect_and_login_java_client(addr, &codec, 5, "legacy-craft", 0x30, 12).await?;
    let (mut modern, mut modern_buffer) =
        connect_and_login_java_client(addr, &codec, 340, "modern-craft", 0x14, 24).await?;
    let _ = read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x0c, 12).await?;

    write_packet(
        &mut modern,
        &codec,
        &creative_inventory_action_1_12(36, 17, 1, 0),
    )
    .await?;
    let _ = read_until_set_slot(&mut modern, &codec, &mut modern_buffer, 0x16, 0, 36, 16).await?;
    write_packet(
        &mut modern,
        &codec,
        &click_window_1_12(36, 0, 1, Some((17, 1, 0))),
    )
    .await?;
    let reject_ack =
        read_until_confirm_transaction(&mut modern, &codec, &mut modern_buffer, 0x11, 0, 1, 16)
            .await?;
    assert_eq!(
        decode_confirm_transaction(&reject_ack, 0x11)?,
        (0, 1, false)
    );
    let modern_resync =
        read_until_packet_id(&mut modern, &codec, &mut modern_buffer, 0x14, 16).await?;
    assert_eq!(
        window_items_slot_with_packet_id(&modern_resync, 0x14, 36)?,
        None
    );
    let _ = read_until_set_slot(&mut modern, &codec, &mut modern_buffer, 0x16, 0, 36, 16).await?;
    let _ = read_until_set_slot(&mut modern, &codec, &mut modern_buffer, 0x16, -1, -1, 16).await?;

    write_packet(
        &mut modern,
        &codec,
        &click_window_1_12(1, 0, 2, Some((17, 1, 0))),
    )
    .await?;
    assert_no_packet_id(&mut modern, &codec, &mut modern_buffer, 0x11).await?;

    let result_preview = {
        write_packet(&mut legacy, &codec, &held_item_change(4)).await?;
        let held_item =
            read_until_packet_id(&mut legacy, &codec, &mut legacy_buffer, 0x09, 8).await?;
        assert_eq!(held_item_from_packet(&held_item)?, 4);

        write_packet(&mut modern, &codec, &confirm_transaction_1_12(0, 1, false)).await?;
        write_packet(
            &mut modern,
            &codec,
            &click_window_1_12(1, 0, 3, Some((17, 1, 0))),
        )
        .await?;
        let accept_ack =
            read_until_confirm_transaction(&mut modern, &codec, &mut modern_buffer, 0x11, 0, 3, 16)
                .await?;
        assert_eq!(decode_confirm_transaction(&accept_ack, 0x11)?, (0, 3, true));
        read_until_set_slot(&mut modern, &codec, &mut modern_buffer, 0x16, 0, 0, 16).await?
    };
    assert_eq!(
        decode_set_slot(&result_preview, 0x16)?,
        (0, 0, Some((5, 4, 0)))
    );

    server.shutdown().await
}
