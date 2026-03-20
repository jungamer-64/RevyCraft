use crate::{
    PACKET_SB_CLIENT_COMMAND, PACKET_SB_CREATIVE_INVENTORY_ACTION, PACKET_SB_FLYING,
    PACKET_SB_HELD_ITEM_CHANGE, PACKET_SB_KEEP_ALIVE, PACKET_SB_LOOK,
    PACKET_SB_PLAYER_BLOCK_PLACEMENT, PACKET_SB_PLAYER_DIGGING, PACKET_SB_POSITION,
    PACKET_SB_POSITION_LOOK, PACKET_SB_SETTINGS, PACKET_SB_USE_ITEM,
};
use mc_core::{BlockFace, CoreCommand, InteractionHand, PlayerId, Vec3};
use mc_proto_common::{PacketReader, ProtocolError};
use mc_proto_je_common::{modern_inventory_slot, read_legacy_slot, unpack_block_position};

pub fn decode_play_packet(
    player_id: PlayerId,
    frame: &[u8],
) -> Result<Option<CoreCommand>, ProtocolError> {
    let mut reader = PacketReader::new(frame);
    let packet_id = reader.read_varint()?;
    match packet_id {
        PACKET_SB_KEEP_ALIVE => Ok(Some(CoreCommand::KeepAliveResponse {
            player_id,
            keep_alive_id: i32::try_from(reader.read_i64()?)
                .map_err(|_| ProtocolError::InvalidPacket("keepalive id out of range"))?,
        })),
        PACKET_SB_FLYING => Ok(Some(CoreCommand::MoveIntent {
            player_id,
            position: None,
            yaw: None,
            pitch: None,
            on_ground: reader.read_bool()?,
        })),
        PACKET_SB_POSITION => Ok(Some(decode_position_packet(player_id, &mut reader)?)),
        PACKET_SB_LOOK => Ok(Some(CoreCommand::MoveIntent {
            player_id,
            position: None,
            yaw: Some(reader.read_f32()?),
            pitch: Some(reader.read_f32()?),
            on_ground: reader.read_bool()?,
        })),
        PACKET_SB_POSITION_LOOK => Ok(Some(decode_position_look_packet(player_id, &mut reader)?)),
        PACKET_SB_PLAYER_DIGGING => Ok(Some(decode_digging_packet(player_id, &mut reader)?)),
        PACKET_SB_HELD_ITEM_CHANGE => Ok(Some(CoreCommand::SetHeldSlot {
            player_id,
            slot: reader.read_i16()?,
        })),
        PACKET_SB_CREATIVE_INVENTORY_ACTION => {
            let slot = reader.read_i16()?;
            let stack = read_legacy_slot(&mut reader)?;
            Ok(
                modern_inventory_slot(slot).map(|slot| CoreCommand::CreativeInventorySet {
                    player_id,
                    slot,
                    stack,
                }),
            )
        }
        PACKET_SB_PLAYER_BLOCK_PLACEMENT => decode_place_block_packet(player_id, &mut reader),
        PACKET_SB_USE_ITEM => {
            let _hand = decode_interaction_hand(reader.read_varint()?)?;
            Ok(None)
        }
        PACKET_SB_SETTINGS => Ok(Some(decode_client_settings_packet(player_id, &mut reader)?)),
        PACKET_SB_CLIENT_COMMAND => Ok(Some(CoreCommand::ClientStatus {
            player_id,
            action_id: i8::try_from(reader.read_varint()?)
                .map_err(|_| ProtocolError::InvalidPacket("client command out of range"))?,
        })),
        _ => Ok(None),
    }
}

pub fn read_login_byte_array(reader: &mut PacketReader<'_>) -> Result<Vec<u8>, ProtocolError> {
    let len = usize::try_from(reader.read_varint()?)
        .map_err(|_| ProtocolError::InvalidPacket("negative login byte array length"))?;
    Ok(reader.read_bytes(len)?.to_vec())
}

fn decode_position_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<CoreCommand, ProtocolError> {
    let x = reader.read_f64()?;
    let y = reader.read_f64()?;
    let z = reader.read_f64()?;
    let on_ground = reader.read_bool()?;
    Ok(CoreCommand::MoveIntent {
        player_id,
        position: Some(Vec3::new(x, y, z)),
        yaw: None,
        pitch: None,
        on_ground,
    })
}

fn decode_position_look_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<CoreCommand, ProtocolError> {
    let x = reader.read_f64()?;
    let y = reader.read_f64()?;
    let z = reader.read_f64()?;
    let yaw = reader.read_f32()?;
    let pitch = reader.read_f32()?;
    let on_ground = reader.read_bool()?;
    Ok(CoreCommand::MoveIntent {
        player_id,
        position: Some(Vec3::new(x, y, z)),
        yaw: Some(yaw),
        pitch: Some(pitch),
        on_ground,
    })
}

fn decode_digging_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<CoreCommand, ProtocolError> {
    Ok(CoreCommand::DigBlock {
        player_id,
        status: u8::try_from(reader.read_varint()?)
            .map_err(|_| ProtocolError::InvalidPacket("dig status out of range"))?,
        position: unpack_block_position(reader.read_i64()?),
        face: BlockFace::from_protocol_byte(reader.read_i8()?.to_be_bytes()[0]),
    })
}

fn decode_place_block_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<Option<CoreCommand>, ProtocolError> {
    let position = unpack_block_position(reader.read_i64()?);
    let face = u8::try_from(reader.read_varint()?)
        .map_err(|_| ProtocolError::InvalidPacket("face out of range"))?;
    let hand = decode_interaction_hand(reader.read_varint()?)?;
    let _cursor_x = reader.read_f32()?;
    let _cursor_y = reader.read_f32()?;
    let _cursor_z = reader.read_f32()?;
    Ok(Some(CoreCommand::PlaceBlock {
        player_id,
        hand,
        position,
        face: BlockFace::from_protocol_byte(face),
        held_item: None,
    }))
}

fn decode_client_settings_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<CoreCommand, ProtocolError> {
    let _locale = reader.read_string(16)?;
    let view_distance = i8_to_u8(reader.read_i8()?);
    let _chat_flags = reader.read_varint()?;
    let _chat_colors = reader.read_bool()?;
    let _skin_parts = reader.read_u8()?;
    let _main_hand = reader.read_varint()?;
    Ok(CoreCommand::UpdateClientView {
        player_id,
        view_distance: view_distance.max(1),
    })
}

const fn decode_interaction_hand(hand: i32) -> Result<InteractionHand, ProtocolError> {
    match hand {
        0 => Ok(InteractionHand::Main),
        1 => Ok(InteractionHand::Offhand),
        _ => Err(ProtocolError::InvalidPacket("invalid interaction hand")),
    }
}

const fn i8_to_u8(value: i8) -> u8 {
    if value.is_negative() {
        0
    } else {
        value.cast_unsigned()
    }
}
