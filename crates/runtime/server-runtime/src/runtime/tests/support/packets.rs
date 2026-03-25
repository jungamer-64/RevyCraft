use super::*;
use base64::Engine;
use bedrockrs_proto::V924;
use bedrockrs_proto::codec::{decode_packets, encode_packets};
use bedrockrs_proto::compression::Compression as BedrockCompression;
use bedrockrs_proto::info::RAKNET_GAMEPACKET_ID;
use bedrockrs_proto::v662::enums::{
    InputMode, ItemUseInventoryTransactionType, NewInteractionModel, PlayerActionType,
};
use bedrockrs_proto::v662::packets::{
    LoginPacket, PlayerActionPacket, RequestNetworkSettingsPacket,
};
use bedrockrs_proto::v662::types::{
    ActorRuntimeID, NetworkBlockPosition, NetworkItemStackDescriptor,
};
use bedrockrs_proto::v712::types::{
    PackedItemUseLegacyInventoryTransaction, PredictedResult, TriggerType,
};
use bedrockrs_proto::v766::packets::ClientPlayMode;
use bedrockrs_proto::v766::packets::PlayerAuthInputPacket;
use bedrockrs_proto::v766::packets::player_auth_input_packet::PlayerAuthInputFlags;
use bedrockrs_proto_core::{PacketHeader, ProtoCodec, ProtoCodecLE, ProtoCodecVAR};
use mc_proto_be_924::BE_924_PROTOCOL_NUMBER;
use serde_json::json;
use std::io::Cursor;
use vek::{Vec2, Vec3};

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

pub(crate) fn held_item_change_for_protocol(protocol: TestJavaProtocol, slot: i16) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(match protocol {
        TestJavaProtocol::Je5 | TestJavaProtocol::Je47 => 0x09,
        TestJavaProtocol::Je340 => 0x1a,
    });
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

pub(crate) fn click_window_in_window(
    protocol: TestJavaProtocol,
    window_id: i8,
    slot: i16,
    button: i8,
    action_number: i16,
    clicked_item: Option<(i16, u8, i16)>,
) -> Vec<u8> {
    protocol.encode_click_window_in_window(window_id, slot, button, action_number, clicked_item)
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

pub(crate) fn player_digging_1_12(status: i32, x: i32, y: i32, z: i32, face: u8) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x14);
    writer.write_varint(status);
    writer.write_i64(pack_block_position(mc_core::BlockPos::new(x, y, z)));
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

pub(crate) fn decode_open_window(
    protocol: TestJavaProtocol,
    packet: &[u8],
) -> Result<(u8, String, String, u8, Option<bool>), RuntimeError> {
    Ok(protocol.decode_open_window(packet)?)
}

pub(crate) fn decode_window_property(
    protocol: TestJavaProtocol,
    packet: &[u8],
) -> Result<(u8, i16, i16), RuntimeError> {
    Ok(protocol.decode_window_property(packet)?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestBedrockPacket {
    NetworkSettings,
    PlayStatus,
    StartGame,
    LevelChunk,
    UpdateBlock,
    LevelEvent,
    AddItemActor,
    RemoveActor,
    InventoryContent,
    InventorySlot,
    PlayerHotbar,
    ContainerOpen,
    ContainerClose,
    ContainerSetData,
}

pub(crate) fn bedrock_network_settings_request() -> Result<Vec<u8>, RuntimeError> {
    encode_packets(
        &[V924::RequestNetworkSettingsPacket(
            RequestNetworkSettingsPacket {
                client_network_version: BE_924_PROTOCOL_NUMBER,
            },
        )],
        None,
        None,
    )
    .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn bedrock_login_packet(
    username: &str,
    compression: Option<&BedrockCompression>,
) -> Result<Vec<u8>, RuntimeError> {
    let chain_entry = bedrock_test_jwt(&json!({"extraData":{"displayName":username}}));
    let chain = json!({ "chain": [chain_entry] }).to_string();
    let client_jwt = bedrock_test_jwt(&json!({"DisplayName":username}));
    let mut connection_request = Vec::new();
    let chain_len = u32::try_from(chain.len()).expect("test chain jwt should fit in u32");
    connection_request.extend_from_slice(&chain_len.to_le_bytes());
    connection_request.extend_from_slice(chain.as_bytes());
    let client_jwt_len =
        u32::try_from(client_jwt.len()).expect("test client jwt should fit in u32");
    connection_request.extend_from_slice(&client_jwt_len.to_le_bytes());
    connection_request.extend_from_slice(client_jwt.as_bytes());

    encode_packets(
        &[V924::LoginPacket(LoginPacket {
            client_network_version: BE_924_PROTOCOL_NUMBER,
            connection_request,
        })],
        compression,
        None,
    )
    .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn bedrock_place_block_payload(
    position: mc_core::BlockPos,
    face: i32,
) -> Result<Vec<u8>, RuntimeError> {
    bedrock_block_interaction_payload(ItemUseInventoryTransactionType::Place, position, face)
}

pub(crate) fn bedrock_break_block_payload(
    position: mc_core::BlockPos,
    face: i32,
) -> Result<Vec<u8>, RuntimeError> {
    bedrock_block_interaction_payload(ItemUseInventoryTransactionType::Destroy, position, face)
}

pub(crate) fn bedrock_start_break_block_payload(
    position: mc_core::BlockPos,
    face: i32,
) -> Result<Vec<u8>, RuntimeError> {
    bedrock_player_action_payload(PlayerActionType::StartDestroyBlock, position, face)
}

pub(crate) fn bedrock_abort_break_block_payload(
    position: mc_core::BlockPos,
    face: i32,
) -> Result<Vec<u8>, RuntimeError> {
    bedrock_player_action_payload(PlayerActionType::AbortDestroyBlock, position, face)
}

pub(crate) fn decode_bedrock_packets(
    payload: &[u8],
    compression: Option<&BedrockCompression>,
) -> Result<Vec<V924>, RuntimeError> {
    let payload = match payload.first().copied() {
        Some(RAKNET_GAMEPACKET_ID) => &payload[1..],
        _ => payload,
    };
    decode_packets::<V924>(payload.to_vec(), compression, None)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn bedrock_transport_payload(payload: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(payload.len() + 1);
    framed.push(RAKNET_GAMEPACKET_ID);
    framed.extend_from_slice(payload);
    framed
}

pub(crate) fn test_bedrock_packet(packet: &V924) -> Option<TestBedrockPacket> {
    Some(match packet {
        V924::NetworkSettingsPacket(_) => TestBedrockPacket::NetworkSettings,
        V924::PlayStatusPacket(_) => TestBedrockPacket::PlayStatus,
        V924::StartGamePacket(_) => TestBedrockPacket::StartGame,
        V924::LevelChunkPacket(_) => TestBedrockPacket::LevelChunk,
        V924::UpdateBlockPacket(_) => TestBedrockPacket::UpdateBlock,
        V924::LevelEventPacket(_) => TestBedrockPacket::LevelEvent,
        V924::AddItemActorPacket(_) => TestBedrockPacket::AddItemActor,
        V924::RemoveActorPacket(_) => TestBedrockPacket::RemoveActor,
        V924::InventoryContentPacket(_) => TestBedrockPacket::InventoryContent,
        V924::InventorySlotPacket(_) => TestBedrockPacket::InventorySlot,
        V924::PlayerHotbarPacket(_) => TestBedrockPacket::PlayerHotbar,
        V924::ContainerOpenPacket(_) => TestBedrockPacket::ContainerOpen,
        V924::ContainerClosePacket(_) => TestBedrockPacket::ContainerClose,
        V924::ContainerSetDataPacket(_) => TestBedrockPacket::ContainerSetData,
        _ => return None,
    })
}

fn bedrock_test_jwt(payload: &serde_json::Value) -> String {
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
    let payload =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
    format!("{header}.{payload}.")
}

pub(crate) fn decode_close_window(
    protocol: TestJavaProtocol,
    packet: &[u8],
) -> Result<u8, RuntimeError> {
    Ok(protocol.decode_close_window(packet)?)
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

pub(crate) fn held_item_from_packet_for_protocol(
    protocol: TestJavaProtocol,
    packet: &[u8],
) -> Result<i8, RuntimeError> {
    let mut reader = PacketReader::new(packet);
    let expected_packet_id = protocol
        .clientbound_packet_id(TestJavaPacket::HeldItemChange)
        .ok_or_else(|| RuntimeError::Config("held item change packet is unsupported".into()))?;
    if reader.read_varint()? != expected_packet_id {
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

pub(crate) fn block_change_from_packet_1_12(
    packet: &[u8],
) -> Result<(i32, i32, i32, i32), RuntimeError> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint()? != 0x0b {
        return Err(RuntimeError::Config(
            "expected 1.12.2 block change packet".to_string(),
        ));
    }
    let position = unpack_block_position(reader.read_i64()?);
    let block_state = reader.read_varint()?;
    Ok((position.x, position.y, position.z, block_state))
}

pub(crate) fn block_break_animation_from_packet(
    protocol: TestJavaProtocol,
    packet: &[u8],
) -> Result<(i32, i32, i32, i32, i8), RuntimeError> {
    protocol
        .decode_block_break_animation(packet)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn bedrock_level_event_from_packet(
    packet: &V924,
) -> Result<(i32, f32, f32, f32, i32), RuntimeError> {
    match packet {
        V924::LevelEventPacket(packet) => Ok((
            packet.event_id,
            packet.position.x,
            packet.position.y,
            packet.position.z,
            packet.data,
        )),
        other => Err(RuntimeError::Config(format!(
            "expected level event packet, got {other:?}"
        ))),
    }
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

fn bedrock_block_interaction_payload(
    action_type: ItemUseInventoryTransactionType,
    position: mc_core::BlockPos,
    face: i32,
) -> Result<Vec<u8>, RuntimeError> {
    encode_bedrock_player_auth_input(PlayerAuthInputPacket {
        player_rotation: Vec2::new(0.0, 0.0),
        player_position: Vec3::new(0.0, 0.0, 0.0),
        move_vector: Vec3::new(0.0, 0.0, 0.0),
        player_head_rotation: 0.0,
        input_data: PlayerAuthInputFlags::PerformItemInteraction as u128,
        input_mode: InputMode::Mouse,
        play_mode: ClientPlayMode::Normal,
        new_interaction_model: NewInteractionModel::Crosshair,
        interact_rotation: Vec3::new(0.0, 0.0, 0.0),
        client_tick: 0,
        velocity: Vec3::new(0.0, 0.0, 0.0),
        item_use_transaction: Some(PackedItemUseLegacyInventoryTransaction {
            id: 0,
            container_slots: None,
            action: bedrockrs_proto::v662::types::InventoryTransaction { action: Vec::new() },
            action_type,
            trigger_type: TriggerType::PlayerInput,
            position: NetworkBlockPosition {
                x: position.x,
                y: u32::try_from(position.y)
                    .expect("test bedrock block interaction y should fit into u32"),
                z: position.z,
            },
            face,
            slot: 0,
            item: empty_bedrock_item_stack_descriptor()?,
            from_position: Vec3::new(0.0, 0.0, 0.0),
            click_position: Vec3::new(0.5, 0.5, 0.5),
            target_block_id: 0,
            predicted_result: PredictedResult::Success,
        }),
        item_stack_request: None,
        player_block_actions: None,
        client_predicted_vehicle: None,
        analog_move_vector: Vec2::new(0.0, 0.0),
        camera_orientation: Vec3::new(0.0, 0.0, 0.0),
        raw_move_vector: Vec2::new(0.0, 0.0),
    })
}

fn bedrock_player_action_payload(
    action: PlayerActionType,
    position: mc_core::BlockPos,
    face: i32,
) -> Result<Vec<u8>, RuntimeError> {
    encode_packets(
        &[V924::PlayerActionPacket(PlayerActionPacket {
            player_runtime_id: ActorRuntimeID(1),
            action,
            block_position: NetworkBlockPosition {
                x: position.x,
                y: u32::try_from(position.y)
                    .expect("test bedrock player action y should fit into u32"),
                z: position.z,
            },
            result_pos: NetworkBlockPosition {
                x: position.x,
                y: u32::try_from(position.y)
                    .expect("test bedrock player action result y should fit into u32"),
                z: position.z,
            },
            face,
        })],
        None,
        None,
    )
    .map_err(|error| RuntimeError::Config(error.to_string()))
}

fn empty_bedrock_item_stack_descriptor() -> Result<NetworkItemStackDescriptor, RuntimeError> {
    let mut bytes = Vec::new();
    <i32 as ProtoCodecVAR>::serialize(&0, &mut bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    NetworkItemStackDescriptor::deserialize(&mut Cursor::new(bytes))
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn bedrock_stack_descriptor_summary(
    descriptor: &NetworkItemStackDescriptor,
) -> Result<(i32, u16, u32), RuntimeError> {
    let mut bytes = Vec::new();
    descriptor
        .serialize(&mut bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut cursor = Cursor::new(bytes);
    let item_id = <i32 as ProtoCodecVAR>::deserialize(&mut cursor)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    if item_id == 0 {
        return Ok((0, 0, 0));
    }
    let count = <u16 as ProtoCodecLE>::deserialize(&mut cursor)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let aux = <u32 as ProtoCodecVAR>::deserialize(&mut cursor)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    Ok((item_id, count, aux))
}

fn encode_bedrock_player_auth_input(
    packet: PlayerAuthInputPacket<V924>,
) -> Result<Vec<u8>, RuntimeError> {
    let mut body = Vec::new();
    PacketHeader {
        packet_id: 144,
        sender_sub_client_id: 0,
        target_sub_client_id: 0,
    }
    .serialize(&mut body)
    .map_err(|error| RuntimeError::Config(error.to_string()))?;
    packet
        .serialize(&mut body)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    <Vec2<f32> as ProtoCodecLE>::serialize(&packet.raw_move_vector, &mut body)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let mut frame = Vec::new();
    <u32 as ProtoCodecVAR>::serialize(
        &u32::try_from(body.len()).expect("test bedrock payload length should fit into u32"),
        &mut frame,
    )
    .map_err(|error| RuntimeError::Config(error.to_string()))?;
    frame.extend_from_slice(&body);
    Ok(frame)
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
