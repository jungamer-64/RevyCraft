use super::*;

mod container_windows;
mod bedrock;
mod furnace;
mod general;
mod player_window;
mod world_chest;

fn creative_server_config(world_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.game_mode = 1;
    config
}

fn multi_version_creative_server_config(world_dir: PathBuf) -> ServerConfig {
    let mut config = creative_server_config(world_dir);
    config.topology.enabled_adapters = Some(vec![
        JE_5_ADAPTER_ID.into(),
        JE_47_ADAPTER_ID.into(),
        JE_340_ADAPTER_ID.into(),
    ]);
    config
}

async fn login_java_client_with_packet(
    addr: SocketAddr,
    protocol: TestJavaProtocol,
    username: &str,
    expected_packet: TestJavaPacket,
) -> Result<(tokio::net::TcpStream, BytesMut, Vec<u8>), RuntimeError> {
    let codec = MinecraftWireCodec;
    connect_and_login_java_client_until(addr, &codec, protocol, username, expected_packet).await
}

async fn login_legacy(
    addr: SocketAddr,
    username: &str,
) -> Result<(tokio::net::TcpStream, BytesMut, Vec<u8>), RuntimeError> {
    login_java_client_with_packet(
        addr,
        TestJavaProtocol::Je5,
        username,
        TestJavaPacket::WindowItems,
    )
    .await
}

async fn login_legacy_with_position(
    addr: SocketAddr,
    username: &str,
) -> Result<(tokio::net::TcpStream, BytesMut, Vec<u8>), RuntimeError> {
    login_java_client_with_packet(
        addr,
        TestJavaProtocol::Je5,
        username,
        TestJavaPacket::PositionAndLook,
    )
    .await
}

async fn login_modern_1_8(
    addr: SocketAddr,
    username: &str,
) -> Result<(tokio::net::TcpStream, BytesMut, Vec<u8>), RuntimeError> {
    login_java_client_with_packet(
        addr,
        TestJavaProtocol::Je47,
        username,
        TestJavaPacket::WindowItems,
    )
    .await
}

async fn login_modern_1_12(
    addr: SocketAddr,
    username: &str,
) -> Result<(tokio::net::TcpStream, BytesMut, Vec<u8>), RuntimeError> {
    login_java_client_with_packet(
        addr,
        TestJavaProtocol::Je340,
        username,
        TestJavaPacket::WindowItems,
    )
    .await
}

async fn craft_log_into_planks(
    protocol: TestJavaProtocol,
    stream: &mut tokio::net::TcpStream,
    buffer: &mut BytesMut,
    codec: &MinecraftWireCodec,
) -> Result<(), RuntimeError> {
    write_packet(
        stream,
        codec,
        &creative_inventory_action(protocol, 36, 17, 1, 0),
    )
    .await?;
    let hotbar_update = read_until_set_slot(stream, codec, buffer, protocol, 0, 36, 16).await?;
    assert_eq!(
        decode_set_slot(protocol, &hotbar_update)?,
        (0, 36, Some((17, 1, 0)))
    );

    write_packet(stream, codec, &click_window(protocol, 36, 0, 1, None)).await?;
    let pickup_ack =
        read_until_confirm_transaction(stream, codec, buffer, protocol, 0, 1, 16).await?;
    assert_eq!(
        decode_confirm_transaction(protocol, &pickup_ack)?,
        (0, 1, true)
    );
    let hotbar_pickup = read_until_set_slot(stream, codec, buffer, protocol, 0, 36, 16).await?;
    assert_eq!(decode_set_slot(protocol, &hotbar_pickup)?, (0, 36, None));
    let cursor_pickup = read_until_set_slot(stream, codec, buffer, protocol, -1, -1, 16).await?;
    assert_eq!(
        decode_set_slot(protocol, &cursor_pickup)?,
        (-1, -1, Some((17, 1, 0)))
    );

    write_packet(
        stream,
        codec,
        &click_window(protocol, 1, 0, 2, Some((17, 1, 0))),
    )
    .await?;
    let place_ack =
        read_until_confirm_transaction(stream, codec, buffer, protocol, 0, 2, 16).await?;
    assert_eq!(
        decode_confirm_transaction(protocol, &place_ack)?,
        (0, 2, true)
    );
    let result_preview = read_until_set_slot(stream, codec, buffer, protocol, 0, 0, 16).await?;
    assert_eq!(
        decode_set_slot(protocol, &result_preview)?,
        (0, 0, Some((5, 4, 0)))
    );
    let craft_input = read_until_set_slot(stream, codec, buffer, protocol, 0, 1, 16).await?;
    assert_eq!(
        decode_set_slot(protocol, &craft_input)?,
        (0, 1, Some((17, 1, 0)))
    );
    let cursor_cleared = read_until_set_slot(stream, codec, buffer, protocol, -1, -1, 16).await?;
    assert_eq!(decode_set_slot(protocol, &cursor_cleared)?, (-1, -1, None));

    write_packet(stream, codec, &click_window(protocol, 0, 0, 3, None)).await?;
    let result_ack =
        read_until_confirm_transaction(stream, codec, buffer, protocol, 0, 3, 16).await?;
    assert_eq!(
        decode_confirm_transaction(protocol, &result_ack)?,
        (0, 3, true)
    );
    let result_taken = read_until_set_slot(stream, codec, buffer, protocol, 0, 0, 16).await?;
    assert_eq!(decode_set_slot(protocol, &result_taken)?, (0, 0, None));
    let input_consumed = read_until_set_slot(stream, codec, buffer, protocol, 0, 1, 16).await?;
    assert_eq!(decode_set_slot(protocol, &input_consumed)?, (0, 1, None));
    let cursor_result = read_until_set_slot(stream, codec, buffer, protocol, -1, -1, 16).await?;
    assert_eq!(
        decode_set_slot(protocol, &cursor_result)?,
        (-1, -1, Some((5, 4, 0)))
    );
    Ok(())
}

fn assert_java_set_slot(
    protocol: TestJavaProtocol,
    packet: &[u8],
    expected_slot: i16,
    expected_item: Option<(i16, u8, i16)>,
) -> Result<(), RuntimeError> {
    assert_eq!(
        decode_set_slot(protocol, packet)?,
        (0, expected_slot, expected_item)
    );
    Ok(())
}
