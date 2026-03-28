use crate::{
    PACKET_SB_CLICK_WINDOW, PACKET_SB_CLIENT_COMMAND, PACKET_SB_CLOSE_WINDOW,
    PACKET_SB_CONFIRM_TRANSACTION, PACKET_SB_CREATIVE_INVENTORY_ACTION, PACKET_SB_FLYING,
    PACKET_SB_HELD_ITEM_CHANGE, PACKET_SB_KEEP_ALIVE, PACKET_SB_LOOK,
    PACKET_SB_PLAYER_BLOCK_PLACEMENT, PACKET_SB_PLAYER_DIGGING, PACKET_SB_POSITION,
    PACKET_SB_POSITION_LOOK, PACKET_SB_SETTINGS,
};
use mc_core::{
    BlockFace, CoreCommand, InteractionHand, InventoryClickButton, InventoryClickTarget,
    InventoryClickValidation, InventoryTransactionContext, PlayerId, RuntimeCommand,
    SessionCommand, Vec3,
};
use mc_proto_common::{PacketReader, ProtocolError, ProtocolSessionSnapshot};
use mc_proto_je_common::__version_support::{
    inventory::{inventory_slot, read_slot},
    positions::unpack_block_position,
};
use mc_proto_je_common::JavaProtocolSessionStore;

pub(crate) fn decode_play_packet(
    session: &ProtocolSessionSnapshot,
    sessions: &JavaProtocolSessionStore,
    frame: &[u8],
) -> Result<Option<RuntimeCommand>, ProtocolError> {
    let player_id = session.player_id.ok_or(ProtocolError::InvalidPacket(
        "play session is missing player id",
    ))?;
    let mut reader = PacketReader::new(frame);
    let packet_id = reader.read_varint()?;
    match packet_id {
        PACKET_SB_KEEP_ALIVE => Ok(Some(RuntimeCommand::Core(CoreCommand::KeepAliveResponse {
            player_id,
            keep_alive_id: reader.read_varint()?,
        }))),
        PACKET_SB_FLYING => Ok(Some(RuntimeCommand::Core(CoreCommand::MoveIntent {
            player_id,
            position: None,
            yaw: None,
            pitch: None,
            on_ground: reader.read_bool()?,
        }))),
        PACKET_SB_POSITION => Ok(Some(RuntimeCommand::Core(decode_position_packet(
            player_id,
            &mut reader,
        )?))),
        PACKET_SB_LOOK => Ok(Some(RuntimeCommand::Core(CoreCommand::MoveIntent {
            player_id,
            position: None,
            yaw: Some(reader.read_f32()?),
            pitch: Some(reader.read_f32()?),
            on_ground: reader.read_bool()?,
        }))),
        PACKET_SB_POSITION_LOOK => Ok(Some(RuntimeCommand::Core(decode_position_look_packet(
            player_id,
            &mut reader,
        )?))),
        PACKET_SB_PLAYER_DIGGING => Ok(Some(RuntimeCommand::Core(decode_digging_packet(
            player_id,
            &mut reader,
        )?))),
        PACKET_SB_PLAYER_BLOCK_PLACEMENT => decode_place_block_packet(player_id, &mut reader),
        PACKET_SB_HELD_ITEM_CHANGE => Ok(Some(RuntimeCommand::Core(CoreCommand::SetHeldSlot {
            player_id,
            slot: reader.read_i16()?,
        }))),
        PACKET_SB_CONFIRM_TRANSACTION => Ok(Some(decode_confirm_transaction_packet(
            session,
            sessions,
            player_id,
            &mut reader,
        )?)),
        PACKET_SB_CLOSE_WINDOW => Ok(Some(RuntimeCommand::Core(CoreCommand::CloseContainer {
            player_id,
            window_id: reader.read_u8()?,
        }))),
        PACKET_SB_CLICK_WINDOW => {
            decode_click_window_packet(session, sessions, player_id, &mut reader)
        }
        PACKET_SB_CREATIVE_INVENTORY_ACTION => {
            let slot = reader.read_i16()?;
            let stack = read_slot(&mut reader, crate::INVENTORY_SPEC.slot)?;
            Ok(inventory_slot(
                mc_core::InventoryContainer::Player,
                crate::INVENTORY_SPEC.layout,
                slot,
            )
            .map(|slot| {
                RuntimeCommand::Core(CoreCommand::CreativeInventorySet {
                    player_id,
                    slot,
                    stack,
                })
            }))
        }
        PACKET_SB_SETTINGS => Ok(Some(RuntimeCommand::Core(decode_client_settings_packet(
            player_id,
            &mut reader,
        )?))),
        PACKET_SB_CLIENT_COMMAND => Ok(Some(RuntimeCommand::Session(
            SessionCommand::ClientStatus {
                player_id,
                action_id: i8::try_from(reader.read_varint()?)
                    .map_err(|_| ProtocolError::InvalidPacket("client command out of range"))?,
            },
        ))),
        _ => Ok(None),
    }
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
) -> Result<Option<RuntimeCommand>, ProtocolError> {
    let position = unpack_block_position(reader.read_i64()?);
    let direction = reader.read_i8()?;
    let held_item = read_slot(reader, crate::INVENTORY_SPEC.slot)?;
    let _cursor_x = reader.read_i8()?;
    let _cursor_y = reader.read_i8()?;
    let _cursor_z = reader.read_i8()?;
    if position.x == -1 && position.z == -1 && position.y == 255 && direction == -1 {
        return Ok(None);
    }
    Ok(Some(RuntimeCommand::Core(CoreCommand::UseBlock {
        player_id,
        hand: InteractionHand::Main,
        position,
        face: u8::try_from(direction)
            .ok()
            .and_then(BlockFace::from_protocol_byte),
        held_item,
    })))
}

fn decode_client_settings_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<CoreCommand, ProtocolError> {
    let _locale = reader.read_string(16)?;
    let view_distance = i8_to_u8(reader.read_i8()?);
    let _chat_flags = reader.read_i8()?;
    let _chat_colors = reader.read_bool()?;
    let _skin_parts = reader.read_u8()?;
    Ok(CoreCommand::UpdateClientView {
        player_id,
        view_distance: view_distance.max(1),
    })
}

const fn i8_to_u8(value: i8) -> u8 {
    if value.is_negative() {
        0
    } else {
        value.cast_unsigned()
    }
}

fn decode_click_window_packet(
    session: &ProtocolSessionSnapshot,
    sessions: &JavaProtocolSessionStore,
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<Option<RuntimeCommand>, ProtocolError> {
    let window_id = reader.read_u8()?;
    let raw_slot = reader.read_i16()?;
    let raw_button = reader.read_i8()?;
    let action_number = reader.read_i16()?;
    let mode = reader.read_i8()?;
    let clicked_item = read_slot(reader, crate::INVENTORY_SPEC.slot)?;

    let button = match raw_button {
        1 => InventoryClickButton::Right,
        _ => InventoryClickButton::Left,
    };
    let target = if mode != 0 || !matches!(raw_button, 0 | 1) {
        InventoryClickTarget::Unsupported
    } else if raw_slot == -999 {
        InventoryClickTarget::Outside
    } else {
        sessions
            .resolve_container(session, window_id)
            .and_then(|container| inventory_slot(container, crate::INVENTORY_SPEC.layout, raw_slot))
            .map(InventoryClickTarget::Slot)
            .unwrap_or(InventoryClickTarget::Unsupported)
    };

    let transaction = InventoryTransactionContext {
        window_id,
        action_number,
    };
    if sessions.should_gate_click(session, transaction) {
        return Ok(None);
    }

    Ok(Some(RuntimeCommand::Core(CoreCommand::InventoryClick {
        player_id,
        transaction,
        target,
        button,
        validation: InventoryClickValidation::StrictSlotEcho { clicked_item },
    })))
}

fn decode_confirm_transaction_packet(
    session: &ProtocolSessionSnapshot,
    sessions: &JavaProtocolSessionStore,
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<RuntimeCommand, ProtocolError> {
    let transaction = InventoryTransactionContext {
        window_id: reader.read_u8()?,
        action_number: reader.read_i16()?,
    };
    let accepted = reader.read_bool()?;
    sessions.observe_transaction_ack(session, transaction, accepted);
    Ok(RuntimeCommand::Session(
        SessionCommand::InventoryTransactionAck {
            player_id,
            transaction,
            accepted,
        },
    ))
}
