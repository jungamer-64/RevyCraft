use super::*;

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
            &creative_inventory_action(TestJavaProtocol::Je340, 45, 20, 64, 0),
        )
        .await?;
        read_until_java_packet(
            &mut modern,
            &codec,
            &mut modern_buffer,
            TestJavaProtocol::Je340,
            TestJavaPacket::SetSlot,
        )
        .await?
    };
    assert_eq!(set_slot_slot(TestJavaProtocol::Je340, &set_slot)?, 45);

    server.shutdown().await?;

    let restarted = build_test_server(
        multi_version_creative_server_config(world_dir),
        plugin_test_registries_all()?,
    )
    .await?;
    let addr = listener_addr(&restarted);

    let (_, _, window_items) = login_modern_1_12(addr, "alpha").await?;
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je340, &window_items, 45)?,
        Some((20, 64, 0))
    );

    let (_, _, legacy_window_items) = login_modern_1_8(addr, "beta").await?;
    assert!(window_items_slot(TestJavaProtocol::Je47, &legacy_window_items, 45).is_err());

    restarted.shutdown().await
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
    craft_log_into_planks(TestJavaProtocol::Je5, &mut stream, &mut buffer, &codec).await?;

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
    craft_log_into_planks(TestJavaProtocol::Je47, &mut stream, &mut buffer, &codec).await?;

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
    craft_log_into_planks(TestJavaProtocol::Je340, &mut stream, &mut buffer, &codec).await?;

    server.shutdown().await
}

#[tokio::test]
async fn legacy_rejected_window_zero_click_requires_apology_before_more_clicks()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        creative_server_config(temp_dir.path().join("world")),
        plugin_test_registries_tcp_only()?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (mut stream, mut buffer, _) = login_legacy(addr, "reject-legacy").await?;
    write_packet(
        &mut stream,
        &codec,
        &creative_inventory_action(TestJavaProtocol::Je5, 36, 17, 1, 0),
    )
    .await?;
    let _ = read_until_set_slot(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        0,
        36,
        16,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &click_window(TestJavaProtocol::Je5, 36, 0, 1, Some((17, 1, 0))),
    )
    .await?;
    let reject_ack = read_until_confirm_transaction(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        0,
        1,
        16,
    )
    .await?;
    assert_eq!(
        decode_confirm_transaction(TestJavaProtocol::Je5, &reject_ack)?,
        (0, 1, false)
    );
    let window_items = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::WindowItems,
    )
    .await?;
    assert_eq!(
        window_items_slot(TestJavaProtocol::Je5, &window_items, 36)?,
        None
    );
    let slot_resync = read_until_set_slot(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        0,
        36,
        16,
    )
    .await?;
    assert_eq!(
        decode_set_slot(TestJavaProtocol::Je5, &slot_resync)?,
        (0, 36, None)
    );
    let held_slot = read_until_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;
    assert_eq!(held_item_from_packet(&held_slot)?, 0);
    let cursor_resync = read_until_set_slot(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        -1,
        -1,
        16,
    )
    .await?;
    assert_eq!(
        decode_set_slot(TestJavaProtocol::Je5, &cursor_resync)?,
        (-1, -1, Some((17, 1, 0)))
    );

    write_packet(
        &mut stream,
        &codec,
        &click_window(TestJavaProtocol::Je5, 1, 0, 2, Some((17, 1, 0))),
    )
    .await?;
    assert_no_java_packet(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::ConfirmTransaction,
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &confirm_transaction_ack(TestJavaProtocol::Je5, 0, 1, false),
    )
    .await?;

    write_packet(
        &mut stream,
        &codec,
        &click_window(TestJavaProtocol::Je5, 1, 0, 3, Some((17, 1, 0))),
    )
    .await?;
    let accept_ack = read_until_confirm_transaction(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        0,
        3,
        16,
    )
    .await?;
    assert_eq!(
        decode_confirm_transaction(TestJavaProtocol::Je5, &accept_ack)?,
        (0, 3, true)
    );
    let result_preview = read_until_set_slot(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        0,
        0,
        16,
    )
    .await?;
    assert_eq!(
        decode_set_slot(TestJavaProtocol::Je5, &result_preview)?,
        (0, 0, Some((5, 4, 0)))
    );

    server.shutdown().await
}
