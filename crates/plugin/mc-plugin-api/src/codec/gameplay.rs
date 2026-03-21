use crate::abi::{CURRENT_PLUGIN_ABI, PluginKind};
use crate::codec::__internal::binary::{
    Decoder, Encoder, EnvelopeHeader, PROTOCOL_FLAG_RESPONSE, ProtocolCodecError, decode_block_pos,
    decode_block_state, decode_capability_set, decode_connection_phase, decode_core_command,
    decode_core_event, decode_entity_id, decode_envelope, decode_f32_value, decode_inventory_slot,
    decode_option, decode_player_id, decode_player_snapshot, decode_u8_value, decode_world_meta,
    encode_block_pos, encode_block_state, encode_capability_set, encode_connection_phase,
    encode_core_command, encode_core_event, encode_entity_id, encode_envelope,
    encode_inventory_slot, encode_option, encode_player_id, encode_player_snapshot,
    encode_world_meta,
};
use mc_core::{
    CapabilitySet, CoreCommand, EventTarget, GameplayEffect, GameplayJoinEffect, GameplayMutation,
    GameplayProfileId, PlayerId, PlayerInventory, PlayerSnapshot, TargetedEvent, WorldMeta,
};
use mc_proto_common::ConnectionPhase;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GameplayOpCode {
    Describe = 1,
    CapabilitySet = 2,
    HandlePlayerJoin = 3,
    HandleCommand = 4,
    HandleTick = 5,
    SessionClosed = 6,
    ExportSessionState = 7,
    ImportSessionState = 8,
}

impl TryFrom<u8> for GameplayOpCode {
    type Error = ProtocolCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Describe),
            2 => Ok(Self::CapabilitySet),
            3 => Ok(Self::HandlePlayerJoin),
            4 => Ok(Self::HandleCommand),
            5 => Ok(Self::HandleTick),
            6 => Ok(Self::SessionClosed),
            7 => Ok(Self::ExportSessionState),
            8 => Ok(Self::ImportSessionState),
            _ => Err(ProtocolCodecError::InvalidValue("invalid gameplay op code")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameplayDescriptor {
    pub profile: GameplayProfileId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameplaySessionSnapshot {
    pub phase: ConnectionPhase,
    pub player_id: Option<PlayerId>,
    pub entity_id: Option<mc_core::EntityId>,
    pub gameplay_profile: GameplayProfileId,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GameplayRequest {
    Describe,
    CapabilitySet,
    HandlePlayerJoin {
        session: GameplaySessionSnapshot,
        player: PlayerSnapshot,
    },
    HandleCommand {
        session: GameplaySessionSnapshot,
        command: CoreCommand,
    },
    HandleTick {
        session: GameplaySessionSnapshot,
        now_ms: u64,
    },
    SessionClosed {
        session: GameplaySessionSnapshot,
    },
    ExportSessionState {
        session: GameplaySessionSnapshot,
    },
    ImportSessionState {
        session: GameplaySessionSnapshot,
        blob: Vec<u8>,
    },
}

impl GameplayRequest {
    #[must_use]
    pub const fn op_code(&self) -> GameplayOpCode {
        match self {
            Self::Describe => GameplayOpCode::Describe,
            Self::CapabilitySet => GameplayOpCode::CapabilitySet,
            Self::HandlePlayerJoin { .. } => GameplayOpCode::HandlePlayerJoin,
            Self::HandleCommand { .. } => GameplayOpCode::HandleCommand,
            Self::HandleTick { .. } => GameplayOpCode::HandleTick,
            Self::SessionClosed { .. } => GameplayOpCode::SessionClosed,
            Self::ExportSessionState { .. } => GameplayOpCode::ExportSessionState,
            Self::ImportSessionState { .. } => GameplayOpCode::ImportSessionState,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GameplayResponse {
    Descriptor(GameplayDescriptor),
    CapabilitySet(CapabilitySet),
    JoinEffect(GameplayJoinEffect),
    Effect(GameplayEffect),
    SessionTransferBlob(Vec<u8>),
    Empty,
}

/// Encodes a gameplay request into the plugin protocol envelope.
///
/// # Errors
///
/// Returns an error when the request payload exceeds protocol length limits or contains values
/// that cannot be serialized.
pub fn encode_gameplay_request(request: &GameplayRequest) -> Result<Vec<u8>, ProtocolCodecError> {
    let mut payload = Encoder::default();
    encode_gameplay_request_payload(&mut payload, request)?;
    let payload = payload.into_inner();
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::Gameplay,
            op_code: request.op_code() as u8,
            flags: 0,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

/// Decodes a gameplay request from the plugin protocol envelope.
///
/// # Errors
///
/// Returns an error when the envelope is malformed, the plugin kind/opcode is invalid, or the
/// gameplay payload cannot be decoded.
pub fn decode_gameplay_request(bytes: &[u8]) -> Result<GameplayRequest, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::Gameplay {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "gameplay request had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE != 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "gameplay request unexpectedly set response flag",
        ));
    }
    let mut decoder = Decoder::new(payload);
    let request =
        decode_gameplay_request_payload(&mut decoder, GameplayOpCode::try_from(header.op_code)?)?;
    decoder.finish()?;
    Ok(request)
}

/// Encodes a gameplay response for the provided gameplay request.
///
/// # Errors
///
/// Returns an error when the response does not match the request opcode, exceeds protocol
/// length limits, or contains values that cannot be serialized.
pub fn encode_gameplay_response(
    request: &GameplayRequest,
    response: &GameplayResponse,
) -> Result<Vec<u8>, ProtocolCodecError> {
    let mut payload = Encoder::default();
    encode_gameplay_response_payload(&mut payload, request.op_code(), response)?;
    let payload = payload.into_inner();
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::Gameplay,
            op_code: request.op_code() as u8,
            flags: PROTOCOL_FLAG_RESPONSE,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

/// Decodes a gameplay response for the provided gameplay request.
///
/// # Errors
///
/// Returns an error when the envelope is malformed, the response opcode does not match the
/// request, or the gameplay payload cannot be decoded.
pub fn decode_gameplay_response(
    request: &GameplayRequest,
    bytes: &[u8],
) -> Result<GameplayResponse, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::Gameplay {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "gameplay response had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE == 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "gameplay response was missing response flag",
        ));
    }
    if GameplayOpCode::try_from(header.op_code)? != request.op_code() {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "gameplay response opcode did not match request",
        ));
    }
    let mut decoder = Decoder::new(payload);
    let response = decode_gameplay_response_payload(&mut decoder, request.op_code())?;
    decoder.finish()?;
    Ok(response)
}

fn encode_gameplay_request_payload(
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

fn decode_gameplay_request_payload(
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

fn encode_gameplay_response_payload(
    encoder: &mut Encoder,
    op_code: GameplayOpCode,
    response: &GameplayResponse,
) -> Result<(), ProtocolCodecError> {
    match (op_code, response) {
        (GameplayOpCode::Describe, GameplayResponse::Descriptor(descriptor)) => {
            encode_gameplay_descriptor(encoder, descriptor)
        }
        (GameplayOpCode::CapabilitySet, GameplayResponse::CapabilitySet(capability_set)) => {
            encode_capability_set(encoder, capability_set)
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

fn decode_gameplay_response_payload(
    decoder: &mut Decoder<'_>,
    op_code: GameplayOpCode,
) -> Result<GameplayResponse, ProtocolCodecError> {
    match op_code {
        GameplayOpCode::Describe => Ok(GameplayResponse::Descriptor(decode_gameplay_descriptor(
            decoder,
        )?)),
        GameplayOpCode::CapabilitySet => Ok(GameplayResponse::CapabilitySet(
            decode_capability_set(decoder)?,
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

fn encode_gameplay_descriptor(
    encoder: &mut Encoder,
    descriptor: &GameplayDescriptor,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(descriptor.profile.as_str())
}

fn decode_gameplay_descriptor(
    decoder: &mut Decoder<'_>,
) -> Result<GameplayDescriptor, ProtocolCodecError> {
    Ok(GameplayDescriptor {
        profile: GameplayProfileId::new(decoder.read_string()?),
    })
}

fn encode_gameplay_session_snapshot(
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

fn decode_gameplay_session_snapshot(
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
            encode_option(
                encoder,
                stack.as_ref(),
                crate::codec::protocol::encode_item_stack,
            )
        }
        GameplayMutation::Block { position, block } => {
            encoder.write_u8(4);
            encode_block_pos(encoder, *position);
            encode_block_state(encoder, block)
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
            stack: decode_option(decoder, crate::codec::protocol::decode_item_stack)?,
        }),
        4 => Ok(GameplayMutation::Block {
            position: decode_block_pos(decoder)?,
            block: decode_block_state(decoder)?,
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

fn encode_player_inventory_blob_inner(
    encoder: &mut Encoder,
    inventory: &PlayerInventory,
) -> Result<(), ProtocolCodecError> {
    encoder.write_len(inventory.slots.len())?;
    for stack in &inventory.slots {
        encode_option(
            encoder,
            stack.as_ref(),
            crate::codec::protocol::encode_item_stack,
        )?;
    }
    encode_option(
        encoder,
        inventory.offhand.as_ref(),
        crate::codec::protocol::encode_item_stack,
    )
}

fn decode_player_inventory_blob_inner(
    decoder: &mut Decoder<'_>,
) -> Result<PlayerInventory, ProtocolCodecError> {
    let len = decoder.read_len()?;
    let mut slots = Vec::with_capacity(len);
    for _ in 0..len {
        slots.push(decode_option(
            decoder,
            crate::codec::protocol::decode_item_stack,
        )?);
    }
    Ok(PlayerInventory {
        slots,
        offhand: decode_option(decoder, crate::codec::protocol::decode_item_stack)?,
    })
}

pub mod host_blob {
    use super::*;

    #[must_use]
    pub fn encode_player_id(player_id: PlayerId) -> Vec<u8> {
        let mut encoder = Encoder::default();
        super::encode_player_id(&mut encoder, player_id);
        encoder.into_inner()
    }

    /// Decodes a player identifier blob returned by the gameplay host API.
    ///
    /// # Errors
    ///
    /// Returns an error when the blob is truncated, malformed, or contains trailing bytes.
    pub fn decode_player_id(bytes: &[u8]) -> Result<PlayerId, ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let player_id = super::decode_player_id(&mut decoder)?;
        decoder.finish()?;
        Ok(player_id)
    }

    #[must_use]
    pub fn encode_block_pos(position: mc_core::BlockPos) -> Vec<u8> {
        let mut encoder = Encoder::default();
        super::encode_block_pos(&mut encoder, position);
        encoder.into_inner()
    }

    /// Decodes a block-position blob returned by the gameplay host API.
    ///
    /// # Errors
    ///
    /// Returns an error when the blob is truncated, malformed, or contains trailing bytes.
    pub fn decode_block_pos(bytes: &[u8]) -> Result<mc_core::BlockPos, ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let position = super::decode_block_pos(&mut decoder)?;
        decoder.finish()?;
        Ok(position)
    }

    /// Decodes the `(player_id, block_pos)` key used by the host can-edit-block cache.
    ///
    /// # Errors
    ///
    /// Returns an error when the blob is truncated, malformed, or contains trailing bytes.
    pub fn decode_can_edit_block_key(
        bytes: &[u8],
    ) -> Result<(PlayerId, mc_core::BlockPos), ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let player_id = super::decode_player_id(&mut decoder)?;
        let position = super::decode_block_pos(&mut decoder)?;
        decoder.finish()?;
        Ok((player_id, position))
    }

    #[must_use]
    pub fn encode_can_edit_block_key(player_id: PlayerId, position: mc_core::BlockPos) -> Vec<u8> {
        let mut encoder = Encoder::default();
        super::encode_player_id(&mut encoder, player_id);
        super::encode_block_pos(&mut encoder, position);
        encoder.into_inner()
    }

    /// Encodes an optional player snapshot for host-side gameplay queries.
    ///
    /// # Errors
    ///
    /// Returns an error when the snapshot cannot be serialized.
    pub fn encode_player_snapshot(
        snapshot: Option<&PlayerSnapshot>,
    ) -> Result<Vec<u8>, ProtocolCodecError> {
        let mut encoder = Encoder::default();
        encode_option(&mut encoder, snapshot, super::encode_player_snapshot)?;
        Ok(encoder.into_inner())
    }

    /// Decodes an optional player snapshot returned by the gameplay host API.
    ///
    /// # Errors
    ///
    /// Returns an error when the blob is truncated, malformed, or contains trailing bytes.
    pub fn decode_player_snapshot(
        bytes: &[u8],
    ) -> Result<Option<PlayerSnapshot>, ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let snapshot = decode_option(&mut decoder, super::decode_player_snapshot)?;
        decoder.finish()?;
        Ok(snapshot)
    }

    /// Encodes world metadata for host-side gameplay queries.
    ///
    /// # Errors
    ///
    /// Returns an error when the world metadata cannot be serialized.
    pub fn encode_world_meta(meta: &WorldMeta) -> Result<Vec<u8>, ProtocolCodecError> {
        let mut encoder = Encoder::default();
        super::encode_world_meta(&mut encoder, meta)?;
        Ok(encoder.into_inner())
    }

    /// Decodes world metadata returned by the gameplay host API.
    ///
    /// # Errors
    ///
    /// Returns an error when the blob is truncated, malformed, or contains trailing bytes.
    pub fn decode_world_meta(bytes: &[u8]) -> Result<WorldMeta, ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let meta = super::decode_world_meta(&mut decoder)?;
        decoder.finish()?;
        Ok(meta)
    }

    /// Encodes a block state for host-side gameplay queries.
    ///
    /// # Errors
    ///
    /// Returns an error when the block state cannot be serialized.
    pub fn encode_block_state(
        block_state: &mc_core::BlockState,
    ) -> Result<Vec<u8>, ProtocolCodecError> {
        let mut encoder = Encoder::default();
        super::encode_block_state(&mut encoder, block_state)?;
        Ok(encoder.into_inner())
    }

    /// Decodes a block state returned by the gameplay host API.
    ///
    /// # Errors
    ///
    /// Returns an error when the blob is truncated, malformed, or contains trailing bytes.
    pub fn decode_block_state(bytes: &[u8]) -> Result<mc_core::BlockState, ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let block = super::decode_block_state(&mut decoder)?;
        decoder.finish()?;
        Ok(block)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GameplayDescriptor, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
        decode_gameplay_request, decode_gameplay_response, encode_gameplay_request,
        encode_gameplay_response,
        host_blob::{
            decode_block_state, decode_can_edit_block_key, decode_player_id,
            decode_player_snapshot, decode_world_meta, encode_block_pos, encode_block_state,
            encode_can_edit_block_key, encode_player_id, encode_player_snapshot, encode_world_meta,
        },
    };
    use mc_core::{
        BlockPos, BlockState, CapabilitySet, CoreCommand, CoreEvent, GameplayEffect,
        GameplayJoinEffect, GameplayMutation, GameplayProfileId, InventoryContainer, InventorySlot,
        ItemStack, PlayerId, PlayerInventory, PlayerSnapshot, TargetedEvent, Vec3, WorldMeta,
    };
    use mc_proto_common::ConnectionPhase;
    use uuid::Uuid;

    fn sample_player_id() -> PlayerId {
        PlayerId(Uuid::from_u128(7))
    }

    fn sample_player() -> PlayerSnapshot {
        let mut inventory = PlayerInventory::new_empty();
        let _ = inventory.set(36, Some(ItemStack::new("minecraft:stone", 64, 0)));
        PlayerSnapshot {
            id: sample_player_id(),
            username: "alice".to_string(),
            position: Vec3::new(1.0, 64.0, 2.0),
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            dimension: mc_core::DimensionId::Overworld,
            health: 20.0,
            food: 20,
            food_saturation: 5.0,
            inventory,
            selected_hotbar_slot: 0,
        }
    }

    fn sample_session() -> GameplaySessionSnapshot {
        GameplaySessionSnapshot {
            phase: ConnectionPhase::Play,
            player_id: Some(sample_player_id()),
            entity_id: Some(mc_core::EntityId(3)),
            gameplay_profile: GameplayProfileId::new("canonical"),
        }
    }

    fn sample_world_meta() -> WorldMeta {
        WorldMeta {
            level_name: "world".to_string(),
            seed: 5,
            spawn: BlockPos::new(0, 64, 0),
            dimension: mc_core::DimensionId::Overworld,
            age: 10,
            time: 20,
            level_type: "FLAT".to_string(),
            game_mode: 1,
            difficulty: 1,
            max_players: 20,
        }
    }

    #[test]
    fn gameplay_ops_round_trip_with_binary_codec() {
        let mut capabilities = CapabilitySet::new();
        let _ = capabilities.insert("runtime.reload.gameplay");
        let requests_and_responses = vec![
            (
                GameplayRequest::Describe,
                GameplayResponse::Descriptor(GameplayDescriptor {
                    profile: GameplayProfileId::new("canonical"),
                }),
            ),
            (
                GameplayRequest::CapabilitySet,
                GameplayResponse::CapabilitySet(capabilities),
            ),
            (
                GameplayRequest::HandlePlayerJoin {
                    session: sample_session(),
                    player: sample_player(),
                },
                GameplayResponse::JoinEffect(GameplayJoinEffect {
                    inventory: None,
                    selected_hotbar_slot: Some(2),
                    emitted_events: vec![TargetedEvent {
                        target: mc_core::EventTarget::Player(sample_player_id()),
                        event: CoreEvent::SelectedHotbarSlotChanged { slot: 2 },
                    }],
                }),
            ),
            (
                GameplayRequest::HandleCommand {
                    session: sample_session(),
                    command: CoreCommand::PlaceBlock {
                        player_id: sample_player_id(),
                        hand: mc_core::InteractionHand::Main,
                        position: BlockPos::new(0, 64, 0),
                        face: Some(mc_core::BlockFace::Top),
                        held_item: Some(ItemStack::new("minecraft:stone", 64, 0)),
                    },
                },
                GameplayResponse::Effect(GameplayEffect {
                    mutations: vec![GameplayMutation::Block {
                        position: BlockPos::new(0, 65, 0),
                        block: BlockState::stone(),
                    }],
                    emitted_events: vec![TargetedEvent {
                        target: mc_core::EventTarget::Player(sample_player_id()),
                        event: CoreEvent::InventorySlotChanged {
                            container: InventoryContainer::Player,
                            slot: InventorySlot::Hotbar(0),
                            stack: Some(ItemStack::new("minecraft:stone", 63, 0)),
                        },
                    }],
                }),
            ),
            (
                GameplayRequest::HandleTick {
                    session: sample_session(),
                    now_ms: 42,
                },
                GameplayResponse::Effect(GameplayEffect::default()),
            ),
            (
                GameplayRequest::SessionClosed {
                    session: sample_session(),
                },
                GameplayResponse::Empty,
            ),
            (
                GameplayRequest::ExportSessionState {
                    session: sample_session(),
                },
                GameplayResponse::SessionTransferBlob(b"state".to_vec()),
            ),
            (
                GameplayRequest::ImportSessionState {
                    session: sample_session(),
                    blob: b"state".to_vec(),
                },
                GameplayResponse::Empty,
            ),
        ];

        for (request, response) in requests_and_responses {
            let encoded_request = encode_gameplay_request(&request).expect("request should encode");
            let decoded_request =
                decode_gameplay_request(&encoded_request).expect("request should decode");
            assert_eq!(decoded_request, request);

            let encoded_response =
                encode_gameplay_response(&request, &response).expect("response should encode");
            let decoded_response = decode_gameplay_response(&request, &encoded_response)
                .expect("response should decode");
            assert_eq!(decoded_response, response);
        }
    }

    #[test]
    fn host_blob_helpers_round_trip() {
        let player_id_bytes = encode_player_id(sample_player_id());
        assert_eq!(
            decode_player_id(&player_id_bytes).expect("player id should decode"),
            sample_player_id()
        );

        let player_blob = encode_player_snapshot(Some(&sample_player())).expect("snapshot encodes");
        assert_eq!(
            decode_player_snapshot(&player_blob).expect("snapshot decodes"),
            Some(sample_player())
        );

        let world_blob = encode_world_meta(&sample_world_meta()).expect("world encodes");
        assert_eq!(
            decode_world_meta(&world_blob).expect("world decodes"),
            sample_world_meta()
        );

        let block_blob = encode_block_state(&BlockState::stone()).expect("block state encodes");
        assert_eq!(
            decode_block_state(&block_blob).expect("block state decodes"),
            BlockState::stone()
        );

        let key = encode_can_edit_block_key(sample_player_id(), BlockPos::new(1, 2, 3));
        assert_eq!(
            decode_can_edit_block_key(&key).expect("key decodes"),
            (sample_player_id(), BlockPos::new(1, 2, 3))
        );

        let position_bytes = encode_block_pos(BlockPos::new(4, 5, 6));
        assert_eq!(
            decode_block_state(
                &encode_block_state(&BlockState::new("minecraft:glass")).expect("block encodes")
            )
            .expect("block decodes"),
            BlockState::new("minecraft:glass")
        );
        assert!(!position_bytes.is_empty());
    }
}
