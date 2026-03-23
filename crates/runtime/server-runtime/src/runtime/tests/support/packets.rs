use super::*;

pub(crate) fn encode_handshake(
    protocol_version: i32,
    next_state: i32,
) -> Result<Vec<u8>, RuntimeError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    writer.write_varint(protocol_version);
    writer.write_string("localhost")?;
    writer.write_u16(25565);
    writer.write_varint(next_state);
    Ok(writer.into_inner())
}

pub(crate) fn login_start(username: &str) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    let _ = writer.write_string(username);
    writer.into_inner()
}

pub(crate) fn status_request() -> Vec<u8> {
    vec![0x00]
}

pub(crate) fn status_ping(value: i64) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x01);
    writer.write_i64(value);
    writer.into_inner()
}

pub(crate) fn raknet_unconnected_ping() -> Vec<u8> {
    let mut frame = Vec::with_capacity(33);
    frame.push(0x01);
    frame.extend_from_slice(&123_i64.to_be_bytes());
    frame.extend_from_slice(&[
        0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56,
        0x78,
    ]);
    frame.extend_from_slice(&456_i64.to_be_bytes());
    frame
}

pub(crate) fn player_position_look(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x06);
    writer.write_f64(x);
    writer.write_f64(y + 1.62);
    writer.write_f64(y);
    writer.write_f64(z);
    writer.write_f32(yaw);
    writer.write_f32(pitch);
    writer.write_bool(true);
    writer.into_inner()
}

pub(crate) fn held_item_change(slot: i16) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x09);
    writer.write_i16(slot);
    writer.into_inner()
}

pub(crate) fn creative_inventory_action(
    protocol: TestJavaProtocol,
    slot: i16,
    item_id: i16,
    count: u8,
    damage: i16,
) -> Vec<u8> {
    protocol.encode_creative_inventory_action(slot, item_id, count, damage)
}

pub(crate) fn click_window(
    protocol: TestJavaProtocol,
    slot: i16,
    button: i8,
    action_number: i16,
    clicked_item: Option<(i16, u8, i16)>,
) -> Vec<u8> {
    protocol.encode_click_window(slot, button, action_number, clicked_item)
}

pub(crate) fn confirm_transaction_ack(
    protocol: TestJavaProtocol,
    window_id: u8,
    action_number: i16,
    accepted: bool,
) -> Vec<u8> {
    protocol.encode_confirm_transaction_ack(window_id, action_number, accepted)
}

pub(crate) fn player_block_placement(
    x: i32,
    y: u8,
    z: i32,
    face: u8,
    held_item: Option<(i16, u8, i16)>,
) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x08);
    writer.write_i32(x);
    writer.write_u8(y);
    writer.write_i32(z);
    writer.write_u8(face);
    if let Some((item_id, count, damage)) = held_item {
        writer.write_i16(item_id);
        writer.write_u8(count);
        writer.write_i16(damage);
        writer.write_i16(-1);
    } else {
        writer.write_i16(-1);
    }
    writer.write_u8(8);
    writer.write_u8(8);
    writer.write_u8(8);
    writer.into_inner()
}

pub(crate) fn player_digging(status: u8, x: i32, y: u8, z: i32, face: u8) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x07);
    writer.write_u8(status);
    writer.write_i32(x);
    writer.write_u8(y);
    writer.write_i32(z);
    writer.write_u8(face);
    writer.into_inner()
}

pub(crate) fn player_position_look_1_8(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x06);
    writer.write_f64(x);
    writer.write_f64(y);
    writer.write_f64(z);
    writer.write_f32(yaw);
    writer.write_f32(pitch);
    writer.write_bool(true);
    writer.into_inner()
}

pub(crate) fn player_position_look_1_12(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x0e);
    writer.write_f64(x);
    writer.write_f64(y);
    writer.write_f64(z);
    writer.write_f32(yaw);
    writer.write_f32(pitch);
    writer.write_bool(true);
    writer.into_inner()
}

pub(crate) fn player_block_placement_1_12(x: i32, y: i32, z: i32, face: i32, hand: i32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x1f);
    writer.write_i64(pack_block_position(mc_core::BlockPos::new(x, y, z)));
    writer.write_varint(face);
    writer.write_varint(hand);
    writer.write_f32(0.5);
    writer.write_f32(0.5);
    writer.write_f32(0.5);
    writer.into_inner()
}

pub(crate) fn pack_block_position(position: mc_core::BlockPos) -> i64 {
    let x = i64::from(position.x) & 0x3ff_ffff;
    let y = i64::from(position.y) & 0xfff;
    let z = i64::from(position.z) & 0x3ff_ffff;
    (x << 38) | (y << 26) | z
}

pub(crate) fn unpack_block_position(packed: i64) -> mc_core::BlockPos {
    fn sign_extend(value: i64, bits: u8) -> i64 {
        let shift = 64 - i64::from(bits);
        (value << shift) >> shift
    }

    let x = sign_extend((packed >> 38) & 0x3ff_ffff, 26);
    let y = sign_extend((packed >> 26) & 0xfff, 12);
    let z = sign_extend(packed & 0x3ff_ffff, 26);
    mc_core::BlockPos::new(
        i32::try_from(x).expect("packed x should fit into i32"),
        i32::try_from(y).expect("packed y should fit into i32"),
        i32::try_from(z).expect("packed z should fit into i32"),
    )
}

pub(crate) fn window_items_slot(
    protocol: TestJavaProtocol,
    packet: &[u8],
    wanted_slot: usize,
) -> Result<Option<(i16, u8, i16)>, RuntimeError> {
    Ok(protocol.window_items_slot(packet, wanted_slot)?)
}

pub(crate) fn set_slot_slot(
    protocol: TestJavaProtocol,
    packet: &[u8],
) -> Result<i16, RuntimeError> {
    let (_, slot, _) = protocol.decode_set_slot(packet)?;
    Ok(slot)
}

pub(crate) fn decode_set_slot(
    protocol: TestJavaProtocol,
    packet: &[u8],
) -> Result<(i8, i16, Option<(i16, u8, i16)>), RuntimeError> {
    Ok(protocol.decode_set_slot(packet)?)
}

pub(crate) fn decode_confirm_transaction(
    protocol: TestJavaProtocol,
    packet: &[u8],
) -> Result<(u8, i16, bool), RuntimeError> {
    Ok(protocol.decode_confirm_transaction(packet)?)
}

pub(crate) fn held_item_from_packet(packet: &[u8]) -> Result<i8, RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x09 {
        return Err(RuntimeError::Config(
            "expected held item change packet".to_string(),
        ));
    }
    reader.read_i8().map_err(RuntimeError::from)
}

pub(crate) fn block_change_from_packet(
    packet: &[u8],
) -> Result<(i32, u8, i32, i32, u8), RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x23 {
        return Err(RuntimeError::Config(
            "expected block change packet".to_string(),
        ));
    }
    let x = reader.read_i32()?;
    let y = reader.read_u8()?;
    let z = reader.read_i32()?;
    let block_id = reader.read_varint()?;
    let metadata = reader.read_u8()?;
    Ok((x, y, z, block_id, metadata))
}

pub(crate) fn block_change_from_packet_1_8(
    packet: &[u8],
) -> Result<(i32, i32, i32, i32), RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x23 {
        return Err(RuntimeError::Config(
            "expected 1.8 block change packet".to_string(),
        ));
    }
    let position = unpack_block_position(reader.read_i64()?);
    let block_state = reader.read_varint()?;
    Ok((position.x, position.y, position.z, block_state))
}

pub(crate) fn player_abilities_flags(packet: &[u8]) -> Result<u8, RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x39 {
        return Err(RuntimeError::Config(
            "expected player abilities packet".to_string(),
        ));
    }
    reader.read_u8().map_err(RuntimeError::from)
}

pub(crate) fn packet_id(frame: &[u8]) -> i32 {
    let mut reader = PacketReader::new(frame);
    reader.read_varint().expect("packet id should decode")
}

pub(crate) fn login_encryption_response(
    shared_secret_encrypted: &[u8],
    verify_token_encrypted: &[u8],
) -> Result<Vec<u8>, RuntimeError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x01);
    writer.write_varint(
        i32::try_from(shared_secret_encrypted.len())
            .map_err(|_| RuntimeError::Config("encrypted shared secret too large".to_string()))?,
    );
    writer.write_bytes(shared_secret_encrypted);
    writer.write_varint(
        i32::try_from(verify_token_encrypted.len())
            .map_err(|_| RuntimeError::Config("encrypted verify token too large".to_string()))?,
    );
    writer.write_bytes(verify_token_encrypted);
    Ok(writer.into_inner())
}

pub(crate) fn parse_encryption_request(
    packet: &[u8],
) -> Result<(String, Vec<u8>, Vec<u8>), RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x01 {
        return Err(RuntimeError::Config(
            "expected login encryption request packet".to_string(),
        ));
    }
    let server_id = reader.read_string(20)?;
    let public_key_len = usize::try_from(reader.read_varint()?)
        .map_err(|_| RuntimeError::Config("negative public key length".to_string()))?;
    let public_key_der = reader.read_bytes(public_key_len)?.to_vec();
    let verify_token_len = usize::try_from(reader.read_varint()?)
        .map_err(|_| RuntimeError::Config("negative verify token length".to_string()))?;
    let verify_token = reader.read_bytes(verify_token_len)?.to_vec();
    Ok((server_id, public_key_der, verify_token))
}
