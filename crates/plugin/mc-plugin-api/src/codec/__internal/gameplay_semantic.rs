use crate::codec::__internal::binary::{Decoder, Encoder, ProtocolCodecError};
use crate::codec::__internal::inventory::{
    decode_inventory_slot, decode_item_stack, encode_inventory_slot, encode_item_stack,
};
use crate::codec::__internal::shared::{
    decode_block_entity_state, decode_block_pos, decode_capability_announcement,
    decode_connection_phase, decode_core_event, decode_entity_id, decode_f32_value,
    decode_gameplay_command, decode_option, decode_optional_block_state, decode_player_id,
    decode_player_snapshot, decode_vec3, decode_world_meta, encode_block_entity_state,
    encode_block_pos, encode_capability_announcement, encode_connection_phase, encode_core_event,
    encode_entity_id, encode_gameplay_command, encode_option, encode_optional_block_state,
    encode_player_id, encode_player_snapshot, encode_vec3, encode_world_meta,
};
use crate::codec::gameplay::{
    GameplayDescriptor, GameplayOpCode, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
};
use revy_voxel_rules::ContainerKindId;
use revy_voxel_semantic::{
    CapabilityAnnouncement, EventTarget, GameplayCanEditBlockKey, GameplayEffect,
    GameplayEffectBatch, GameplayProfileId, GameplayReadSet, ProtocolCapability, TargetedEvent,
};

pub(crate) fn encode_gameplay_request_payload(
    encoder: &mut Encoder,
    request: &GameplayRequest,
) -> Result<(), ProtocolCodecError> {
    match request {
        GameplayRequest::Describe | GameplayRequest::CapabilitySet => Ok(()),
        GameplayRequest::HandlePlayerJoin {
            session,
            player_id,
            now_ms,
        } => {
            encode_gameplay_session_snapshot(encoder, session)?;
            encode_player_id(encoder, *player_id);
            encoder.write_u64(*now_ms);
            Ok(())
        }
        GameplayRequest::HandleCommand {
            session,
            command,
            now_ms,
        } => {
            encode_gameplay_session_snapshot(encoder, session)?;
            encode_gameplay_command(encoder, command)?;
            encoder.write_u64(*now_ms);
            Ok(())
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
            player_id: decode_player_id(decoder)?,
            now_ms: decoder.read_u64()?,
        }),
        GameplayOpCode::HandleCommand => Ok(GameplayRequest::HandleCommand {
            session: decode_gameplay_session_snapshot(decoder)?,
            command: decode_gameplay_command(decoder)?,
            now_ms: decoder.read_u64()?,
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
        (
            GameplayOpCode::HandlePlayerJoin
            | GameplayOpCode::HandleCommand
            | GameplayOpCode::HandleTick,
            GameplayResponse::EffectBatch(batch),
        ) => encode_gameplay_effect_batch(encoder, batch),
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
        GameplayOpCode::HandlePlayerJoin
        | GameplayOpCode::HandleCommand
        | GameplayOpCode::HandleTick => Ok(GameplayResponse::EffectBatch(
            decode_gameplay_effect_batch(decoder)?,
        )),
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
    encode_capability_announcement(
        encoder,
        &CapabilityAnnouncement::<ProtocolCapability>::new(snapshot.protocol.clone()),
    )?;
    encoder.write_string(snapshot.gameplay_profile.as_str())?;
    encode_option(
        encoder,
        snapshot.protocol_generation.as_ref(),
        |encoder, generation| {
            encoder.write_u64(generation.0);
            Ok(())
        },
    )?;
    encode_option(
        encoder,
        snapshot.gameplay_generation.as_ref(),
        |encoder, generation| {
            encoder.write_u64(generation.0);
            Ok(())
        },
    )
}

pub(crate) fn decode_gameplay_session_snapshot(
    decoder: &mut Decoder<'_>,
) -> Result<GameplaySessionSnapshot, ProtocolCodecError> {
    Ok(GameplaySessionSnapshot {
        phase: decode_connection_phase(decoder)?,
        player_id: decode_option(decoder, decode_player_id)?,
        entity_id: decode_option(decoder, decode_entity_id)?,
        protocol: decode_capability_announcement::<ProtocolCapability>(decoder)?.capabilities,
        gameplay_profile: GameplayProfileId::new(decoder.read_string()?),
        protocol_generation: decode_option(decoder, |decoder| {
            Ok(revy_voxel_semantic::PluginGenerationId(decoder.read_u64()?))
        })?,
        gameplay_generation: decode_option(decoder, |decoder| {
            Ok(revy_voxel_semantic::PluginGenerationId(decoder.read_u64()?))
        })?,
    })
}

#[expect(
    dead_code,
    reason = "vector helpers are retained for future multi-event host payloads"
)]
pub(crate) fn encode_targeted_events(
    encoder: &mut Encoder,
    events: &[TargetedEvent],
) -> Result<(), ProtocolCodecError> {
    encoder.write_len(events.len())?;
    for event in events {
        encode_targeted_event(encoder, event)?;
    }
    Ok(())
}

#[expect(
    dead_code,
    reason = "vector helpers are retained for future multi-event host payloads"
)]
pub(crate) fn decode_targeted_events(
    decoder: &mut Decoder<'_>,
) -> Result<Vec<TargetedEvent>, ProtocolCodecError> {
    let len = decoder.read_len()?;
    let mut events = Vec::with_capacity(len);
    for _ in 0..len {
        events.push(decode_targeted_event(decoder)?);
    }
    Ok(events)
}

pub(crate) fn encode_targeted_event(
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

pub(crate) fn decode_targeted_event(
    decoder: &mut Decoder<'_>,
) -> Result<TargetedEvent, ProtocolCodecError> {
    let target = match decoder.read_u8()? {
        1 => EventTarget::Connection(revy_voxel_semantic::ConnectionId(decoder.read_u64()?)),
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

pub(crate) fn encode_gameplay_read_set(
    encoder: &mut Encoder,
    reads: &GameplayReadSet,
) -> Result<(), ProtocolCodecError> {
    encode_option(encoder, reads.world_meta.as_ref(), encode_world_meta)?;

    encoder.write_len(reads.player_snapshots.len())?;
    for (player_id, snapshot) in &reads.player_snapshots {
        encode_player_id(encoder, *player_id);
        encode_option(encoder, snapshot.as_ref(), encode_player_snapshot)?;
    }

    encoder.write_len(reads.block_states.len())?;
    for (position, block_state) in &reads.block_states {
        encode_block_pos(encoder, *position);
        encode_optional_block_state(encoder, block_state.as_ref())?;
    }

    encoder.write_len(reads.block_entities.len())?;
    for (position, block_entity) in &reads.block_entities {
        encode_block_pos(encoder, *position);
        encode_option(encoder, block_entity.as_ref(), encode_block_entity_state)?;
    }

    encoder.write_len(reads.can_edit_block.len())?;
    for (key, allowed) in &reads.can_edit_block {
        encode_player_id(encoder, key.player_id);
        encode_block_pos(encoder, key.position);
        encoder.write_bool(*allowed);
    }
    Ok(())
}

pub(crate) fn decode_gameplay_read_set(
    decoder: &mut Decoder<'_>,
) -> Result<GameplayReadSet, ProtocolCodecError> {
    let world_meta = decode_option(decoder, decode_world_meta)?;

    let player_snapshots_len = decoder.read_len()?;
    let mut player_snapshots = std::collections::BTreeMap::new();
    for _ in 0..player_snapshots_len {
        player_snapshots.insert(
            decode_player_id(decoder)?,
            decode_option(decoder, decode_player_snapshot)?,
        );
    }

    let block_states_len = decoder.read_len()?;
    let mut block_states = std::collections::BTreeMap::new();
    for _ in 0..block_states_len {
        block_states.insert(
            decode_block_pos(decoder)?,
            decode_optional_block_state(decoder)?,
        );
    }

    let block_entities_len = decoder.read_len()?;
    let mut block_entities = std::collections::BTreeMap::new();
    for _ in 0..block_entities_len {
        block_entities.insert(
            decode_block_pos(decoder)?,
            decode_option(decoder, decode_block_entity_state)?,
        );
    }

    let can_edit_block_len = decoder.read_len()?;
    let mut can_edit_block = std::collections::BTreeMap::new();
    for _ in 0..can_edit_block_len {
        can_edit_block.insert(
            GameplayCanEditBlockKey {
                player_id: decode_player_id(decoder)?,
                position: decode_block_pos(decoder)?,
            },
            decoder.read_bool()?,
        );
    }

    Ok(GameplayReadSet {
        world_meta,
        player_snapshots,
        block_states,
        block_entities,
        can_edit_block,
    })
}

pub(crate) fn encode_gameplay_effect(
    encoder: &mut Encoder,
    effect: &GameplayEffect,
) -> Result<(), ProtocolCodecError> {
    match effect {
        GameplayEffect::SetPlayerPose {
            player_id,
            position,
            yaw,
            pitch,
            on_ground,
        } => {
            encoder.write_u8(1);
            encode_player_id(encoder, *player_id);
            encode_option(encoder, position.as_ref(), |encoder, position| {
                encode_vec3(encoder, *position);
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
        }
        GameplayEffect::SetSelectedHotbarSlot { player_id, slot } => {
            encoder.write_u8(2);
            encode_player_id(encoder, *player_id);
            encoder.write_u8(*slot);
        }
        GameplayEffect::SetInventorySlot {
            player_id,
            slot,
            stack,
        } => {
            encoder.write_u8(3);
            encode_player_id(encoder, *player_id);
            encode_inventory_slot(encoder, *slot);
            encode_option(encoder, stack.as_ref(), encode_item_stack)?;
        }
        GameplayEffect::ClearMining { player_id } => {
            encoder.write_u8(4);
            encode_player_id(encoder, *player_id);
        }
        GameplayEffect::BeginMining {
            player_id,
            position,
            duration_ms,
        } => {
            encoder.write_u8(5);
            encode_player_id(encoder, *player_id);
            encode_block_pos(encoder, *position);
            encoder.write_u64(*duration_ms);
        }
        GameplayEffect::OpenContainerAt {
            player_id,
            position,
        } => {
            encoder.write_u8(6);
            encode_player_id(encoder, *player_id);
            encode_block_pos(encoder, *position);
        }
        GameplayEffect::OpenVirtualContainer { player_id, kind } => {
            encoder.write_u8(7);
            encode_player_id(encoder, *player_id);
            encoder.write_string(kind.as_str())?;
        }
        GameplayEffect::SetBlock { position, block } => {
            encoder.write_u8(8);
            encode_block_pos(encoder, *position);
            encode_optional_block_state(encoder, block.as_ref())?;
        }
        GameplayEffect::SpawnDroppedItem { position, item } => {
            encoder.write_u8(9);
            encode_vec3(encoder, *position);
            encode_item_stack(encoder, item)?;
        }
        GameplayEffect::EmitEvent { event } => {
            encoder.write_u8(10);
            encode_targeted_event(encoder, event)?;
        }
    }
    Ok(())
}

pub(crate) fn decode_gameplay_effect(
    decoder: &mut Decoder<'_>,
) -> Result<GameplayEffect, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(GameplayEffect::SetPlayerPose {
            player_id: decode_player_id(decoder)?,
            position: decode_option(decoder, decode_vec3)?,
            yaw: decode_option(decoder, decode_f32_value)?,
            pitch: decode_option(decoder, decode_f32_value)?,
            on_ground: decoder.read_bool()?,
        }),
        2 => Ok(GameplayEffect::SetSelectedHotbarSlot {
            player_id: decode_player_id(decoder)?,
            slot: decoder.read_u8()?,
        }),
        3 => Ok(GameplayEffect::SetInventorySlot {
            player_id: decode_player_id(decoder)?,
            slot: decode_inventory_slot(decoder)?,
            stack: decode_option(decoder, decode_item_stack)?,
        }),
        4 => Ok(GameplayEffect::ClearMining {
            player_id: decode_player_id(decoder)?,
        }),
        5 => Ok(GameplayEffect::BeginMining {
            player_id: decode_player_id(decoder)?,
            position: decode_block_pos(decoder)?,
            duration_ms: decoder.read_u64()?,
        }),
        6 => Ok(GameplayEffect::OpenContainerAt {
            player_id: decode_player_id(decoder)?,
            position: decode_block_pos(decoder)?,
        }),
        7 => Ok(GameplayEffect::OpenVirtualContainer {
            player_id: decode_player_id(decoder)?,
            kind: ContainerKindId::new(decoder.read_string()?),
        }),
        8 => Ok(GameplayEffect::SetBlock {
            position: decode_block_pos(decoder)?,
            block: decode_optional_block_state(decoder)?,
        }),
        9 => Ok(GameplayEffect::SpawnDroppedItem {
            position: decode_vec3(decoder)?,
            item: decode_item_stack(decoder)?,
        }),
        10 => Ok(GameplayEffect::EmitEvent {
            event: decode_targeted_event(decoder)?,
        }),
        _ => Err(ProtocolCodecError::InvalidValue(
            "invalid gameplay effect tag",
        )),
    }
}

pub(crate) fn encode_gameplay_effect_batch(
    encoder: &mut Encoder,
    batch: &GameplayEffectBatch,
) -> Result<(), ProtocolCodecError> {
    encoder.write_u64(batch.now_ms);
    encode_gameplay_read_set(encoder, &batch.reads)?;
    encoder.write_len(batch.effects.len())?;
    for effect in &batch.effects {
        encode_gameplay_effect(encoder, effect)?;
    }
    Ok(())
}

pub(crate) fn decode_gameplay_effect_batch(
    decoder: &mut Decoder<'_>,
) -> Result<GameplayEffectBatch, ProtocolCodecError> {
    let now_ms = decoder.read_u64()?;
    let reads = decode_gameplay_read_set(decoder)?;
    let len = decoder.read_len()?;
    let mut effects = Vec::with_capacity(len);
    for _ in 0..len {
        effects.push(decode_gameplay_effect(decoder)?);
    }
    Ok(GameplayEffectBatch {
        now_ms,
        reads,
        effects,
    })
}
