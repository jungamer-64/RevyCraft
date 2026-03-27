use super::*;

#[tokio::test]
async fn world_backed_furnace_opens_smelts_and_closes_via_protocol() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        multi_version_creative_server_config(temp_dir.path().join("world")),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let furnace_packet_timeout = Duration::from_secs(10);

    let (mut stream, mut buffer, _) = login_modern_1_12(addr, "alpha").await?;
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 36, 61, 1, 0),
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
        &creative_inventory_action(TestJavaProtocol::Je340, 37, 12, 1, 0),
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
        &creative_inventory_action(TestJavaProtocol::Je340, 38, 5, 1, 0),
    )
    .await?;
    let _ = read_until_set_slot(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        0,
        38,
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
            "minecraft:furnace".to_string(),
            "{\"text\":\"Furnace\"}".to_string(),
            3,
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
        window_items_slot(TestJavaProtocol::Je340, &open_contents, 31)?,
        Some((12, 1, 0))
    );
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je340, &open_contents, 32)?,
        Some((5, 1, 0))
    );

    for (property_id, value) in [(0, 0), (1, 0), (2, 0), (3, 200)] {
        assert_eq!(
            decode_window_property(
                TestJavaProtocol::Je340,
                &read_until_window_property_with_timeout(
                    &mut stream,
                    &codec,
                    &mut buffer,
                    TestJavaProtocol::Je340,
                    1,
                    property_id,
                    furnace_packet_timeout,
                )
                .await?,
            )?,
            (1, property_id, value)
        );
    }

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 31, 0, 1, None),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        1,
        1,
        31,
        None,
        Some((12, 1, 0)),
        None,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 0, 0, 2, Some((12, 1, 0))),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        1,
        2,
        0,
        Some((12, 1, 0)),
        None,
        None,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 32, 0, 3, None),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        1,
        3,
        32,
        None,
        Some((5, 1, 0)),
        None,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 1, 0, 4, Some((5, 1, 0))),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        1,
        4,
        1,
        Some((5, 1, 0)),
        None,
        None,
    )
    .await?;

    server.runtime.tick().await?;

    // Parallel suite runs can delay outbound flushes, so this test gives the furnace transcript a
    // larger deadline while still asserting the same property and slot sequence.
    assert_eq!(
        decode_window_property(
            TestJavaProtocol::Je340,
            &read_until_window_property_with_timeout(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                1,
                0,
                furnace_packet_timeout,
            )
            .await?,
        )?,
        (1, 0, 300)
    );
    assert_eq!(
        decode_window_property(
            TestJavaProtocol::Je340,
            &read_until_window_property_with_timeout(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                1,
                1,
                furnace_packet_timeout,
            )
            .await?,
        )?,
        (1, 1, 300)
    );
    assert_eq!(
        decode_window_property(
            TestJavaProtocol::Je340,
            &read_until_window_property_with_timeout(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                1,
                2,
                furnace_packet_timeout,
            )
            .await?,
        )?,
        (1, 2, 1)
    );

    for _ in 2..200 {
        server.runtime.tick().await?;
        let _ = read_until_window_property_with_timeout(
            &mut stream,
            &codec,
            &mut buffer,
            TestJavaProtocol::Je340,
            1,
            0,
            furnace_packet_timeout,
        )
        .await?;
        let _ = read_until_window_property_with_timeout(
            &mut stream,
            &codec,
            &mut buffer,
            TestJavaProtocol::Je340,
            1,
            2,
            furnace_packet_timeout,
        )
        .await?;
    }

    server.runtime.tick().await?;

    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot_with_timeout(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                1,
                0,
                furnace_packet_timeout,
            )
            .await?,
        )?,
        (1, 0, None)
    );
    assert_eq!(
        decode_set_slot(
            TestJavaProtocol::Je340,
            &read_until_set_slot_with_timeout(
                &mut stream,
                &codec,
                &mut buffer,
                TestJavaProtocol::Je340,
                1,
                2,
                furnace_packet_timeout,
            )
            .await?,
        )?,
        (1, 2, Some((20, 1, 0)))
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

    server.shutdown().await
}

#[tokio::test]
async fn world_backed_furnace_output_persists_across_restart() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let world_dir = temp_dir.path().join("world");
    let server = build_test_server(
        multi_version_creative_server_config(world_dir.clone()),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut stream, mut buffer, _) = login_modern_1_12(addr, "alpha").await?;
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 36, 61, 1, 0),
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
        &creative_inventory_action(TestJavaProtocol::Je340, 37, 12, 1, 0),
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
        &creative_inventory_action(TestJavaProtocol::Je340, 38, 5, 1, 0),
    )
    .await?;
    let _ = read_until_set_slot(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        0,
        38,
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
    let _ = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::OpenWindow,
    )
    .await?;
    let _ = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::WindowItems,
    )
    .await?;
    for property_id in 0..=3 {
        let _ = read_until_window_property(
            &mut stream,
            &codec,
            &mut buffer,
            TestJavaProtocol::Je340,
            1,
            property_id,
            16,
        )
        .await?;
    }

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 31, 0, 1, None),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        1,
        1,
        31,
        None,
        Some((12, 1, 0)),
        None,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 0, 0, 2, Some((12, 1, 0))),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        1,
        2,
        0,
        Some((12, 1, 0)),
        None,
        None,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 32, 0, 3, None),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        1,
        3,
        32,
        None,
        Some((5, 1, 0)),
        None,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 1, 0, 4, Some((5, 1, 0))),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        1,
        4,
        1,
        Some((5, 1, 0)),
        None,
        None,
    )
    .await?;

    for _ in 0..200 {
        server.runtime.tick().await?;
    }

    server.shutdown().await?;

    let restarted = build_test_server(
        multi_version_creative_server_config(world_dir),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&restarted);
    let codec = MinecraftWireCodec;
    let (mut stream, mut buffer, _) = login_modern_1_12(addr, "alpha").await?;
    write_packet(
        &mut stream,
        &codec,
        &player_block_placement_1_12(2, 4, 0, 1, 0),
    )
    .await?;
    let _ = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::OpenWindow,
    )
    .await?;
    let window_items = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::WindowItems,
    )
    .await?;
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je340, &window_items, 2)?,
        Some((20, 1, 0))
    );

    restarted.shutdown().await
}
