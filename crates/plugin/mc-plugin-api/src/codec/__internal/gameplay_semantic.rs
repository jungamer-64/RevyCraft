use crate::codec::__internal::binary::{Decoder, Encoder, ProtocolCodecError};
use crate::codec::__internal::inventory::{
    decode_inventory_slot, decode_item_stack, decode_player_inventory, encode_inventory_slot,
    encode_item_stack, encode_player_inventory,
};
use crate::codec::__internal::shared::{
    decode_block_pos, decode_block_state, decode_capability_announcement, decode_connection_phase,
    decode_core_command, decode_core_event, decode_entity_id, decode_f32_value, decode_option,
    decode_player_id, decode_player_snapshot, decode_u8_value, decode_vec3, encode_block_pos,
    encode_block_state, encode_capability_announcement, encode_connection_phase,
    encode_core_command, encode_core_event, encode_entity_id, encode_option, encode_player_id,
    encode_player_snapshot, encode_vec3,
};
use crate::codec::gameplay::{
    GameplayDescriptor, GameplayOpCode, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
};
use mc_core::{
    EventTarget, GameplayEffect, GameplayJoinEffect, GameplayMutation, GameplayProfileId,
    PlayerInventory, TargetedEvent,
};

pub(crate) fn encode_gameplay_request_payload(
    encoder: &mut Encoder,
    request: &GameplayRequest,
) -> Result<(), ProtocolCodecError> {
    match request {
        GameplayRequest::Describe | GameplayRequest::CapabilitySet => Ok(()),
        GameplayRequest::HandlePlayerJoin { session, player } => {
            encode_gameplay_session_snapshot(encoder, session)?;
            encode_player_snapshot(encoder, player)
        }
        GameplayRequest::HandleCommand { session, command } => {
            encode_gameplay_session_snapshot(encoder, session)?;
            encode_core_command(encoder, command)
        }
        GameplayRequest::HandleTick { session, now_ms } => {
            encode_gameplay_session_snapshot(encoder, session)?;
            encoder.write_u64(*now_ms);
            Ok(())
        }
        GameplayRequest::SessionClosed { session }
        | GameplayRequest::ExportSessionState { session } => {
            encode_gameplay_session_snapshot(encoder, session)
        }
        GameplayRequest::ImportSessionState { session, blob } => {
            encode_gameplay_session_snapshot(encoder, session)?;
            encoder.write_bytes(blob)
        }
    }
}

pub(crate) fn decode_gameplay_request_payload(
    decoder: &mut Decoder<'_>,
    op_code: GameplayOpCode,
) -> Result<GameplayRequest, ProtocolCodecError> {
    match op_code {
        GameplayOpCode::Describe => Ok(GameplayRequest::Describe),
        GameplayOpCode::CapabilitySet => Ok(GameplayRequest::CapabilitySet),
        GameplayOpCode::HandlePlayerJoin => Ok(GameplayRequest::HandlePlayerJoin {
            session: decode_gameplay_session_snapshot(decoder)?,
            player: decode_player_snapshot(decoder)?,
        }),
        GameplayOpCode::HandleCommand => Ok(GameplayRequest::HandleCommand {
            session: decode_gameplay_session_snapshot(decoder)?,
            command: decode_core_command(decoder)?,
        }),
        GameplayOpCode::HandleTick => Ok(GameplayRequest::HandleTick {
            session: decode_gameplay_session_snapshot(decoder)?,
            now_ms: decoder.read_u64()?,
        }),
        GameplayOpCode::SessionClosed => Ok(GameplayRequest::SessionClosed {
            session: decode_gameplay_session_snapshot(decoder)?,
        }),
        GameplayOpCode::ExportSessionState => Ok(GameplayRequest::ExportSessionState {
            session: decode_gameplay_session_snapshot(decoder)?,
        }),
        GameplayOpCode::ImportSessionState => Ok(GameplayRequest::ImportSessionState {
            session: decode_gameplay_session_snapshot(decoder)?,
            blob: decoder.read_bytes()?,
        }),
    }
}

pub(crate) fn encode_gameplay_response_payload(
    encoder: &mut Encoder,
    op_code: GameplayOpCode,
    response: &GameplayResponse,
) -> Result<(), ProtocolCodecError> {
    match (op_code, response) {
        (GameplayOpCode::Describe, GameplayResponse::Descriptor(descriptor)) => {
            encode_gameplay_descriptor(encoder, descriptor)
        }
        (GameplayOpCode::CapabilitySet, GameplayResponse::CapabilitySet(capability_set)) => {
            encode_capability_announcement(encoder, capability_set)
        }
        (GameplayOpCode::HandlePlayerJoin, GameplayResponse::JoinEffect(effect)) => {
            encode_gameplay_join_effect(encoder, effect)
        }
        (
            GameplayOpCode::HandleCommand | GameplayOpCode::HandleTick,
            GameplayResponse::Effect(effect),
        ) => encode_gameplay_effect(encoder, effect),
        (
            GameplayOpCode::SessionClosed | GameplayOpCode::ImportSessionState,
            GameplayResponse::Empty,
        ) => Ok(()),
        (GameplayOpCode::ExportSessionState, GameplayResponse::SessionTransferBlob(blob)) => {
            encoder.write_bytes(blob)
        }
        _ => Err(ProtocolCodecError::InvalidValue(
            "unexpected gameplay response payload",
        )),
    }
}

pub(crate) fn decode_gameplay_response_payload(
    decoder: &mut Decoder<'_>,
    op_code: GameplayOpCode,
) -> Result<GameplayResponse, ProtocolCodecError> {
    match op_code {
        GameplayOpCode::Describe => Ok(GameplayResponse::Descriptor(decode_gameplay_descriptor(
            decoder,
        )?)),
        GameplayOpCode::CapabilitySet => Ok(GameplayResponse::CapabilitySet(
            decode_capability_announcement(decoder)?,
        )),
        GameplayOpCode::HandlePlayerJoin => Ok(GameplayResponse::JoinEffect(
            decode_gameplay_join_effect(decoder)?,
        )),
        GameplayOpCode::HandleCommand | GameplayOpCode::HandleTick => {
            Ok(GameplayResponse::Effect(decode_gameplay_effect(decoder)?))
        }
        GameplayOpCode::SessionClosed | GameplayOpCode::ImportSessionState => {
            Ok(GameplayResponse::Empty)
        }
        GameplayOpCode::ExportSessionState => {
            Ok(GameplayResponse::SessionTransferBlob(decoder.read_bytes()?))
        }
    }
}

pub(crate) fn encode_gameplay_descriptor(
    encoder: &mut Encoder,
    descriptor: &GameplayDescriptor,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(descriptor.profile.as_str())
}

pub(crate) fn decode_gameplay_descriptor(
    decoder: &mut Decoder<'_>,
) -> Result<GameplayDescriptor, ProtocolCodecError> {
    Ok(GameplayDescriptor {
        profile: GameplayProfileId::new(decoder.read_string()?),
    })
}

pub(crate) fn encode_gameplay_session_snapshot(
    encoder: &mut Encoder,
    snapshot: &GameplaySessionSnapshot,
) -> Result<(), ProtocolCodecError> {
    encode_connection_phase(encoder, snapshot.phase);
    encode_option(
        encoder,
        snapshot.player_id.as_ref(),
        |encoder, player_id| {
            encode_player_id(encoder, *player_id);
            Ok(())
        },
    )?;
    encode_option(
        encoder,
        snapshot.entity_id.as_ref(),
        |encoder, entity_id| {
            encode_entity_id(encoder, *entity_id);
            Ok(())
        },
    )?;
    encoder.write_string(snapshot.gameplay_profile.as_str())
}

pub(crate) fn decode_gameplay_session_snapshot(
    decoder: &mut Decoder<'_>,
) -> Result<GameplaySessionSnapshot, ProtocolCodecError> {
    Ok(GameplaySessionSnapshot {
        phase: decode_connection_phase(decoder)?,
        player_id: decode_option(decoder, decode_player_id)?,
        entity_id: decode_option(decoder, decode_entity_id)?,
        gameplay_profile: GameplayProfileId::new(decoder.read_string()?),
    })
}

fn encode_gameplay_join_effect(
    encoder: &mut Encoder,
    effect: &GameplayJoinEffect,
) -> Result<(), ProtocolCodecError> {
    encode_option(
        encoder,
        effect.inventory.as_ref(),
        encode_player_inventory_blob_inner,
    )?;
    encode_option(
        encoder,
        effect.selected_hotbar_slot.as_ref(),
        |encoder, slot| {
            encoder.write_u8(*slot);
            Ok(())
        },
    )?;
    encode_targeted_events(encoder, &effect.emitted_events)
}

fn decode_gameplay_join_effect(
    decoder: &mut Decoder<'_>,
) -> Result<GameplayJoinEffect, ProtocolCodecError> {
    Ok(GameplayJoinEffect {
        inventory: decode_option(decoder, decode_player_inventory_blob_inner)?,
        selected_hotbar_slot: decode_option(decoder, decode_u8_value)?,
        emitted_events: decode_targeted_events(decoder)?,
    })
}

fn encode_gameplay_effect(
    encoder: &mut Encoder,
    effect: &GameplayEffect,
) -> Result<(), ProtocolCodecError> {
    encoder.write_len(effect.mutations.len())?;
    for mutation in &effect.mutations {
        encode_gameplay_mutation(encoder, mutation)?;
    }
    encode_targeted_events(encoder, &effect.emitted_events)
}

fn decode_gameplay_effect(decoder: &mut Decoder<'_>) -> Result<GameplayEffect, ProtocolCodecError> {
    let len = decoder.read_len()?;
    let mut mutations = Vec::with_capacity(len);
    for _ in 0..len {
        mutations.push(decode_gameplay_mutation(decoder)?);
    }
    Ok(GameplayEffect {
        mutations,
        emitted_events: decode_targeted_events(decoder)?,
    })
}

fn encode_gameplay_mutation(
    encoder: &mut Encoder,
    mutation: &GameplayMutation,
) -> Result<(), ProtocolCodecError> {
    match mutation {
        GameplayMutation::PlayerPose {
            player_id,
            position,
            yaw,
            pitch,
            on_ground,
        } => {
            encoder.write_u8(1);
            encode_player_id(encoder, *player_id);
            encode_option(encoder, position.as_ref(), |encoder, position| {
                encoder.write_f64(position.x);
                encoder.write_f64(position.y);
                encoder.write_f64(position.z);
                Ok(())
            })?;
            encode_option(encoder, yaw.as_ref(), |encoder, yaw| {
                encoder.write_f32(*yaw);
                Ok(())
            })?;
            encode_option(encoder, pitch.as_ref(), |encoder, pitch| {
                encoder.write_f32(*pitch);
                Ok(())
            })?;
            encoder.write_bool(*on_ground);
            Ok(())
        }
        GameplayMutation::SelectedHotbarSlot { player_id, slot } => {
            encoder.write_u8(2);
            encode_player_id(encoder, *player_id);
            encoder.write_u8(*slot);
            Ok(())
        }
        GameplayMutation::InventorySlot {
            player_id,
            slot,
            stack,
        } => {
            encoder.write_u8(3);
            encode_player_id(encoder, *player_id);
            encode_inventory_slot(encoder, *slot);
            encode_option(encoder, stack.as_ref(), encode_item_stack)
        }
        GameplayMutation::ClearMining { player_id } => {
            encoder.write_u8(7);
            encode_player_id(encoder, *player_id);
            Ok(())
        }
        GameplayMutation::BeginMining {
            player_id,
            position,
            duration_ms,
        } => {
            encoder.write_u8(8);
            encode_player_id(encoder, *player_id);
            encode_block_pos(encoder, *position);
            encoder.write_u64(*duration_ms);
            Ok(())
        }
        GameplayMutation::Block { position, block } => {
            encoder.write_u8(4);
            encode_block_pos(encoder, *position);
            encode_block_state(encoder, block)
        }
        GameplayMutation::OpenChest {
            player_id,
            position,
        } => {
            encoder.write_u8(5);
            encode_player_id(encoder, *player_id);
            encode_block_pos(encoder, *position);
            Ok(())
        }
        GameplayMutation::DroppedItem { position, item } => {
            encoder.write_u8(6);
            encode_vec3(encoder, *position);
            encode_item_stack(encoder, item)
        }
    }
}

fn decode_gameplay_mutation(
    decoder: &mut Decoder<'_>,
) -> Result<GameplayMutation, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(GameplayMutation::PlayerPose {
            player_id: decode_player_id(decoder)?,
            position: decode_option(decoder, |decoder| {
                Ok(mc_core::Vec3::new(
                    decoder.read_f64()?,
                    decoder.read_f64()?,
                    decoder.read_f64()?,
                ))
            })?,
            yaw: decode_option(decoder, decode_f32_value)?,
            pitch: decode_option(decoder, decode_f32_value)?,
            on_ground: decoder.read_bool()?,
        }),
        2 => Ok(GameplayMutation::SelectedHotbarSlot {
            player_id: decode_player_id(decoder)?,
            slot: decoder.read_u8()?,
        }),
        3 => Ok(GameplayMutation::InventorySlot {
            player_id: decode_player_id(decoder)?,
            slot: decode_inventory_slot(decoder)?,
            stack: decode_option(decoder, decode_item_stack)?,
        }),
        7 => Ok(GameplayMutation::ClearMining {
            player_id: decode_player_id(decoder)?,
        }),
        8 => Ok(GameplayMutation::BeginMining {
            player_id: decode_player_id(decoder)?,
            position: decode_block_pos(decoder)?,
            duration_ms: decoder.read_u64()?,
        }),
        4 => Ok(GameplayMutation::Block {
            position: decode_block_pos(decoder)?,
            block: decode_block_state(decoder)?,
        }),
        5 => Ok(GameplayMutation::OpenChest {
            player_id: decode_player_id(decoder)?,
            position: decode_block_pos(decoder)?,
        }),
        6 => Ok(GameplayMutation::DroppedItem {
            position: decode_vec3(decoder)?,
            item: decode_item_stack(decoder)?,
        }),
        _ => Err(ProtocolCodecError::InvalidValue(
            "invalid gameplay mutation tag",
        )),
    }
}

fn encode_targeted_events(
    encoder: &mut Encoder,
    events: &[TargetedEvent],
) -> Result<(), ProtocolCodecError> {
    encoder.write_len(events.len())?;
    for event in events {
        encode_targeted_event(encoder, event)?;
    }
    Ok(())
}

fn decode_targeted_events(
    decoder: &mut Decoder<'_>,
) -> Result<Vec<TargetedEvent>, ProtocolCodecError> {
    let len = decoder.read_len()?;
    let mut events = Vec::with_capacity(len);
    for _ in 0..len {
        events.push(decode_targeted_event(decoder)?);
    }
    Ok(events)
}

fn encode_targeted_event(
    encoder: &mut Encoder,
    event: &TargetedEvent,
) -> Result<(), ProtocolCodecError> {
    match event.target {
        EventTarget::Connection(connection_id) => {
            encoder.write_u8(1);
            encoder.write_u64(connection_id.0);
        }
        EventTarget::Player(player_id) => {
            encoder.write_u8(2);
            encode_player_id(encoder, player_id);
        }
        EventTarget::EveryoneExcept(player_id) => {
            encoder.write_u8(3);
            encode_player_id(encoder, player_id);
        }
    }
    encode_core_event(encoder, &event.event)
}

fn decode_targeted_event(decoder: &mut Decoder<'_>) -> Result<TargetedEvent, ProtocolCodecError> {
    let target = match decoder.read_u8()? {
        1 => EventTarget::Connection(mc_core::ConnectionId(decoder.read_u64()?)),
        2 => EventTarget::Player(decode_player_id(decoder)?),
        3 => EventTarget::EveryoneExcept(decode_player_id(decoder)?),
        _ => {
            return Err(ProtocolCodecError::InvalidValue(
                "invalid targeted event tag",
            ));
        }
    };
    Ok(TargetedEvent {
        target,
        event: decode_core_event(decoder)?,
    })
}

pub(crate) fn encode_player_inventory_blob_inner(
    encoder: &mut Encoder,
    inventory: &PlayerInventory,
) -> Result<(), ProtocolCodecError> {
    encode_player_inventory(encoder, inventory)
}

pub(crate) fn decode_player_inventory_blob_inner(
    decoder: &mut Decoder<'_>,
) -> Result<PlayerInventory, ProtocolCodecError> {
    decode_player_inventory(decoder)
}
