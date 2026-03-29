use super::*;

#[tokio::test]
async fn world_backed_chest_place_open_and_persist_across_restart() -> Result<(), RuntimeError> {
    let _guard = lock_window_transaction_tests().await;
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let codec = MinecraftWireCodec;

    let server = build_test_server(
        multi_version_creative_server_config(world_dir.clone()),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let (mut stream, mut buffer, _) = login_modern_1_12(addr, "alpha").await?;

    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 36, 54, 1, 0),
    )
    .await?;
    let _ = read_until_set_slot(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        0,
        36,
        16,
    )
    .await?;
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 37, 1, 2, 0),
    )
    .await?;
    let _ = read_until_set_slot(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        0,
        37,
        16,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    write_packet(
        &mut stream,
        &codec,
        &player_block_placement_1_12(2, 4, 0, 1, 0),
    )
    .await?;

    let open_window = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::OpenWindow,
    )
    .await?;
    assert_eq!(
        decode_open_window(TestJavaProtocol::Je340, &open_window)?,
        (
            1,
            "minecraft:chest".to_string(),
            "{\"text\":\"Chest\"}".to_string(),
            27,
            None,
        )
    );
    let open_contents = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::WindowItems,
    )
    .await?;
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je340, &open_contents, 55)?,
        Some((1, 2, 0))
    );

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 55, 0, 1, None),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        1,
        1,
        55,
        None,
        Some((1, 2, 0)),
        None,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 0, 0, 2, Some((1, 2, 0))),
    )
    .await?;
    let _ = read_until_confirm_transaction(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        1,
        2,
        16,
    )
    .await?;
    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                1,
                0,
                16,
            )
            .await?,
        )?,
        (1, 0, Some((1, 2, 0)))
    );
    let _ = read_until_set_slot(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        -1,
        -1,
        16,
    )
    .await?;

    server.shutdown().await?;

    let restarted = build_test_server(
        multi_version_creative_server_config(world_dir),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&restarted);
    let (mut stream, mut buffer, _) = login_modern_1_12(addr, "alpha").await?;

    write_packet(
        &mut stream,
        &codec,
        &player_block_placement_1_12(2, 4, 0, 1, 0),
    )
    .await?;
    let open_window = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::OpenWindow,
    )
    .await?;
    assert_eq!(
        decode_open_window(TestJavaProtocol::Je340, &open_window)?.0,
        1
    );
    let open_contents = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::WindowItems,
    )
    .await?;
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je340, &open_contents, 0)?,
        Some((1, 2, 0))
    );

    restarted.shutdown().await
}

#[tokio::test]
async fn world_backed_chest_syncs_slot_updates_to_other_viewers() -> Result<(), RuntimeError> {
    let _guard = lock_window_transaction_tests().await;
    let temp_dir = tempdir()?;
    let server = build_test_server(
        multi_version_creative_server_config(temp_dir.path().join("world")),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut first, mut first_buffer, _) = login_modern_1_12(addr, "alpha").await?;
    let (mut second, mut second_buffer, _) = login_modern_1_12(addr, "beta").await?;

    write_packet(
        &mut first,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 36, 54, 1, 0),
    )
    .await?;
    let _ = read_until_set_slot(
        &mut first,
        &codec,
        &mut first_buffer,
        TestJavaProtocol::Je340,
        0,
        36,
        16,
    )
    .await?;
    write_packet(
        &mut first,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 37, 1, 2, 0),
    )
    .await?;
    let _ = read_until_set_slot(
        &mut first,
        &codec,
        &mut first_buffer,
        TestJavaProtocol::Je340,
        0,
        37,
        16,
    )
    .await?;

    write_packet(
        &mut first,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let _ = read_until_java_packet(
        &mut second,
        &codec,
        &mut second_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::BlockChange,
    )
    .await?;

    write_packet(
        &mut first,
        &codec,
        &player_block_placement_1_12(2, 4, 0, 1, 0),
    )
    .await?;
    let _ = read_until_java_packet(
        &mut first,
        &codec,
        &mut first_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::OpenWindow,
    )
    .await?;
    let _ = read_until_java_packet(
        &mut first,
        &codec,
        &mut first_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::WindowItems,
    )
    .await?;

    write_packet(
        &mut second,
        &codec,
        &player_block_placement_1_12(2, 4, 0, 1, 0),
    )
    .await?;
    let _ = read_until_java_packet(
        &mut second,
        &codec,
        &mut second_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::OpenWindow,
    )
    .await?;
    let _ = read_until_java_packet(
        &mut second,
        &codec,
        &mut second_buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::WindowItems,
    )
    .await?;

    write_packet(
        &mut first,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 55, 0, 1, None),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut first,
        &mut first_buffer,
        &codec,
        1,
        1,
        55,
        None,
        Some((1, 2, 0)),
        None,
    )
    .await?;

    write_packet(
        &mut first,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 0, 0, 2, Some((1, 2, 0))),
    )
    .await?;
    let _ = read_until_confirm_transaction(
        &mut first,
        &codec,
        &mut first_buffer,
        TestJavaProtocol::Je340,
        1,
        2,
        16,
    )
    .await?;
    let _ = read_until_set_slot(
        &mut first,
        &codec,
        &mut first_buffer,
        TestJavaProtocol::Je340,
        1,
        0,
        16,
    )
    .await?;
    let _ = read_until_set_slot(
        &mut first,
        &codec,
        &mut first_buffer,
        TestJavaProtocol::Je340,
        -1,
        -1,
        16,
    )
    .await?;

    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot(
                &mut second,
                &codec,
                &mut second_buffer,
                TestJavaProtocol::Je340,
                1,
                0,
                16,
            )
            .await?,
        )?,
        (1, 0, Some((1, 2, 0)))
    );

    server.shutdown().await
}
