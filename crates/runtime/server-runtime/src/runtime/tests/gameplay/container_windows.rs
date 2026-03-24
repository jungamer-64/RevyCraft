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
    let pickup_ack = read_until_confirm_transaction(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        2,
        1,
        16,
    )
    .await?;
    assert_eq!(
        decode_confirm_transaction(TestJavaProtocol::Je340, &pickup_ack)?,
        (2, 1, true)
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
                37,
                16,
            )
            .await?,
        )?,
        (2, 37, None)
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
        (-1, -1, Some((17, 1, 0)))
    );

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

    close_test_container(&server, player_id, 2).await?;

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
async fn runtime_test_helper_opens_moves_items_and_closes_chest_window() -> Result<(), RuntimeError>
{
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
        &creative_inventory_action(TestJavaProtocol::Je340, 36, 1, 2, 0),
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
        (0, 36, Some((1, 2, 0)))
    );

    open_test_chest(&server, player_id, 4, "Chest").await?;

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
            4,
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
        Some((1, 2, 0))
    );

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 4, 54, 0, 1, None),
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
                4,
                1,
                16,
            )
            .await?,
        )?,
        (4, 1, true)
    );
    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                4,
                54,
                16,
            )
            .await?,
        )?,
        (4, 54, None)
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
        (-1, -1, Some((1, 2, 0)))
    );

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 4, 0, 0, 2, Some((1, 2, 0))),
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
                4,
                2,
                16,
            )
            .await?,
        )?,
        (4, 2, true)
    );
    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                4,
                0,
                16,
            )
            .await?,
        )?,
        (4, 0, Some((1, 2, 0)))
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

    close_test_container(&server, player_id, 4).await?;
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
        4
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
        window_items_slot(TestJavaProtocol::Je340, &player_contents, 9)?,
        Some((1, 2, 0))
    );

    server.shutdown().await
}
