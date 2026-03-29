use crate::abi::{CURRENT_PLUGIN_ABI, PluginKind};
#[cfg(test)]
use crate::codec::__internal::binary::PLUGIN_ENVELOPE_HEADER_LEN;
pub use crate::codec::__internal::binary::ProtocolCodecError;
use crate::codec::__internal::binary::{
    Decoder, Encoder, EnvelopeHeader, PROTOCOL_FLAG_RESPONSE, decode_envelope, encode_envelope,
};
use crate::codec::__internal::protocol_semantic::{
    decode_protocol_request_payload, decode_protocol_response_payload,
    encode_protocol_request_payload, encode_protocol_response_payload,
};
use mc_core::{
    CapabilityAnnouncement, CoreEvent, PlayerSnapshot, ProtocolCapability, RuntimeCommand,
};
pub use mc_proto_common::ProtocolSessionSnapshot;
use mc_proto_common::{
    BedrockListenerDescriptor, ConnectionPhase, HandshakeIntent, LoginRequest, PlayEncodingContext,
    ProtocolDescriptor, ServerListStatus, StatusRequest,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProtocolOpCode {
    Describe = 1,
    DescribeBedrockListener = 2,
    CapabilitySet = 3,
    TryRoute = 4,
    DecodeStatus = 5,
    DecodeLogin = 6,
    EncodeStatusResponse = 7,
    EncodeStatusPong = 8,
    EncodeDisconnect = 9,
    EncodeEncryptionRequest = 10,
    EncodeNetworkSettings = 11,
    EncodeLoginSuccess = 12,
    DecodePlay = 13,
    EncodePlayEvent = 14,
    ExportSessionState = 15,
    ImportSessionState = 16,
    EncodeWireFrame = 17,
    TryDecodeWireFrame = 18,
    SessionClosed = 19,
}

impl TryFrom<u8> for ProtocolOpCode {
    type Error = ProtocolCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Describe),
            2 => Ok(Self::DescribeBedrockListener),
            3 => Ok(Self::CapabilitySet),
            4 => Ok(Self::TryRoute),
            5 => Ok(Self::DecodeStatus),
            6 => Ok(Self::DecodeLogin),
            7 => Ok(Self::EncodeStatusResponse),
            8 => Ok(Self::EncodeStatusPong),
            9 => Ok(Self::EncodeDisconnect),
            10 => Ok(Self::EncodeEncryptionRequest),
            11 => Ok(Self::EncodeNetworkSettings),
            12 => Ok(Self::EncodeLoginSuccess),
            13 => Ok(Self::DecodePlay),
            14 => Ok(Self::EncodePlayEvent),
            15 => Ok(Self::ExportSessionState),
            16 => Ok(Self::ImportSessionState),
            17 => Ok(Self::EncodeWireFrame),
            18 => Ok(Self::TryDecodeWireFrame),
            19 => Ok(Self::SessionClosed),
            _ => Err(ProtocolCodecError::InvalidProtocolOpCode(value)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireFrameDecodeResult {
    pub frame: Vec<u8>,
    pub bytes_consumed: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProtocolRequest {
    Describe,
    DescribeBedrockListener,
    CapabilitySet,
    TryRoute {
        frame: Vec<u8>,
    },
    DecodeStatus {
        frame: Vec<u8>,
    },
    DecodeLogin {
        frame: Vec<u8>,
    },
    EncodeStatusResponse {
        status: ServerListStatus,
    },
    EncodeStatusPong {
        payload: i64,
    },
    EncodeDisconnect {
        phase: ConnectionPhase,
        reason: String,
    },
    EncodeEncryptionRequest {
        server_id: String,
        public_key_der: Vec<u8>,
        verify_token: Vec<u8>,
    },
    EncodeNetworkSettings {
        compression_threshold: u16,
    },
    EncodeLoginSuccess {
        player: PlayerSnapshot,
    },
    DecodePlay {
        session: ProtocolSessionSnapshot,
        frame: Vec<u8>,
    },
    EncodePlayEvent {
        session: ProtocolSessionSnapshot,
        event: CoreEvent,
        context: PlayEncodingContext,
    },
    SessionClosed {
        session: ProtocolSessionSnapshot,
    },
    ExportSessionState {
        session: ProtocolSessionSnapshot,
    },
    ImportSessionState {
        session: ProtocolSessionSnapshot,
        blob: Vec<u8>,
    },
    EncodeWireFrame {
        payload: Vec<u8>,
    },
    TryDecodeWireFrame {
        buffer: Vec<u8>,
    },
}

impl ProtocolRequest {
    #[must_use]
    pub const fn op_code(&self) -> ProtocolOpCode {
        match self {
            Self::Describe => ProtocolOpCode::Describe,
            Self::DescribeBedrockListener => ProtocolOpCode::DescribeBedrockListener,
            Self::CapabilitySet => ProtocolOpCode::CapabilitySet,
            Self::TryRoute { .. } => ProtocolOpCode::TryRoute,
            Self::DecodeStatus { .. } => ProtocolOpCode::DecodeStatus,
            Self::DecodeLogin { .. } => ProtocolOpCode::DecodeLogin,
            Self::EncodeStatusResponse { .. } => ProtocolOpCode::EncodeStatusResponse,
            Self::EncodeStatusPong { .. } => ProtocolOpCode::EncodeStatusPong,
            Self::EncodeDisconnect { .. } => ProtocolOpCode::EncodeDisconnect,
            Self::EncodeEncryptionRequest { .. } => ProtocolOpCode::EncodeEncryptionRequest,
            Self::EncodeNetworkSettings { .. } => ProtocolOpCode::EncodeNetworkSettings,
            Self::EncodeLoginSuccess { .. } => ProtocolOpCode::EncodeLoginSuccess,
            Self::DecodePlay { .. } => ProtocolOpCode::DecodePlay,
            Self::EncodePlayEvent { .. } => ProtocolOpCode::EncodePlayEvent,
            Self::SessionClosed { .. } => ProtocolOpCode::SessionClosed,
            Self::ExportSessionState { .. } => ProtocolOpCode::ExportSessionState,
            Self::ImportSessionState { .. } => ProtocolOpCode::ImportSessionState,
            Self::EncodeWireFrame { .. } => ProtocolOpCode::EncodeWireFrame,
            Self::TryDecodeWireFrame { .. } => ProtocolOpCode::TryDecodeWireFrame,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProtocolResponse {
    Descriptor(ProtocolDescriptor),
    BedrockListenerDescriptor(Option<BedrockListenerDescriptor>),
    CapabilitySet(CapabilityAnnouncement<ProtocolCapability>),
    HandshakeIntent(Option<HandshakeIntent>),
    StatusRequest(StatusRequest),
    LoginRequest(LoginRequest),
    Frame(Vec<u8>),
    Frames(Vec<Vec<u8>>),
    RuntimeCommand(Option<RuntimeCommand>),
    SessionTransferBlob(Vec<u8>),
    WireFrameDecodeResult(Option<WireFrameDecodeResult>),
    Empty,
}

/// Encodes a protocol request into the plugin protocol envelope.
///
/// # Errors
///
/// Returns an error when the request payload exceeds protocol length limits or contains values
/// that cannot be serialized.
pub fn encode_protocol_request(request: &ProtocolRequest) -> Result<Vec<u8>, ProtocolCodecError> {
    let mut payload = Encoder::default();
    encode_protocol_request_payload(&mut payload, request)?;
    let payload = payload.into_inner();
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::Protocol,
            op_code: request.op_code() as u8,
            flags: 0,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

/// Decodes a protocol request from the plugin protocol envelope.
///
/// # Errors
///
/// Returns an error when the envelope is malformed, the plugin kind/opcode is invalid, or the
/// protocol payload cannot be decoded.
pub fn decode_protocol_request(bytes: &[u8]) -> Result<ProtocolRequest, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::Protocol {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "protocol request had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE != 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "protocol request unexpectedly set response flag",
        ));
    }
    let mut decoder = Decoder::new(payload);
    let request =
        decode_protocol_request_payload(&mut decoder, ProtocolOpCode::try_from(header.op_code)?)?;
    decoder.finish()?;
    Ok(request)
}

/// Encodes a protocol response for the provided protocol request.
///
/// # Errors
///
/// Returns an error when the response does not match the request opcode, exceeds protocol
/// length limits, or contains values that cannot be serialized.
pub fn encode_protocol_response(
    request: &ProtocolRequest,
    response: &ProtocolResponse,
) -> Result<Vec<u8>, ProtocolCodecError> {
    let mut payload = Encoder::default();
    encode_protocol_response_payload(&mut payload, request.op_code(), response)?;
    let payload = payload.into_inner();
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::Protocol,
            op_code: request.op_code() as u8,
            flags: PROTOCOL_FLAG_RESPONSE,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

/// Decodes a protocol response for the provided protocol request.
///
/// # Errors
///
/// Returns an error when the envelope is malformed, the response opcode does not match the
/// request, or the protocol payload cannot be decoded.
pub fn decode_protocol_response(
    request: &ProtocolRequest,
    bytes: &[u8],
) -> Result<ProtocolResponse, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::Protocol {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "protocol response had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE == 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "protocol response was missing response flag",
        ));
    }
    if header.op_code != request.op_code() as u8 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "protocol response opcode did not match request",
        ));
    }
    let mut decoder = Decoder::new(payload);
    let response =
        decode_protocol_response_payload(&mut decoder, ProtocolOpCode::try_from(header.op_code)?)?;
    decoder.finish()?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::{
        CURRENT_PLUGIN_ABI, PLUGIN_ENVELOPE_HEADER_LEN, PROTOCOL_FLAG_RESPONSE, ProtocolCodecError,
        ProtocolRequest, ProtocolResponse, WireFrameDecodeResult, decode_protocol_request,
        decode_protocol_response, encode_protocol_request, encode_protocol_response,
    };
    use mc_core::{
        BlockPos, CapabilityAnnouncement, ChunkColumn, ConnectionId, ContainerKindId, CoreCommand,
        CoreEvent, EntityId, GameplayCapabilitySet, GameplayProfileId, InventorySlot, ItemStack,
        PlayerId, PlayerInventory, PlayerSnapshot, PluginGenerationId, ProtocolCapability,
        ProtocolCapabilitySet, RuntimeCommand, SessionCapabilitySet, Vec3, WorldMeta,
    };
    use mc_proto_common::{
        BedrockListenerDescriptor, ConnectionPhase, Edition, HandshakeIntent, HandshakeNextState,
        LoginRequest, PlayEncodingContext, ProtocolDescriptor, ServerListStatus, StatusRequest,
        TransportKind, WireFormatKind,
    };
    use uuid::Uuid;

    fn sample_player_id() -> PlayerId {
        PlayerId(Uuid::from_u128(42))
    }

    fn sample_player() -> PlayerSnapshot {
        let mut inventory = PlayerInventory::new_empty();
        let _ = inventory.set(36, Some(ItemStack::new("minecraft:stone", 32, 0)));
        inventory.offhand = Some(ItemStack::new("minecraft:shield", 1, 0));
        PlayerSnapshot {
            id: sample_player_id(),
            username: "alice".to_string(),
            position: Vec3::new(1.5, 64.0, -3.25),
            yaw: 90.0,
            pitch: 15.0,
            on_ground: true,
            dimension: mc_core::DimensionId::Overworld,
            health: 20.0,
            food: 18,
            food_saturation: 5.0,
            inventory,
            selected_hotbar_slot: 2,
        }
    }

    fn sample_world_meta() -> WorldMeta {
        WorldMeta {
            level_name: "world".to_string(),
            seed: 99,
            spawn: BlockPos::new(0, 64, 0),
            dimension: mc_core::DimensionId::Overworld,
            age: 10,
            time: 20,
            level_type: "FLAT".to_string(),
            game_mode: 1,
            difficulty: 2,
            max_players: 20,
        }
    }

    fn player_container() -> ContainerKindId {
        ContainerKindId::new("canonical:player")
    }

    fn sample_descriptor() -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: "je-5".to_string(),
            transport: TransportKind::Tcp,
            wire_format: WireFormatKind::MinecraftFramed,
            edition: Edition::Je,
            version_name: "1.7.10".to_string(),
            protocol_number: 5,
        }
    }

    fn sample_bedrock_listener_descriptor() -> BedrockListenerDescriptor {
        BedrockListenerDescriptor {
            game_version: "1.26.0".to_string(),
            raknet_version: 11,
        }
    }

    fn sample_event() -> CoreEvent {
        let mut chunk = ChunkColumn::new(mc_core::ChunkPos::new(0, 0));
        chunk.set_block(1, 64, 1, Some(mc_core::BlockState::new("minecraft:stone")));
        CoreEvent::PlayBootstrap {
            player: sample_player(),
            entity_id: EntityId(7),
            world_meta: sample_world_meta(),
            view_distance: 2,
        }
    }

    fn sample_command() -> RuntimeCommand {
        RuntimeCommand::Core(CoreCommand::CreativeInventorySet {
            player_id: sample_player_id(),
            slot: InventorySlot::Hotbar(1),
            stack: Some(ItemStack::new("minecraft:glass", 16, 0)),
        })
    }

    fn sample_protocol_session() -> super::ProtocolSessionSnapshot {
        super::ProtocolSessionSnapshot {
            connection_id: ConnectionId(1),
            phase: ConnectionPhase::Play,
            player_id: Some(sample_player_id()),
            entity_id: Some(EntityId(7)),
        }
    }

    fn sample_protocol_round_trips() -> Vec<(ProtocolRequest, ProtocolResponse)> {
        let mut round_trips = sample_protocol_round_trips_part_one();
        round_trips.extend(sample_protocol_round_trips_part_two());
        round_trips
    }

    fn sample_protocol_round_trips_part_one() -> Vec<(ProtocolRequest, ProtocolResponse)> {
        let mut capabilities = ProtocolCapabilitySet::new();
        let _ = capabilities.insert(ProtocolCapability::Je);
        let _ = capabilities.insert(ProtocolCapability::RuntimeReload);

        vec![
            (
                ProtocolRequest::Describe,
                ProtocolResponse::Descriptor(sample_descriptor()),
            ),
            (
                ProtocolRequest::DescribeBedrockListener,
                ProtocolResponse::BedrockListenerDescriptor(Some(
                    sample_bedrock_listener_descriptor(),
                )),
            ),
            (
                ProtocolRequest::CapabilitySet,
                ProtocolResponse::CapabilitySet(CapabilityAnnouncement::new(capabilities)),
            ),
            (
                ProtocolRequest::TryRoute {
                    frame: vec![1, 2, 3],
                },
                ProtocolResponse::HandshakeIntent(Some(HandshakeIntent {
                    edition: Edition::Je,
                    protocol_number: 5,
                    server_host: "localhost".to_string(),
                    server_port: 25565,
                    next_state: HandshakeNextState::Login,
                })),
            ),
            (
                ProtocolRequest::DecodeStatus {
                    frame: vec![9, 8, 7],
                },
                ProtocolResponse::StatusRequest(StatusRequest::Ping { payload: 99 }),
            ),
            (
                ProtocolRequest::DecodeLogin {
                    frame: vec![6, 5, 4],
                },
                ProtocolResponse::LoginRequest(LoginRequest::EncryptionResponse {
                    shared_secret_encrypted: vec![1, 2, 3, 4],
                    verify_token_encrypted: vec![5, 6, 7, 8],
                }),
            ),
            (
                ProtocolRequest::EncodeStatusResponse {
                    status: ServerListStatus {
                        version: sample_descriptor(),
                        players_online: 1,
                        max_players: 20,
                        description: "hello".to_string(),
                    },
                },
                ProtocolResponse::Frame(vec![0, 1]),
            ),
            (
                ProtocolRequest::EncodeStatusPong { payload: 123 },
                ProtocolResponse::Frame(vec![2, 3]),
            ),
        ]
    }

    fn sample_protocol_round_trips_part_two() -> Vec<(ProtocolRequest, ProtocolResponse)> {
        vec![
            (
                ProtocolRequest::EncodeDisconnect {
                    phase: ConnectionPhase::Login,
                    reason: "bye".to_string(),
                },
                ProtocolResponse::Frame(vec![4, 5]),
            ),
            (
                ProtocolRequest::EncodeEncryptionRequest {
                    server_id: String::new(),
                    public_key_der: vec![9, 8, 7, 6],
                    verify_token: vec![5, 4, 3, 2],
                },
                ProtocolResponse::Frame(vec![6, 7]),
            ),
            (
                ProtocolRequest::EncodeLoginSuccess {
                    player: sample_player(),
                },
                ProtocolResponse::Frame(vec![8, 9]),
            ),
            (
                ProtocolRequest::DecodePlay {
                    session: sample_protocol_session(),
                    frame: vec![10, 11],
                },
                ProtocolResponse::RuntimeCommand(Some(sample_command())),
            ),
            (
                ProtocolRequest::EncodePlayEvent {
                    session: sample_protocol_session(),
                    event: sample_event(),
                    context: PlayEncodingContext {
                        player_id: sample_player_id(),
                        entity_id: EntityId(7),
                    },
                },
                ProtocolResponse::Frames(vec![vec![12], vec![13, 14]]),
            ),
            (
                ProtocolRequest::ExportSessionState {
                    session: sample_protocol_session(),
                },
                ProtocolResponse::SessionTransferBlob(vec![15, 16]),
            ),
            (
                ProtocolRequest::ImportSessionState {
                    session: sample_protocol_session(),
                    blob: vec![17, 18],
                },
                ProtocolResponse::Empty,
            ),
            (
                ProtocolRequest::EncodeWireFrame {
                    payload: vec![0xaa, 0xbb],
                },
                ProtocolResponse::Frame(vec![0x02, 0xaa, 0xbb]),
            ),
            (
                ProtocolRequest::TryDecodeWireFrame {
                    buffer: vec![0x02, 0xcc, 0xdd, 0xee],
                },
                ProtocolResponse::WireFrameDecodeResult(Some(WireFrameDecodeResult {
                    frame: vec![0xcc, 0xdd],
                    bytes_consumed: 3,
                })),
            ),
        ]
    }

    #[test]
    fn protocol_header_rejects_wrong_version_kind_and_length() {
        let request = encode_protocol_request(&ProtocolRequest::Describe)
            .expect("describe request should encode");

        let mut wrong_version = request.clone();
        wrong_version[0..2].copy_from_slice(&2_u16.to_le_bytes());
        wrong_version[2..4].copy_from_slice(&0_u16.to_le_bytes());
        let error = decode_protocol_request(&wrong_version).expect_err("wrong version should fail");
        assert!(matches!(
            error,
            ProtocolCodecError::InvalidEnvelope("plugin ABI version mismatch")
        ));

        let mut wrong_kind = request.clone();
        wrong_kind[4] = 9;
        let error = decode_protocol_request(&wrong_kind).expect_err("wrong kind should fail");
        assert!(matches!(error, ProtocolCodecError::InvalidPluginKind(9)));

        let mut wrong_length = request;
        wrong_length[8] = 99;
        let error = decode_protocol_request(&wrong_length).expect_err("wrong length should fail");
        assert!(matches!(
            error,
            ProtocolCodecError::InvalidEnvelope("payload length did not match message size")
        ));
    }

    #[test]
    fn protocol_header_layout_is_stable() {
        let request = encode_protocol_request(&ProtocolRequest::Describe)
            .expect("describe request should encode");
        assert_eq!(request.len(), PLUGIN_ENVELOPE_HEADER_LEN);
        assert_eq!(&request[0..2], &CURRENT_PLUGIN_ABI.major.to_le_bytes());
        assert_eq!(&request[2..4], &CURRENT_PLUGIN_ABI.minor.to_le_bytes());
        assert_eq!(request[4], 1);
        assert_eq!(request[5], 1);
        assert_eq!(&request[6..8], &0_u16.to_le_bytes());
        assert_eq!(&request[8..12], &0_u32.to_le_bytes());
    }

    #[test]
    fn protocol_ops_round_trip_with_binary_codec() {
        for (request, response) in sample_protocol_round_trips() {
            let encoded_request = encode_protocol_request(&request).expect("request should encode");
            let decoded_request =
                decode_protocol_request(&encoded_request).expect("request should decode");
            assert_eq!(decoded_request, request);

            let encoded_response =
                encode_protocol_response(&request, &response).expect("response should encode");
            assert_eq!(
                u16::from_le_bytes([encoded_response[6], encoded_response[7]]),
                PROTOCOL_FLAG_RESPONSE
            );
            let decoded_response = decode_protocol_response(&request, &encoded_response)
                .expect("response should decode");
            assert_eq!(decoded_response, response);
        }
    }

    #[test]
    fn malformed_payloads_fail_deterministically() {
        let response = encode_protocol_response(
            &ProtocolRequest::DecodePlay {
                session: sample_protocol_session(),
                frame: vec![1],
            },
            &ProtocolResponse::RuntimeCommand(Some(sample_command())),
        )
        .expect("response should encode");
        let mut truncated = response.clone();
        let _ = truncated.pop();
        let error = decode_protocol_response(
            &ProtocolRequest::DecodePlay {
                session: sample_protocol_session(),
                frame: vec![1],
            },
            &truncated,
        )
        .expect_err("truncated payload should fail");
        assert!(matches!(
            error,
            ProtocolCodecError::InvalidEnvelope("payload length did not match message size")
        ));

        let mut bad_response_flag = response;
        bad_response_flag[6] = 0;
        bad_response_flag[7] = 0;
        let error = decode_protocol_response(
            &ProtocolRequest::DecodePlay {
                session: sample_protocol_session(),
                frame: vec![1],
            },
            &bad_response_flag,
        )
        .expect_err("missing response flag should fail");
        assert!(matches!(
            error,
            ProtocolCodecError::InvalidEnvelope("protocol response was missing response flag")
        ));
    }

    #[test]
    fn capability_and_session_support_types_round_trip() {
        let mut protocol = ProtocolCapabilitySet::new();
        let _ = protocol.insert(ProtocolCapability::Je340);
        let _ = protocol.insert(ProtocolCapability::Je404);
        let capability_set = SessionCapabilitySet {
            protocol,
            gameplay: GameplayCapabilitySet::new(),
            gameplay_profile: GameplayProfileId::new("canonical"),
            entity_id: Some(EntityId(7)),
            protocol_generation: Some(PluginGenerationId(3)),
            gameplay_generation: Some(PluginGenerationId(4)),
        };
        assert!(capability_set.protocol.contains(&ProtocolCapability::Je340));
        assert!(capability_set.protocol.contains(&ProtocolCapability::Je404));
        assert_eq!(capability_set.gameplay_profile.as_str(), "canonical");
        assert_eq!(capability_set.entity_id, Some(EntityId(7)));
        assert_eq!(
            capability_set.protocol_generation,
            Some(PluginGenerationId(3))
        );
        assert_eq!(
            capability_set.gameplay_generation,
            Some(PluginGenerationId(4))
        );
    }

    #[test]
    fn samples_cover_inventory_and_event_variants() {
        let event = CoreEvent::InventorySlotChanged {
            window_id: 0,
            container: player_container(),
            slot: InventorySlot::Offhand,
            stack: Some(ItemStack::new("minecraft:shield", 1, 0)),
        };
        let request = ProtocolRequest::EncodePlayEvent {
            session: sample_protocol_session(),
            event,
            context: PlayEncodingContext {
                player_id: sample_player_id(),
                entity_id: EntityId(1),
            },
        };
        let encoded_request =
            encode_protocol_request(&request).expect("inventory event request should encode");
        let decoded_request = decode_protocol_request(&encoded_request)
            .expect("inventory event request should decode");
        assert_eq!(decoded_request, request);

        let login_command = CoreCommand::LoginStart {
            connection_id: ConnectionId(1),
            username: "alice".to_string(),
            player_id: sample_player_id(),
        };
        let response = ProtocolResponse::RuntimeCommand(Some(RuntimeCommand::Core(login_command)));
        let decode_play = ProtocolRequest::DecodePlay {
            session: sample_protocol_session(),
            frame: vec![0x10],
        };
        let encoded_response =
            encode_protocol_response(&decode_play, &response).expect("command should encode");
        let decoded_response = decode_protocol_response(&decode_play, &encoded_response)
            .expect("command should decode");
        assert_eq!(decoded_response, response);
    }
}
