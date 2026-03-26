use super::*;

#[tokio::test]
async fn runtime_test_helper_opens_and_closes_crafting_table_window() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        multi_version_creative_server_config(temp_dir.path().join("world")),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut stream, mut buffer, _) = login_modern_1_12(addr, "alpha").await?;
    let player_id = server
        .session_status()
        .await
        .into_iter()
        .find_map(|session| session.player_id)
        .expect("logged-in player should have a player id");

    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 36, 17, 1, 0),
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
                0,
                36,
                16,
            )
            .await?,
        )?,
        (0, 36, Some((17, 1, 0)))
    );

    open_test_crafting_table(&server, player_id, 2, "Crafting").await?;

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
            2,
            "minecraft:crafting_table".to_string(),
            "{\"text\":\"Crafting\"}".to_string(),
            0,
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
        window_items_slot(TestJavaProtocol::Je340, &open_contents, 37)?,
        Some((17, 1, 0))
    );

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 2, 37, 0, 1, None),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        2,
        1,
        37,
        None,
        Some((17, 1, 0)),
        None,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 2, 1, 0, 2, Some((17, 1, 0))),
    )
    .await?;
    let first_place_ack = read_until_confirm_transaction(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        2,
        2,
        16,
    )
    .await?;
    assert_eq!(
        decode_confirm_transaction(TestJavaProtocol::Je340, &first_place_ack)?,
        (2, 2, true)
    );
    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                2,
                0,
                16,
            )
            .await?,
        )?,
        (2, 0, Some((5, 4, 0)))
    );
    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                2,
                1,
                16,
            )
            .await?,
        )?,
        (2, 1, Some((17, 1, 0)))
    );

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 2, 0, 0, 3, None),
    )
    .await?;
    let result_ack = read_until_confirm_transaction(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        2,
        3,
        16,
    )
    .await?;
    assert_eq!(
        decode_confirm_transaction(TestJavaProtocol::Je340, &result_ack)?,
        (2, 3, true)
    );
    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                2,
                0,
                16,
            )
            .await?,
        )?,
        (2, 0, None)
    );
    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                2,
                1,
                16,
            )
            .await?,
        )?,
        (2, 1, None)
    );
    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                -1,
                -1,
                16,
            )
            .await?,
        )?,
        (-1, -1, Some((5, 4, 0)))
    );

    write_packet(
        &mut stream,
        &codec,
        &close_window(TestJavaProtocol::Je340, 2),
    )
    .await?;

    let close_window = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::CloseWindow,
    )
    .await?;
    assert_eq!(
        decode_close_window(TestJavaProtocol::Je340, &close_window)?,
        2
    );

    let player_contents = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::WindowItems,
    )
    .await?;
    let mut reader = PacketReader::new(&player_contents);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x14);
    assert_eq!(reader.read_u8().expect("window id should decode"), 0);

    server.shutdown().await
}

#[tokio::test]
async fn world_backed_chest_moves_items_and_resyncs_player_inventory_on_close()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        multi_version_creative_server_config(temp_dir.path().join("world")),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut stream, mut buffer, _) = login_modern_1_12(addr, "alpha").await?;
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 36, 54, 1, 0),
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
                0,
                36,
                16,
            )
            .await?,
        )?,
        (0, 36, Some((54, 1, 0)))
    );
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 37, 1, 2, 0),
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
                0,
                37,
                16,
            )
            .await?,
        )?,
        (0, 37, Some((1, 2, 0)))
    );

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
        window_items_slot(TestJavaProtocol::Je340, &open_contents, 54)?,
        Some((54, 1, 0))
    );
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
    assert_eq!(
        decode_confirm_transaction(
            TestJavaProtocol::Je340,
            &read_until_confirm_transaction(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                1,
                2,
                16,
            )
            .await?,
        )?,
        (1, 2, true)
    );
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
    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                -1,
                -1,
                16,
            )
            .await?,
        )?,
        (-1, -1, None)
    );

    write_packet(
        &mut stream,
        &codec,
        &close_window(TestJavaProtocol::Je340, 1),
    )
    .await?;
    let close_window = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::CloseWindow,
    )
    .await?;
    assert_eq!(
        decode_close_window(TestJavaProtocol::Je340, &close_window)?,
        1
    );

    let player_contents = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::WindowItems,
    )
    .await?;
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je340, &player_contents, 36)?,
        Some((54, 1, 0))
    );
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je340, &player_contents, 37)?,
        None
    );

    server.shutdown().await
}
