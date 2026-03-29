use crate::abi::{CURRENT_PLUGIN_ABI, PluginKind};
use crate::codec::__internal::binary::{
    Decoder, Encoder, EnvelopeHeader, PROTOCOL_FLAG_RESPONSE, ProtocolCodecError, decode_envelope,
    encode_envelope,
};
use crate::codec::__internal::gameplay_semantic::{
    decode_gameplay_request_payload, decode_gameplay_response_payload,
    encode_gameplay_request_payload, encode_gameplay_response_payload,
};
use crate::codec::__internal::shared::{
    decode_block_pos, decode_option, decode_optional_block_state, decode_player_id,
    decode_player_snapshot, decode_world_meta, encode_block_pos, encode_option,
    encode_optional_block_state, encode_player_id, encode_player_snapshot, encode_world_meta,
};
use mc_core::{
    CapabilityAnnouncement, GameplayCapability, GameplayCommand, GameplayProfileId, PlayerId,
    PlayerSnapshot, PluginGenerationId, ProtocolCapabilitySet, WorldMeta,
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
    pub protocol: ProtocolCapabilitySet,
    pub gameplay_profile: GameplayProfileId,
    pub protocol_generation: Option<PluginGenerationId>,
    pub gameplay_generation: Option<PluginGenerationId>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GameplayRequest {
    Describe,
    CapabilitySet,
    HandlePlayerJoin {
        session: GameplaySessionSnapshot,
        player_id: PlayerId,
    },
    HandleCommand {
        session: GameplaySessionSnapshot,
        command: GameplayCommand,
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
    CapabilitySet(CapabilityAnnouncement<GameplayCapability>),
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

pub mod host_blob {
    use super::*;
    use crate::codec::__internal::gameplay_semantic::{
        decode_targeted_event, encode_targeted_event,
    };
    use crate::codec::__internal::shared::{
        decode_block_entity_state, decode_f32_value, decode_vec3, encode_block_entity_state,
        encode_vec3,
    };

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
        block_state: Option<&mc_core::BlockState>,
    ) -> Result<Vec<u8>, ProtocolCodecError> {
        let mut encoder = Encoder::default();
        encode_optional_block_state(&mut encoder, block_state)?;
        Ok(encoder.into_inner())
    }

    /// Decodes a block state returned by the gameplay host API.
    ///
    /// # Errors
    ///
    /// Returns an error when the blob is truncated, malformed, or contains trailing bytes.
    pub fn decode_block_state(
        bytes: &[u8],
    ) -> Result<Option<mc_core::BlockState>, ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let block = decode_optional_block_state(&mut decoder)?;
        decoder.finish()?;
        Ok(block)
    }

    /// Encodes an optional block entity state for host-side gameplay queries.
    ///
    /// # Errors
    ///
    /// Returns an error when the block entity cannot be serialized.
    pub fn encode_block_entity(
        block_entity: Option<&mc_core::BlockEntityState>,
    ) -> Result<Vec<u8>, ProtocolCodecError> {
        let mut encoder = Encoder::default();
        encode_option(&mut encoder, block_entity, encode_block_entity_state)?;
        Ok(encoder.into_inner())
    }

    /// Decodes an optional block entity returned by the gameplay host API.
    ///
    /// # Errors
    ///
    /// Returns an error when the blob is truncated, malformed, or contains trailing bytes.
    pub fn decode_block_entity(
        bytes: &[u8],
    ) -> Result<Option<mc_core::BlockEntityState>, ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let block_entity = decode_option(&mut decoder, decode_block_entity_state)?;
        decoder.finish()?;
        Ok(block_entity)
    }

    #[must_use]
    pub fn encode_player_pose_update(
        player_id: PlayerId,
        position: Option<mc_core::Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    ) -> Vec<u8> {
        let mut encoder = Encoder::default();
        super::encode_player_id(&mut encoder, player_id);
        let _ = encode_option(&mut encoder, position.as_ref(), |encoder, position| {
            encode_vec3(encoder, *position);
            Ok(())
        });
        let _ = encode_option(&mut encoder, yaw.as_ref(), |encoder, yaw| {
            encoder.write_f32(*yaw);
            Ok(())
        });
        let _ = encode_option(&mut encoder, pitch.as_ref(), |encoder, pitch| {
            encoder.write_f32(*pitch);
            Ok(())
        });
        encoder.write_bool(on_ground);
        encoder.into_inner()
    }

    pub fn decode_player_pose_update(
        bytes: &[u8],
    ) -> Result<
        (
            PlayerId,
            Option<mc_core::Vec3>,
            Option<f32>,
            Option<f32>,
            bool,
        ),
        ProtocolCodecError,
    > {
        let mut decoder = Decoder::new(bytes);
        let player_id = super::decode_player_id(&mut decoder)?;
        let position = decode_option(&mut decoder, decode_vec3)?;
        let yaw = decode_option(&mut decoder, decode_f32_value)?;
        let pitch = decode_option(&mut decoder, decode_f32_value)?;
        let on_ground = decoder.read_bool()?;
        decoder.finish()?;
        Ok((player_id, position, yaw, pitch, on_ground))
    }

    #[must_use]
    pub fn encode_selected_hotbar_slot_update(player_id: PlayerId, slot: u8) -> Vec<u8> {
        let mut encoder = Encoder::default();
        super::encode_player_id(&mut encoder, player_id);
        encoder.write_u8(slot);
        encoder.into_inner()
    }

    pub fn decode_selected_hotbar_slot_update(
        bytes: &[u8],
    ) -> Result<(PlayerId, u8), ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let player_id = super::decode_player_id(&mut decoder)?;
        let slot = decoder.read_u8()?;
        decoder.finish()?;
        Ok((player_id, slot))
    }

    pub fn encode_inventory_slot_update(
        player_id: PlayerId,
        slot: mc_core::InventorySlot,
        stack: Option<&mc_core::ItemStack>,
    ) -> Result<Vec<u8>, ProtocolCodecError> {
        let mut encoder = Encoder::default();
        super::encode_player_id(&mut encoder, player_id);
        crate::codec::__internal::inventory::encode_inventory_slot(&mut encoder, slot);
        encode_option(
            &mut encoder,
            stack,
            crate::codec::__internal::inventory::encode_item_stack,
        )?;
        Ok(encoder.into_inner())
    }

    pub fn decode_inventory_slot_update(
        bytes: &[u8],
    ) -> Result<(PlayerId, mc_core::InventorySlot, Option<mc_core::ItemStack>), ProtocolCodecError>
    {
        let mut decoder = Decoder::new(bytes);
        let player_id = super::decode_player_id(&mut decoder)?;
        let slot = crate::codec::__internal::inventory::decode_inventory_slot(&mut decoder)?;
        let stack = decode_option(
            &mut decoder,
            crate::codec::__internal::inventory::decode_item_stack,
        )?;
        decoder.finish()?;
        Ok((player_id, slot, stack))
    }

    #[must_use]
    pub fn encode_clear_mining(player_id: PlayerId) -> Vec<u8> {
        encode_player_id(player_id)
    }

    pub fn decode_clear_mining(bytes: &[u8]) -> Result<PlayerId, ProtocolCodecError> {
        decode_player_id(bytes)
    }

    #[must_use]
    pub fn encode_begin_mining(
        player_id: PlayerId,
        position: mc_core::BlockPos,
        duration_ms: u64,
    ) -> Vec<u8> {
        let mut encoder = Encoder::default();
        super::encode_player_id(&mut encoder, player_id);
        super::encode_block_pos(&mut encoder, position);
        encoder.write_u64(duration_ms);
        encoder.into_inner()
    }

    pub fn decode_begin_mining(
        bytes: &[u8],
    ) -> Result<(PlayerId, mc_core::BlockPos, u64), ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let player_id = super::decode_player_id(&mut decoder)?;
        let position = super::decode_block_pos(&mut decoder)?;
        let duration_ms = decoder.read_u64()?;
        decoder.finish()?;
        Ok((player_id, position, duration_ms))
    }

    #[must_use]
    pub fn encode_open_container_at(player_id: PlayerId, position: mc_core::BlockPos) -> Vec<u8> {
        let mut encoder = Encoder::default();
        super::encode_player_id(&mut encoder, player_id);
        super::encode_block_pos(&mut encoder, position);
        encoder.into_inner()
    }

    pub fn decode_open_container_at(
        bytes: &[u8],
    ) -> Result<(PlayerId, mc_core::BlockPos), ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let player_id = super::decode_player_id(&mut decoder)?;
        let position = super::decode_block_pos(&mut decoder)?;
        decoder.finish()?;
        Ok((player_id, position))
    }

    pub fn encode_open_virtual_container(
        player_id: PlayerId,
        kind: &mc_core::ContainerKindId,
    ) -> Vec<u8> {
        let mut encoder = Encoder::default();
        super::encode_player_id(&mut encoder, player_id);
        crate::codec::__internal::inventory::encode_inventory_container(&mut encoder, kind)
            .expect("container kind should encode for gameplay host payload");
        encoder.into_inner()
    }

    pub fn decode_open_virtual_container(
        bytes: &[u8],
    ) -> Result<(PlayerId, mc_core::ContainerKindId), ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let player_id = super::decode_player_id(&mut decoder)?;
        let kind = crate::codec::__internal::inventory::decode_inventory_container(&mut decoder)?;
        decoder.finish()?;
        Ok((player_id, kind))
    }

    pub fn encode_set_block(
        position: mc_core::BlockPos,
        block: Option<&mc_core::BlockState>,
    ) -> Result<Vec<u8>, ProtocolCodecError> {
        let mut encoder = Encoder::default();
        super::encode_block_pos(&mut encoder, position);
        encode_optional_block_state(&mut encoder, block)?;
        Ok(encoder.into_inner())
    }

    pub fn decode_set_block(
        bytes: &[u8],
    ) -> Result<(mc_core::BlockPos, Option<mc_core::BlockState>), ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let position = super::decode_block_pos(&mut decoder)?;
        let block = decode_optional_block_state(&mut decoder)?;
        decoder.finish()?;
        Ok((position, block))
    }

    pub fn encode_spawn_dropped_item(
        position: mc_core::Vec3,
        item: &mc_core::ItemStack,
    ) -> Result<Vec<u8>, ProtocolCodecError> {
        let mut encoder = Encoder::default();
        encode_vec3(&mut encoder, position);
        crate::codec::__internal::inventory::encode_item_stack(&mut encoder, item)?;
        Ok(encoder.into_inner())
    }

    pub fn decode_spawn_dropped_item(
        bytes: &[u8],
    ) -> Result<(mc_core::Vec3, mc_core::ItemStack), ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let position = decode_vec3(&mut decoder)?;
        let item = crate::codec::__internal::inventory::decode_item_stack(&mut decoder)?;
        decoder.finish()?;
        Ok((position, item))
    }

    pub fn encode_targeted_event_blob(
        event: &mc_core::TargetedEvent,
    ) -> Result<Vec<u8>, ProtocolCodecError> {
        let mut encoder = Encoder::default();
        encode_targeted_event(&mut encoder, event)?;
        Ok(encoder.into_inner())
    }

    pub fn decode_targeted_event_blob(
        bytes: &[u8],
    ) -> Result<mc_core::TargetedEvent, ProtocolCodecError> {
        let mut decoder = Decoder::new(bytes);
        let event = decode_targeted_event(&mut decoder)?;
        decoder.finish()?;
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GameplayDescriptor, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
        decode_gameplay_request, decode_gameplay_response, encode_gameplay_request,
        encode_gameplay_response,
        host_blob::{
            decode_block_entity, decode_block_state, decode_can_edit_block_key, decode_player_id,
            decode_player_snapshot, decode_world_meta, encode_block_entity, encode_block_pos,
            encode_block_state, encode_can_edit_block_key, encode_player_id,
            encode_player_snapshot, encode_world_meta,
        },
    };
    use mc_core::{
        BlockEntityState, BlockPos, BlockState, CapabilityAnnouncement, GameplayCapability,
        GameplayCapabilitySet, GameplayCommand, GameplayProfileId, ItemStack, PlayerId,
        PlayerInventory, PlayerSnapshot, ProtocolCapabilitySet, Vec3, WorldMeta,
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
            protocol: ProtocolCapabilitySet::new(),
            gameplay_profile: GameplayProfileId::new("canonical"),
            protocol_generation: None,
            gameplay_generation: None,
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
        let mut capabilities = GameplayCapabilitySet::new();
        let _ = capabilities.insert(GameplayCapability::RuntimeReload);
        let requests_and_responses = vec![
            (
                GameplayRequest::Describe,
                GameplayResponse::Descriptor(GameplayDescriptor {
                    profile: GameplayProfileId::new("canonical"),
                }),
            ),
            (
                GameplayRequest::CapabilitySet,
                GameplayResponse::CapabilitySet(CapabilityAnnouncement::new(capabilities)),
            ),
            (
                GameplayRequest::HandlePlayerJoin {
                    session: sample_session(),
                    player_id: sample_player_id(),
                },
                GameplayResponse::Empty,
            ),
            (
                GameplayRequest::HandleCommand {
                    session: sample_session(),
                    command: GameplayCommand::UseBlock {
                        player_id: sample_player_id(),
                        hand: mc_core::InteractionHand::Main,
                        position: BlockPos::new(0, 64, 0),
                        face: Some(mc_core::BlockFace::Top),
                        held_item: Some(ItemStack::new("minecraft:stone", 64, 0)),
                    },
                },
                GameplayResponse::Empty,
            ),
            (
                GameplayRequest::HandleTick {
                    session: sample_session(),
                    now_ms: 42,
                },
                GameplayResponse::Empty,
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

        let block_blob = encode_block_state(Some(&BlockState::new("minecraft:stone")))
            .expect("block state encodes");
        assert_eq!(
            decode_block_state(&block_blob).expect("block state decodes"),
            Some(BlockState::new("minecraft:stone"))
        );

        let block_entity_blob = encode_block_entity(Some(&BlockEntityState::Container(
            mc_core::ContainerBlockEntityState {
                kind: mc_core::BlockEntityKindId::new("canonical:chest"),
                slots: vec![Some(ItemStack::new("minecraft:glass", 1, 0)), None],
                properties: std::collections::BTreeMap::new(),
            },
        )))
        .expect("block entity encodes");
        assert_eq!(
            decode_block_entity(&block_entity_blob).expect("block entity decodes"),
            Some(BlockEntityState::Container(
                mc_core::ContainerBlockEntityState {
                    kind: mc_core::BlockEntityKindId::new("canonical:chest"),
                    slots: vec![Some(ItemStack::new("minecraft:glass", 1, 0)), None],
                    properties: std::collections::BTreeMap::new(),
                }
            ))
        );

        let furnace_blob = encode_block_entity(Some(&BlockEntityState::Container(
            mc_core::ContainerBlockEntityState {
                kind: mc_core::BlockEntityKindId::new("canonical:furnace"),
                slots: vec![
                    Some(ItemStack::new("minecraft:sand", 1, 0)),
                    Some(ItemStack::new("minecraft:oak_planks", 1, 0)),
                    Some(ItemStack::new("minecraft:glass", 1, 0)),
                ],
                properties: std::collections::BTreeMap::from([
                    (
                        mc_core::ContainerPropertyKey::new("canonical:furnace.burn_left"),
                        120,
                    ),
                    (
                        mc_core::ContainerPropertyKey::new("canonical:furnace.burn_max"),
                        300,
                    ),
                    (
                        mc_core::ContainerPropertyKey::new("canonical:furnace.cook_progress"),
                        42,
                    ),
                    (
                        mc_core::ContainerPropertyKey::new("canonical:furnace.cook_total"),
                        200,
                    ),
                ]),
            },
        )))
        .expect("furnace block entity encodes");
        assert_eq!(
            decode_block_entity(&furnace_blob).expect("furnace block entity decodes"),
            Some(BlockEntityState::Container(
                mc_core::ContainerBlockEntityState {
                    kind: mc_core::BlockEntityKindId::new("canonical:furnace"),
                    slots: vec![
                        Some(ItemStack::new("minecraft:sand", 1, 0)),
                        Some(ItemStack::new("minecraft:oak_planks", 1, 0)),
                        Some(ItemStack::new("minecraft:glass", 1, 0)),
                    ],
                    properties: std::collections::BTreeMap::from([
                        (
                            mc_core::ContainerPropertyKey::new("canonical:furnace.burn_left"),
                            120
                        ),
                        (
                            mc_core::ContainerPropertyKey::new("canonical:furnace.burn_max"),
                            300
                        ),
                        (
                            mc_core::ContainerPropertyKey::new("canonical:furnace.cook_progress"),
                            42,
                        ),
                        (
                            mc_core::ContainerPropertyKey::new("canonical:furnace.cook_total"),
                            200
                        ),
                    ]),
                }
            ))
        );

        let key = encode_can_edit_block_key(sample_player_id(), BlockPos::new(1, 2, 3));
        assert_eq!(
            decode_can_edit_block_key(&key).expect("key decodes"),
            (sample_player_id(), BlockPos::new(1, 2, 3))
        );

        let position_bytes = encode_block_pos(BlockPos::new(4, 5, 6));
        assert_eq!(
            decode_block_state(
                &encode_block_state(Some(&BlockState::new("minecraft:glass")))
                    .expect("block encodes")
            )
            .expect("block decodes"),
            Some(BlockState::new("minecraft:glass"))
        );
        assert!(!position_bytes.is_empty());
    }
}
