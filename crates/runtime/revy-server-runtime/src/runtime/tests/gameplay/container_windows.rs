use super::*;

#[tokio::test]
async fn full_reload_preserves_unacknowledged_inventory_rejection() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_reloadable_test_server(
        multi_version_creative_server_config(temp_dir.path().join("world")),
        packaged_default_registries(ALL_PROTOCOL_PLUGIN_IDS)?,
    )
    .await?;
    let codec = MinecraftWireCodec;
    let protocol = TestJavaProtocol::Je340;
    let (mut stream, mut buffer, _) =
        login_modern_1_12(listener_addr(&server), "rejection-reload").await?;
    let status = server.session_status().await;
    let session = &status[0];
    let snapshot = mc_proto_common::ProtocolSessionSnapshot {
        connection_id: session.connection_id,
        phase: session.phase,
        player_id: session.player_id,
        entity_id: session.entity_id,
    };
    let active = active_protocol_registry(&server)
        .resolve_adapter(JE_340_ADAPTER_ID)
        .expect("active protocol");
    let initial = active.export_session_state(&snapshot)?;
    write_packet(
        &mut stream,
        &codec,
        &click_window(protocol, 999, 0, 10, None),
    )
    .await?;
    let reject =
        read_until_confirm_transaction(&mut stream, &codec, &mut buffer, protocol, 0, 10, 16)
            .await?;
    assert_eq!(
        decode_confirm_transaction(protocol, &reject)?,
        (0, 10, false)
    );
    let pending = active.export_session_state(&snapshot)?;
    assert_ne!(pending, initial);

    server.reload_runtime_full().await?;
    let replacement = active_protocol_registry(&server)
        .resolve_adapter(JE_340_ADAPTER_ID)
        .expect("replacement protocol");
    assert!(!Arc::ptr_eq(&active, &replacement));
    assert_eq!(replacement.export_session_state(&snapshot)?, pending);
    assert_eq!(active.export_session_state(&snapshot)?, pending);

    write_packet(
        &mut stream,
        &codec,
        &confirm_transaction_ack(protocol, 0, 10, false),
    )
    .await?;
    write_packet(
        &mut stream,
        &codec,
        &click_window(protocol, 999, 0, 11, None),
    )
    .await?;
    let reject =
        read_until_confirm_transaction(&mut stream, &codec, &mut buffer, protocol, 0, 11, 16)
            .await?;
    assert_eq!(
        decode_confirm_transaction(protocol, &reject)?,
        (0, 11, false)
    );
    assert_ne!(replacement.export_session_state(&snapshot)?, pending);
    // The acknowledgement belongs only to the committed instance, not the retired one.
    assert_eq!(active.export_session_state(&snapshot)?, pending);
    server.shutdown().await
}

#[tokio::test]
async fn world_backed_crafting_table_opens_and_crafts_chest_via_protocol()
-> Result<(), RuntimeError> {
    let _guard = lock_window_transaction_tests().await;
    let temp_dir = tempdir()?;
    let server = build_test_server(
        multi_version_creative_server_config(temp_dir.path().join("world")),
        packaged_default_registries(ALL_PROTOCOL_PLUGIN_IDS)?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut stream, mut buffer, _) = login_modern_1_12(addr, "crafting-world")
        .await
        .map_err(|error| RuntimeError::Config(format!("crafting table login: {error}")))?;
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 36, 58, 1, 0),
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
        (0, 36, Some((58, 1, 0)))
    );
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je340, 37, 5, 8, 0),
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
        (0, 37, Some((5, 8, 0)))
    );

    write_packet(
        &mut stream,
        &codec,
        &player_block_placement_1_12(2, 3, 0, 1, 0),
    )
    .await?;
    let place_change = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::BlockChange,
    )
    .await?;
    assert_eq!(
        block_change_from_packet_1_12(&place_change)?,
        (2, 4, 0, 58 << 4)
    );
    write_packet(
        &mut stream,
        &codec,
        &player_block_placement_1_12(2, 4, 0, 1, 0),
    )
    .await?;

    let open_window = read_until_java_packet_preserving_nonmatching(
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
            "minecraft:crafting_table".to_string(),
            "{\"text\":\"Crafting\"}".to_string(),
            0,
            None,
        )
    );

    let open_contents = read_until_java_packet_preserving_nonmatching(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::WindowItems,
    )
    .await
    .map_err(|error| {
        RuntimeError::Config(format!("world crafting table open contents: {error}"))
    })?;
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je340, &open_contents, 38)?,
        Some((5, 8, 0))
    );

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 38, 0, 1, None),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        1,
        1,
        38,
        None,
        Some((5, 8, 0)),
        None,
    )
    .await?;

    for (index, slot) in [1_i16, 2, 3, 4, 6, 7, 8, 9].into_iter().enumerate() {
        let action_number = i16::try_from(index + 2).expect("action number should fit");
        let remaining = 7_u8.saturating_sub(u8::try_from(index).expect("index should fit"));
        write_packet(
            &mut stream,
            &codec,
            &click_window_in_window(
                TestJavaProtocol::Je340,
                1,
                slot,
                1,
                action_number,
                Some((5, 1, 0)),
            ),
        )
        .await?;
        let _ = read_click_transcript_and_ack_reject_if_needed(
            TestJavaProtocol::Je340,
            &mut stream,
            &mut buffer,
            &codec,
            1,
            action_number,
            slot,
            Some((5, 1, 0)),
            (remaining > 0).then_some((5, remaining, 0)),
            None,
        )
        .await?;
    }

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
        (1, 0, Some((54, 1, 0)))
    );

    write_packet(
        &mut stream,
        &codec,
        &click_window_in_window(TestJavaProtocol::Je340, 1, 0, 0, 10, None),
    )
    .await?;
    let _ = read_click_transcript_and_ack_reject_if_needed(
        TestJavaProtocol::Je340,
        &mut stream,
        &mut buffer,
        &codec,
        1,
        10,
        0,
        None,
        Some((54, 1, 0)),
        None,
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
                1,
                16,
            )
            .await?,
        )?,
        (1, 1, None)
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
                9,
                16,
            )
            .await?,
        )?,
        (1, 9, None)
    );

    server.shutdown().await
}

#[tokio::test]
async fn world_backed_chest_moves_items_and_resyncs_player_inventory_on_close()
-> Result<(), RuntimeError> {
    let _guard = lock_window_transaction_tests().await;
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

    let open_window = read_until_java_packet_preserving_nonmatching(
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

    let open_contents = read_until_java_packet_preserving_nonmatching(
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

    let player_contents = read_until_java_packet_preserving_nonmatching(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je340,
        TestJavaPacket::WindowItems,
    )
    .await
    .map_err(|error| {
        RuntimeError::Config(format!("world crafting table close contents: {error}"))
    })?;
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
