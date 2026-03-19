use crate::{CURRENT_PLUGIN_ABI, PluginAbiVersion, PluginKind};
use mc_core::{
    BlockFace, BlockPos, BlockState, CapabilitySet, ChunkColumn, ChunkSection, ConnectionId,
    CoreCommand, CoreEvent, DimensionId, EntityId, InteractionHand, InventoryContainer,
    InventorySlot, ItemStack, PlayerId, PlayerInventory, PlayerSnapshot, Vec3, WorldMeta,
    WorldSnapshot, expand_block_index,
};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeIntent, HandshakeNextState, LoginRequest,
    PlayEncodingContext, ProtocolDescriptor, ServerListStatus, StatusRequest, TransportKind,
};
use std::collections::BTreeMap;
use thiserror::Error;
use uuid::Uuid;

pub const PROTOCOL_FLAG_RESPONSE: u16 = 0x0001;
pub const PLUGIN_ENVELOPE_HEADER_LEN: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProtocolOpCode {
    Describe = 1,
    CapabilitySet = 2,
    TryRoute = 3,
    DecodeStatus = 4,
    DecodeLogin = 5,
    EncodeStatusResponse = 6,
    EncodeStatusPong = 7,
    EncodeDisconnect = 8,
    EncodeEncryptionRequest = 9,
    EncodeNetworkSettings = 10,
    EncodeLoginSuccess = 11,
    DecodePlay = 12,
    EncodePlayEvent = 13,
    ExportSessionState = 14,
    ImportSessionState = 15,
}

impl TryFrom<u8> for ProtocolOpCode {
    type Error = ProtocolCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Describe),
            2 => Ok(Self::CapabilitySet),
            3 => Ok(Self::TryRoute),
            4 => Ok(Self::DecodeStatus),
            5 => Ok(Self::DecodeLogin),
            6 => Ok(Self::EncodeStatusResponse),
            7 => Ok(Self::EncodeStatusPong),
            8 => Ok(Self::EncodeDisconnect),
            9 => Ok(Self::EncodeEncryptionRequest),
            10 => Ok(Self::EncodeNetworkSettings),
            11 => Ok(Self::EncodeLoginSuccess),
            12 => Ok(Self::DecodePlay),
            13 => Ok(Self::EncodePlayEvent),
            14 => Ok(Self::ExportSessionState),
            15 => Ok(Self::ImportSessionState),
            _ => Err(ProtocolCodecError::InvalidProtocolOpCode(value)),
        }
    }
}

#[derive(Debug, Error)]
pub enum ProtocolCodecError {
    #[error("unexpected end of plugin payload")]
    UnexpectedEof,
    #[error("invalid utf-8 in plugin payload")]
    InvalidUtf8,
    #[error("plugin envelope length overflow")]
    LengthOverflow,
    #[error("invalid plugin envelope: {0}")]
    InvalidEnvelope(&'static str),
    #[error("invalid plugin kind {0}")]
    InvalidPluginKind(u8),
    #[error("invalid protocol op code {0}")]
    InvalidProtocolOpCode(u8),
    #[error("invalid plugin payload value: {0}")]
    InvalidValue(&'static str),
    #[error("plugin payload had trailing bytes")]
    TrailingBytes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolSessionSnapshot {
    pub phase: ConnectionPhase,
    pub player_id: Option<PlayerId>,
    pub entity_id: Option<EntityId>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProtocolRequest {
    Describe,
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
        player_id: PlayerId,
        frame: Vec<u8>,
    },
    EncodePlayEvent {
        event: CoreEvent,
        context: PlayEncodingContext,
    },
    ExportSessionState {
        session: ProtocolSessionSnapshot,
    },
    ImportSessionState {
        session: ProtocolSessionSnapshot,
        blob: Vec<u8>,
    },
}

impl ProtocolRequest {
    #[must_use]
    pub const fn op_code(&self) -> ProtocolOpCode {
        match self {
            Self::Describe => ProtocolOpCode::Describe,
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
            Self::ExportSessionState { .. } => ProtocolOpCode::ExportSessionState,
            Self::ImportSessionState { .. } => ProtocolOpCode::ImportSessionState,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProtocolResponse {
    Descriptor(ProtocolDescriptor),
    CapabilitySet(CapabilitySet),
    HandshakeIntent(Option<HandshakeIntent>),
    StatusRequest(StatusRequest),
    LoginRequest(LoginRequest),
    Frame(Vec<u8>),
    Frames(Vec<Vec<u8>>),
    CoreCommand(Option<CoreCommand>),
    SessionTransferBlob(Vec<u8>),
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EnvelopeHeader {
    pub(crate) abi: PluginAbiVersion,
    pub(crate) plugin_kind: PluginKind,
    pub(crate) op_code: u8,
    pub(crate) flags: u16,
    pub(crate) payload_len: u32,
}

#[derive(Default)]
pub(crate) struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    pub(crate) fn with_header(header: EnvelopeHeader) -> Self {
        let mut encoder = Self::default();
        encoder.write_u16(header.abi.major);
        encoder.write_u16(header.abi.minor);
        encoder.write_u8(match header.plugin_kind {
            PluginKind::Protocol => 1,
            PluginKind::Storage => 2,
            PluginKind::Auth => 3,
            PluginKind::Gameplay => 4,
        });
        encoder.write_u8(header.op_code);
        encoder.write_u16(header.flags);
        encoder.write_u32(header.payload_len);
        encoder
    }

    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.bytes
    }

    pub(crate) fn write_u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub(crate) fn write_i8(&mut self, value: i8) {
        self.bytes.push(value.to_le_bytes()[0]);
    }

    pub(crate) fn write_bool(&mut self, value: bool) {
        self.write_u8(u8::from(value));
    }

    pub(crate) fn write_u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_i16(&mut self, value: i16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_f32(&mut self, value: f32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_f64(&mut self, value: f64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn write_len(&mut self, value: usize) -> Result<(), ProtocolCodecError> {
        let value = u32::try_from(value).map_err(|_| ProtocolCodecError::LengthOverflow)?;
        self.write_u32(value);
        Ok(())
    }

    pub(crate) fn write_string(&mut self, value: &str) -> Result<(), ProtocolCodecError> {
        self.write_bytes(value.as_bytes())
    }

    pub(crate) fn write_bytes(&mut self, value: &[u8]) -> Result<(), ProtocolCodecError> {
        self.write_len(value.len())?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }
}

pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    pub(crate) fn finish(&self) -> Result<(), ProtocolCodecError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(ProtocolCodecError::TrailingBytes)
        }
    }

    pub(crate) fn read_u8(&mut self) -> Result<u8, ProtocolCodecError> {
        let byte = *self
            .bytes
            .get(self.cursor)
            .ok_or(ProtocolCodecError::UnexpectedEof)?;
        self.cursor = self.cursor.saturating_add(1);
        Ok(byte)
    }

    pub(crate) fn read_i8(&mut self) -> Result<i8, ProtocolCodecError> {
        Ok(i8::from_le_bytes([self.read_u8()?]))
    }

    pub(crate) fn read_bool(&mut self) -> Result<bool, ProtocolCodecError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ProtocolCodecError::InvalidValue("invalid bool tag")),
        }
    }

    pub(crate) fn read_u16(&mut self) -> Result<u16, ProtocolCodecError> {
        Ok(u16::from_le_bytes(self.read_exact::<2>()?))
    }

    pub(crate) fn read_i16(&mut self) -> Result<i16, ProtocolCodecError> {
        Ok(i16::from_le_bytes(self.read_exact::<2>()?))
    }

    pub(crate) fn read_u32(&mut self) -> Result<u32, ProtocolCodecError> {
        Ok(u32::from_le_bytes(self.read_exact::<4>()?))
    }

    pub(crate) fn read_i32(&mut self) -> Result<i32, ProtocolCodecError> {
        Ok(i32::from_le_bytes(self.read_exact::<4>()?))
    }

    pub(crate) fn read_u64(&mut self) -> Result<u64, ProtocolCodecError> {
        Ok(u64::from_le_bytes(self.read_exact::<8>()?))
    }

    pub(crate) fn read_i64(&mut self) -> Result<i64, ProtocolCodecError> {
        Ok(i64::from_le_bytes(self.read_exact::<8>()?))
    }

    pub(crate) fn read_f32(&mut self) -> Result<f32, ProtocolCodecError> {
        Ok(f32::from_le_bytes(self.read_exact::<4>()?))
    }

    pub(crate) fn read_f64(&mut self) -> Result<f64, ProtocolCodecError> {
        Ok(f64::from_le_bytes(self.read_exact::<8>()?))
    }

    pub(crate) fn read_len(&mut self) -> Result<usize, ProtocolCodecError> {
        usize::try_from(self.read_u32()?).map_err(|_| ProtocolCodecError::LengthOverflow)
    }

    pub(crate) fn read_string(&mut self) -> Result<String, ProtocolCodecError> {
        let bytes = self.read_bytes()?;
        String::from_utf8(bytes).map_err(|_| ProtocolCodecError::InvalidUtf8)
    }

    pub(crate) fn read_bytes(&mut self) -> Result<Vec<u8>, ProtocolCodecError> {
        let len = self.read_len()?;
        let bytes = self.read_raw(len)?;
        Ok(bytes.to_vec())
    }

    pub(crate) fn read_raw(&mut self, len: usize) -> Result<&'a [u8], ProtocolCodecError> {
        let end = self.cursor.saturating_add(len);
        let slice = self
            .bytes
            .get(self.cursor..end)
            .ok_or(ProtocolCodecError::UnexpectedEof)?;
        self.cursor = end;
        Ok(slice)
    }

    pub(crate) fn read_exact<const N: usize>(&mut self) -> Result<[u8; N], ProtocolCodecError> {
        let bytes = self.read_raw(N)?;
        let mut array = [0_u8; N];
        array.copy_from_slice(bytes);
        Ok(array)
    }
}

pub fn encode_protocol_request(request: &ProtocolRequest) -> Result<Vec<u8>, ProtocolCodecError> {
    let mut payload = Encoder::default();
    encode_protocol_request_payload(&mut payload, request)?;
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::Protocol,
            op_code: request.op_code() as u8,
            flags: 0,
            payload_len: u32::try_from(payload.bytes.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        payload.into_inner(),
    )
}

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

pub fn encode_protocol_response(
    request: &ProtocolRequest,
    response: &ProtocolResponse,
) -> Result<Vec<u8>, ProtocolCodecError> {
    let mut payload = Encoder::default();
    encode_protocol_response_payload(&mut payload, request.op_code(), response)?;
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::Protocol,
            op_code: request.op_code() as u8,
            flags: PROTOCOL_FLAG_RESPONSE,
            payload_len: u32::try_from(payload.bytes.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        payload.into_inner(),
    )
}

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

pub(crate) fn encode_envelope(
    header: EnvelopeHeader,
    payload: Vec<u8>,
) -> Result<Vec<u8>, ProtocolCodecError> {
    if usize::try_from(header.payload_len).map_err(|_| ProtocolCodecError::LengthOverflow)?
        != payload.len()
    {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "payload length did not match header",
        ));
    }
    let mut encoder = Encoder::with_header(header);
    encoder.bytes.extend_from_slice(&payload);
    Ok(encoder.into_inner())
}

pub(crate) fn decode_envelope(bytes: &[u8]) -> Result<(EnvelopeHeader, &[u8]), ProtocolCodecError> {
    if bytes.len() < PLUGIN_ENVELOPE_HEADER_LEN {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "message shorter than header",
        ));
    }
    let mut decoder = Decoder::new(bytes);
    let abi = PluginAbiVersion {
        major: decoder.read_u16()?,
        minor: decoder.read_u16()?,
    };
    let plugin_kind = PluginKind::try_from(decoder.read_u8()?)?;
    let op_code = decoder.read_u8()?;
    let flags = decoder.read_u16()?;
    let payload_len = decoder.read_u32()?;
    if abi != CURRENT_PLUGIN_ABI {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "plugin ABI version mismatch",
        ));
    }
    let payload_len_usize =
        usize::try_from(payload_len).map_err(|_| ProtocolCodecError::LengthOverflow)?;
    if bytes.len() != PLUGIN_ENVELOPE_HEADER_LEN.saturating_add(payload_len_usize) {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "payload length did not match message size",
        ));
    }
    Ok((
        EnvelopeHeader {
            abi,
            plugin_kind,
            op_code,
            flags,
            payload_len,
        },
        &bytes[PLUGIN_ENVELOPE_HEADER_LEN..],
    ))
}

fn encode_protocol_request_payload(
    encoder: &mut Encoder,
    request: &ProtocolRequest,
) -> Result<(), ProtocolCodecError> {
    match request {
        ProtocolRequest::Describe | ProtocolRequest::CapabilitySet => Ok(()),
        ProtocolRequest::TryRoute { frame }
        | ProtocolRequest::DecodeStatus { frame }
        | ProtocolRequest::DecodeLogin { frame } => encoder.write_bytes(frame),
        ProtocolRequest::EncodeStatusResponse { status } => {
            encode_server_list_status(encoder, status)
        }
        ProtocolRequest::EncodeStatusPong { payload } => {
            encoder.write_i64(*payload);
            Ok(())
        }
        ProtocolRequest::EncodeDisconnect { phase, reason } => {
            encode_connection_phase(encoder, *phase);
            encoder.write_string(reason)
        }
        ProtocolRequest::EncodeEncryptionRequest {
            server_id,
            public_key_der,
            verify_token,
        } => {
            encoder.write_string(server_id)?;
            encoder.write_bytes(public_key_der)?;
            encoder.write_bytes(verify_token)
        }
        ProtocolRequest::EncodeNetworkSettings {
            compression_threshold,
        } => {
            encoder.write_u16(*compression_threshold);
            Ok(())
        }
        ProtocolRequest::EncodeLoginSuccess { player } => encode_player_snapshot(encoder, player),
        ProtocolRequest::DecodePlay { player_id, frame } => {
            encode_player_id(encoder, *player_id);
            encoder.write_bytes(frame)
        }
        ProtocolRequest::EncodePlayEvent { event, context } => {
            encode_core_event(encoder, event)?;
            encode_play_encoding_context(encoder, context);
            Ok(())
        }
        ProtocolRequest::ExportSessionState { session } => {
            encode_protocol_session_snapshot(encoder, session)
        }
        ProtocolRequest::ImportSessionState { session, blob } => {
            encode_protocol_session_snapshot(encoder, session)?;
            encoder.write_bytes(blob)
        }
    }
}

fn decode_protocol_request_payload(
    decoder: &mut Decoder<'_>,
    op_code: ProtocolOpCode,
) -> Result<ProtocolRequest, ProtocolCodecError> {
    match op_code {
        ProtocolOpCode::Describe => Ok(ProtocolRequest::Describe),
        ProtocolOpCode::CapabilitySet => Ok(ProtocolRequest::CapabilitySet),
        ProtocolOpCode::TryRoute => Ok(ProtocolRequest::TryRoute {
            frame: decoder.read_bytes()?,
        }),
        ProtocolOpCode::DecodeStatus => Ok(ProtocolRequest::DecodeStatus {
            frame: decoder.read_bytes()?,
        }),
        ProtocolOpCode::DecodeLogin => Ok(ProtocolRequest::DecodeLogin {
            frame: decoder.read_bytes()?,
        }),
        ProtocolOpCode::EncodeStatusResponse => Ok(ProtocolRequest::EncodeStatusResponse {
            status: decode_server_list_status(decoder)?,
        }),
        ProtocolOpCode::EncodeStatusPong => Ok(ProtocolRequest::EncodeStatusPong {
            payload: decoder.read_i64()?,
        }),
        ProtocolOpCode::EncodeDisconnect => Ok(ProtocolRequest::EncodeDisconnect {
            phase: decode_connection_phase(decoder)?,
            reason: decoder.read_string()?,
        }),
        ProtocolOpCode::EncodeEncryptionRequest => Ok(ProtocolRequest::EncodeEncryptionRequest {
            server_id: decoder.read_string()?,
            public_key_der: decoder.read_bytes()?,
            verify_token: decoder.read_bytes()?,
        }),
        ProtocolOpCode::EncodeNetworkSettings => Ok(ProtocolRequest::EncodeNetworkSettings {
            compression_threshold: decoder.read_u16()?,
        }),
        ProtocolOpCode::EncodeLoginSuccess => Ok(ProtocolRequest::EncodeLoginSuccess {
            player: decode_player_snapshot(decoder)?,
        }),
        ProtocolOpCode::DecodePlay => Ok(ProtocolRequest::DecodePlay {
            player_id: decode_player_id(decoder)?,
            frame: decoder.read_bytes()?,
        }),
        ProtocolOpCode::EncodePlayEvent => Ok(ProtocolRequest::EncodePlayEvent {
            event: decode_core_event(decoder)?,
            context: decode_play_encoding_context(decoder)?,
        }),
        ProtocolOpCode::ExportSessionState => Ok(ProtocolRequest::ExportSessionState {
            session: decode_protocol_session_snapshot(decoder)?,
        }),
        ProtocolOpCode::ImportSessionState => Ok(ProtocolRequest::ImportSessionState {
            session: decode_protocol_session_snapshot(decoder)?,
            blob: decoder.read_bytes()?,
        }),
    }
}

fn encode_protocol_response_payload(
    encoder: &mut Encoder,
    op_code: ProtocolOpCode,
    response: &ProtocolResponse,
) -> Result<(), ProtocolCodecError> {
    match (op_code, response) {
        (ProtocolOpCode::Describe, ProtocolResponse::Descriptor(descriptor)) => {
            encode_protocol_descriptor(encoder, descriptor)
        }
        (ProtocolOpCode::CapabilitySet, ProtocolResponse::CapabilitySet(capabilities)) => {
            encode_capability_set(encoder, capabilities)
        }
        (ProtocolOpCode::TryRoute, ProtocolResponse::HandshakeIntent(intent)) => {
            encode_option(encoder, intent.as_ref(), encode_handshake_intent)
        }
        (ProtocolOpCode::DecodeStatus, ProtocolResponse::StatusRequest(request)) => {
            encode_status_request(encoder, request)
        }
        (ProtocolOpCode::DecodeLogin, ProtocolResponse::LoginRequest(request)) => {
            encode_login_request(encoder, request)
        }
        (
            ProtocolOpCode::EncodeStatusResponse
            | ProtocolOpCode::EncodeStatusPong
            | ProtocolOpCode::EncodeDisconnect
            | ProtocolOpCode::EncodeEncryptionRequest
            | ProtocolOpCode::EncodeNetworkSettings
            | ProtocolOpCode::EncodeLoginSuccess,
            ProtocolResponse::Frame(frame),
        ) => encoder.write_bytes(frame),
        (ProtocolOpCode::DecodePlay, ProtocolResponse::CoreCommand(command)) => {
            encode_option(encoder, command.as_ref(), encode_core_command)
        }
        (ProtocolOpCode::EncodePlayEvent, ProtocolResponse::Frames(frames)) => {
            encoder.write_len(frames.len())?;
            for frame in frames {
                encoder.write_bytes(frame)?;
            }
            Ok(())
        }
        (ProtocolOpCode::ExportSessionState, ProtocolResponse::SessionTransferBlob(blob)) => {
            encoder.write_bytes(blob)
        }
        (ProtocolOpCode::ImportSessionState, ProtocolResponse::Empty) => Ok(()),
        _ => Err(ProtocolCodecError::InvalidValue(
            "protocol response did not match opcode",
        )),
    }
}

fn decode_protocol_response_payload(
    decoder: &mut Decoder<'_>,
    op_code: ProtocolOpCode,
) -> Result<ProtocolResponse, ProtocolCodecError> {
    match op_code {
        ProtocolOpCode::Describe => Ok(ProtocolResponse::Descriptor(decode_protocol_descriptor(
            decoder,
        )?)),
        ProtocolOpCode::CapabilitySet => Ok(ProtocolResponse::CapabilitySet(
            decode_capability_set(decoder)?,
        )),
        ProtocolOpCode::TryRoute => Ok(ProtocolResponse::HandshakeIntent(decode_option(
            decoder,
            decode_handshake_intent,
        )?)),
        ProtocolOpCode::DecodeStatus => Ok(ProtocolResponse::StatusRequest(decode_status_request(
            decoder,
        )?)),
        ProtocolOpCode::DecodeLogin => Ok(ProtocolResponse::LoginRequest(decode_login_request(
            decoder,
        )?)),
        ProtocolOpCode::EncodeStatusResponse
        | ProtocolOpCode::EncodeStatusPong
        | ProtocolOpCode::EncodeDisconnect
        | ProtocolOpCode::EncodeEncryptionRequest
        | ProtocolOpCode::EncodeNetworkSettings
        | ProtocolOpCode::EncodeLoginSuccess => Ok(ProtocolResponse::Frame(decoder.read_bytes()?)),
        ProtocolOpCode::DecodePlay => Ok(ProtocolResponse::CoreCommand(decode_option(
            decoder,
            decode_core_command,
        )?)),
        ProtocolOpCode::EncodePlayEvent => {
            let len = decoder.read_len()?;
            let mut frames = Vec::with_capacity(len);
            for _ in 0..len {
                frames.push(decoder.read_bytes()?);
            }
            Ok(ProtocolResponse::Frames(frames))
        }
        ProtocolOpCode::ExportSessionState => {
            Ok(ProtocolResponse::SessionTransferBlob(decoder.read_bytes()?))
        }
        ProtocolOpCode::ImportSessionState => Ok(ProtocolResponse::Empty),
    }
}

pub(crate) fn encode_option<T>(
    encoder: &mut Encoder,
    value: Option<&T>,
    encode: fn(&mut Encoder, &T) -> Result<(), ProtocolCodecError>,
) -> Result<(), ProtocolCodecError> {
    match value {
        Some(value) => {
            encoder.write_bool(true);
            encode(encoder, value)
        }
        None => {
            encoder.write_bool(false);
            Ok(())
        }
    }
}

pub(crate) fn decode_option<T>(
    decoder: &mut Decoder<'_>,
    decode: fn(&mut Decoder<'_>) -> Result<T, ProtocolCodecError>,
) -> Result<Option<T>, ProtocolCodecError> {
    if decoder.read_bool()? {
        Ok(Some(decode(decoder)?))
    } else {
        Ok(None)
    }
}

pub(crate) fn encode_connection_phase(encoder: &mut Encoder, phase: ConnectionPhase) {
    encoder.write_u8(match phase {
        ConnectionPhase::Handshaking => 1,
        ConnectionPhase::Status => 2,
        ConnectionPhase::Login => 3,
        ConnectionPhase::Play => 4,
    });
}

pub(crate) fn decode_connection_phase(
    decoder: &mut Decoder<'_>,
) -> Result<ConnectionPhase, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(ConnectionPhase::Handshaking),
        2 => Ok(ConnectionPhase::Status),
        3 => Ok(ConnectionPhase::Login),
        4 => Ok(ConnectionPhase::Play),
        _ => Err(ProtocolCodecError::InvalidValue("invalid connection phase")),
    }
}

fn encode_transport_kind(encoder: &mut Encoder, transport: TransportKind) {
    encoder.write_u8(match transport {
        TransportKind::Tcp => 1,
        TransportKind::Udp => 2,
    });
}

fn decode_transport_kind(decoder: &mut Decoder<'_>) -> Result<TransportKind, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(TransportKind::Tcp),
        2 => Ok(TransportKind::Udp),
        _ => Err(ProtocolCodecError::InvalidValue("invalid transport kind")),
    }
}

fn encode_edition(encoder: &mut Encoder, edition: Edition) {
    encoder.write_u8(match edition {
        Edition::Je => 1,
        Edition::Be => 2,
    });
}

fn decode_edition(decoder: &mut Decoder<'_>) -> Result<Edition, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(Edition::Je),
        2 => Ok(Edition::Be),
        _ => Err(ProtocolCodecError::InvalidValue("invalid edition")),
    }
}

fn encode_handshake_next_state(encoder: &mut Encoder, next_state: HandshakeNextState) {
    encoder.write_u8(match next_state {
        HandshakeNextState::Status => 1,
        HandshakeNextState::Login => 2,
    });
}

fn decode_handshake_next_state(
    decoder: &mut Decoder<'_>,
) -> Result<HandshakeNextState, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(HandshakeNextState::Status),
        2 => Ok(HandshakeNextState::Login),
        _ => Err(ProtocolCodecError::InvalidValue(
            "invalid handshake next state",
        )),
    }
}

fn encode_dimension_id(encoder: &mut Encoder, dimension: DimensionId) {
    encoder.write_u8(match dimension {
        DimensionId::Overworld => 1,
    });
}

fn decode_dimension_id(decoder: &mut Decoder<'_>) -> Result<DimensionId, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(DimensionId::Overworld),
        _ => Err(ProtocolCodecError::InvalidValue("invalid dimension id")),
    }
}

fn encode_interaction_hand(encoder: &mut Encoder, hand: InteractionHand) {
    encoder.write_u8(match hand {
        InteractionHand::Main => 1,
        InteractionHand::Offhand => 2,
    });
}

fn decode_interaction_hand(
    decoder: &mut Decoder<'_>,
) -> Result<InteractionHand, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(InteractionHand::Main),
        2 => Ok(InteractionHand::Offhand),
        _ => Err(ProtocolCodecError::InvalidValue("invalid interaction hand")),
    }
}

fn encode_block_face(encoder: &mut Encoder, face: BlockFace) {
    encoder.write_u8(match face {
        BlockFace::Bottom => 1,
        BlockFace::Top => 2,
        BlockFace::North => 3,
        BlockFace::South => 4,
        BlockFace::West => 5,
        BlockFace::East => 6,
    });
}

fn decode_block_face(decoder: &mut Decoder<'_>) -> Result<BlockFace, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(BlockFace::Bottom),
        2 => Ok(BlockFace::Top),
        3 => Ok(BlockFace::North),
        4 => Ok(BlockFace::South),
        5 => Ok(BlockFace::West),
        6 => Ok(BlockFace::East),
        _ => Err(ProtocolCodecError::InvalidValue("invalid block face")),
    }
}

fn encode_inventory_container(encoder: &mut Encoder, container: InventoryContainer) {
    encoder.write_u8(match container {
        InventoryContainer::Player => 1,
    });
}

fn decode_inventory_container(
    decoder: &mut Decoder<'_>,
) -> Result<InventoryContainer, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(InventoryContainer::Player),
        _ => Err(ProtocolCodecError::InvalidValue(
            "invalid inventory container",
        )),
    }
}

pub(crate) fn encode_inventory_slot(encoder: &mut Encoder, slot: InventorySlot) {
    match slot {
        InventorySlot::Auxiliary(index) => {
            encoder.write_u8(1);
            encoder.write_u8(index);
        }
        InventorySlot::MainInventory(index) => {
            encoder.write_u8(2);
            encoder.write_u8(index);
        }
        InventorySlot::Hotbar(index) => {
            encoder.write_u8(3);
            encoder.write_u8(index);
        }
        InventorySlot::Offhand => {
            encoder.write_u8(4);
        }
    }
}

pub(crate) fn decode_inventory_slot(
    decoder: &mut Decoder<'_>,
) -> Result<InventorySlot, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(InventorySlot::Auxiliary(decoder.read_u8()?)),
        2 => Ok(InventorySlot::MainInventory(decoder.read_u8()?)),
        3 => Ok(InventorySlot::Hotbar(decoder.read_u8()?)),
        4 => Ok(InventorySlot::Offhand),
        _ => Err(ProtocolCodecError::InvalidValue("invalid inventory slot")),
    }
}

pub(crate) fn encode_player_id(encoder: &mut Encoder, player_id: PlayerId) {
    encoder.bytes.extend_from_slice(player_id.0.as_bytes());
}

pub(crate) fn decode_player_id(decoder: &mut Decoder<'_>) -> Result<PlayerId, ProtocolCodecError> {
    let bytes = decoder.read_exact::<16>()?;
    Ok(PlayerId(Uuid::from_bytes(bytes)))
}

pub(crate) fn encode_entity_id(encoder: &mut Encoder, entity_id: EntityId) {
    encoder.write_i32(entity_id.0);
}

pub(crate) fn decode_entity_id(decoder: &mut Decoder<'_>) -> Result<EntityId, ProtocolCodecError> {
    Ok(EntityId(decoder.read_i32()?))
}

fn encode_connection_id(encoder: &mut Encoder, connection_id: ConnectionId) {
    encoder.write_u64(connection_id.0);
}

fn decode_connection_id(decoder: &mut Decoder<'_>) -> Result<ConnectionId, ProtocolCodecError> {
    Ok(ConnectionId(decoder.read_u64()?))
}

fn encode_protocol_descriptor(
    encoder: &mut Encoder,
    descriptor: &ProtocolDescriptor,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(&descriptor.adapter_id)?;
    encode_transport_kind(encoder, descriptor.transport);
    encode_edition(encoder, descriptor.edition);
    encoder.write_string(&descriptor.version_name)?;
    encoder.write_i32(descriptor.protocol_number);
    Ok(())
}

fn decode_protocol_descriptor(
    decoder: &mut Decoder<'_>,
) -> Result<ProtocolDescriptor, ProtocolCodecError> {
    Ok(ProtocolDescriptor {
        adapter_id: decoder.read_string()?,
        transport: decode_transport_kind(decoder)?,
        edition: decode_edition(decoder)?,
        version_name: decoder.read_string()?,
        protocol_number: decoder.read_i32()?,
    })
}

pub(crate) fn encode_capability_set(
    encoder: &mut Encoder,
    capability_set: &CapabilitySet,
) -> Result<(), ProtocolCodecError> {
    let capabilities = capability_set.iter().collect::<Vec<_>>();
    encoder.write_len(capabilities.len())?;
    for capability in capabilities {
        encoder.write_string(capability)?;
    }
    Ok(())
}

pub(crate) fn decode_capability_set(
    decoder: &mut Decoder<'_>,
) -> Result<CapabilitySet, ProtocolCodecError> {
    let len = decoder.read_len()?;
    let mut capabilities = CapabilitySet::new();
    for _ in 0..len {
        let _ = capabilities.insert(decoder.read_string()?);
    }
    Ok(capabilities)
}

fn encode_handshake_intent(
    encoder: &mut Encoder,
    intent: &HandshakeIntent,
) -> Result<(), ProtocolCodecError> {
    encode_edition(encoder, intent.edition);
    encoder.write_i32(intent.protocol_number);
    encoder.write_string(&intent.server_host)?;
    encoder.write_u16(intent.server_port);
    encode_handshake_next_state(encoder, intent.next_state);
    Ok(())
}

fn decode_handshake_intent(
    decoder: &mut Decoder<'_>,
) -> Result<HandshakeIntent, ProtocolCodecError> {
    Ok(HandshakeIntent {
        edition: decode_edition(decoder)?,
        protocol_number: decoder.read_i32()?,
        server_host: decoder.read_string()?,
        server_port: decoder.read_u16()?,
        next_state: decode_handshake_next_state(decoder)?,
    })
}

fn encode_status_request(
    encoder: &mut Encoder,
    request: &StatusRequest,
) -> Result<(), ProtocolCodecError> {
    match request {
        StatusRequest::Query => {
            encoder.write_u8(1);
            Ok(())
        }
        StatusRequest::Ping { payload } => {
            encoder.write_u8(2);
            encoder.write_i64(*payload);
            Ok(())
        }
    }
}

fn decode_status_request(decoder: &mut Decoder<'_>) -> Result<StatusRequest, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(StatusRequest::Query),
        2 => Ok(StatusRequest::Ping {
            payload: decoder.read_i64()?,
        }),
        _ => Err(ProtocolCodecError::InvalidValue("invalid status request")),
    }
}

fn encode_login_request(
    encoder: &mut Encoder,
    request: &LoginRequest,
) -> Result<(), ProtocolCodecError> {
    match request {
        LoginRequest::LoginStart { username } => {
            encoder.write_u8(1);
            encoder.write_string(username)?;
            Ok(())
        }
        LoginRequest::EncryptionResponse {
            shared_secret_encrypted,
            verify_token_encrypted,
        } => {
            encoder.write_u8(2);
            encoder.write_bytes(shared_secret_encrypted)?;
            encoder.write_bytes(verify_token_encrypted)
        }
        LoginRequest::BedrockNetworkSettingsRequest { protocol_number } => {
            encoder.write_u8(3);
            encoder.write_i32(*protocol_number);
            Ok(())
        }
        LoginRequest::BedrockLogin {
            protocol_number,
            display_name,
            chain_jwts,
            client_data_jwt,
        } => {
            encoder.write_u8(4);
            encoder.write_i32(*protocol_number);
            encoder.write_string(display_name)?;
            encoder.write_len(chain_jwts.len())?;
            for jwt in chain_jwts {
                encoder.write_string(jwt)?;
            }
            encoder.write_string(client_data_jwt)
        }
    }
}

fn decode_login_request(decoder: &mut Decoder<'_>) -> Result<LoginRequest, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(LoginRequest::LoginStart {
            username: decoder.read_string()?,
        }),
        2 => Ok(LoginRequest::EncryptionResponse {
            shared_secret_encrypted: decoder.read_bytes()?,
            verify_token_encrypted: decoder.read_bytes()?,
        }),
        3 => Ok(LoginRequest::BedrockNetworkSettingsRequest {
            protocol_number: decoder.read_i32()?,
        }),
        4 => {
            let protocol_number = decoder.read_i32()?;
            let display_name = decoder.read_string()?;
            let chain_len = decoder.read_len()?;
            let mut chain_jwts = Vec::with_capacity(chain_len);
            for _ in 0..chain_len {
                chain_jwts.push(decoder.read_string()?);
            }
            Ok(LoginRequest::BedrockLogin {
                protocol_number,
                display_name,
                chain_jwts,
                client_data_jwt: decoder.read_string()?,
            })
        }
        _ => Err(ProtocolCodecError::InvalidValue("invalid login request")),
    }
}

fn encode_server_list_status(
    encoder: &mut Encoder,
    status: &ServerListStatus,
) -> Result<(), ProtocolCodecError> {
    encode_protocol_descriptor(encoder, &status.version)?;
    encoder.write_len(status.players_online)?;
    encoder.write_len(status.max_players)?;
    encoder.write_string(&status.description)?;
    Ok(())
}

fn decode_server_list_status(
    decoder: &mut Decoder<'_>,
) -> Result<ServerListStatus, ProtocolCodecError> {
    Ok(ServerListStatus {
        version: decode_protocol_descriptor(decoder)?,
        players_online: decoder.read_len()?,
        max_players: decoder.read_len()?,
        description: decoder.read_string()?,
    })
}

fn encode_play_encoding_context(encoder: &mut Encoder, context: &PlayEncodingContext) {
    encode_player_id(encoder, context.player_id);
    encode_entity_id(encoder, context.entity_id);
}

fn decode_play_encoding_context(
    decoder: &mut Decoder<'_>,
) -> Result<PlayEncodingContext, ProtocolCodecError> {
    Ok(PlayEncodingContext {
        player_id: decode_player_id(decoder)?,
        entity_id: decode_entity_id(decoder)?,
    })
}

fn encode_protocol_session_snapshot(
    encoder: &mut Encoder,
    snapshot: &ProtocolSessionSnapshot,
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
    )
}

fn decode_protocol_session_snapshot(
    decoder: &mut Decoder<'_>,
) -> Result<ProtocolSessionSnapshot, ProtocolCodecError> {
    Ok(ProtocolSessionSnapshot {
        phase: decode_connection_phase(decoder)?,
        player_id: decode_option(decoder, decode_player_id)?,
        entity_id: decode_option(decoder, decode_entity_id)?,
    })
}

pub(crate) fn encode_item_stack(
    encoder: &mut Encoder,
    stack: &ItemStack,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(stack.key.as_str())?;
    encoder.write_u8(stack.count);
    encoder.write_u16(stack.damage);
    Ok(())
}

pub(crate) fn decode_item_stack(
    decoder: &mut Decoder<'_>,
) -> Result<ItemStack, ProtocolCodecError> {
    Ok(ItemStack::new(
        decoder.read_string()?,
        decoder.read_u8()?,
        decoder.read_u16()?,
    ))
}

fn encode_player_inventory(
    encoder: &mut Encoder,
    inventory: &PlayerInventory,
) -> Result<(), ProtocolCodecError> {
    encoder.write_len(inventory.slots.len())?;
    for stack in &inventory.slots {
        encode_option(encoder, stack.as_ref(), encode_item_stack)?;
    }
    encode_option(encoder, inventory.offhand.as_ref(), encode_item_stack)
}

fn decode_player_inventory(
    decoder: &mut Decoder<'_>,
) -> Result<PlayerInventory, ProtocolCodecError> {
    let len = decoder.read_len()?;
    let mut slots = Vec::with_capacity(len);
    for _ in 0..len {
        slots.push(decode_option(decoder, decode_item_stack)?);
    }
    Ok(PlayerInventory {
        slots,
        offhand: decode_option(decoder, decode_item_stack)?,
    })
}

pub(crate) fn encode_block_pos(encoder: &mut Encoder, position: BlockPos) {
    encoder.write_i32(position.x);
    encoder.write_i32(position.y);
    encoder.write_i32(position.z);
}

pub(crate) fn decode_block_pos(decoder: &mut Decoder<'_>) -> Result<BlockPos, ProtocolCodecError> {
    Ok(BlockPos::new(
        decoder.read_i32()?,
        decoder.read_i32()?,
        decoder.read_i32()?,
    ))
}

fn encode_vec3(encoder: &mut Encoder, position: Vec3) {
    encoder.write_f64(position.x);
    encoder.write_f64(position.y);
    encoder.write_f64(position.z);
}

fn decode_vec3(decoder: &mut Decoder<'_>) -> Result<Vec3, ProtocolCodecError> {
    Ok(Vec3::new(
        decoder.read_f64()?,
        decoder.read_f64()?,
        decoder.read_f64()?,
    ))
}

pub(crate) fn encode_block_state(
    encoder: &mut Encoder,
    block_state: &BlockState,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(block_state.key.as_str())?;
    encoder.write_len(block_state.properties.len())?;
    for (key, value) in &block_state.properties {
        encoder.write_string(key)?;
        encoder.write_string(value)?;
    }
    Ok(())
}

pub(crate) fn decode_block_state(
    decoder: &mut Decoder<'_>,
) -> Result<BlockState, ProtocolCodecError> {
    let key = decoder.read_string()?;
    let len = decoder.read_len()?;
    let mut properties = BTreeMap::new();
    for _ in 0..len {
        let key = decoder.read_string()?;
        let value = decoder.read_string()?;
        properties.insert(key, value);
    }
    Ok(BlockState {
        key: mc_core::BlockKey::new(key),
        properties,
    })
}

fn encode_chunk_section(
    encoder: &mut Encoder,
    section: &ChunkSection,
) -> Result<(), ProtocolCodecError> {
    encoder.write_i32(section.y);
    let blocks = section.iter_blocks().collect::<Vec<_>>();
    encoder.write_len(blocks.len())?;
    for (index, state) in blocks {
        encoder.write_u16(index);
        encode_block_state(encoder, state)?;
    }
    Ok(())
}

fn decode_chunk_section(decoder: &mut Decoder<'_>) -> Result<ChunkSection, ProtocolCodecError> {
    let section_y = decoder.read_i32()?;
    let block_len = decoder.read_len()?;
    let mut section = ChunkSection::new(section_y);
    for _ in 0..block_len {
        let index = decoder.read_u16()?;
        let state = decode_block_state(decoder)?;
        let (x, y, z) = expand_block_index(index);
        section.set_block(x, y, z, state);
    }
    Ok(section)
}

pub(crate) fn encode_chunk_column(
    encoder: &mut Encoder,
    chunk: &ChunkColumn,
) -> Result<(), ProtocolCodecError> {
    encoder.write_i32(chunk.pos.x);
    encoder.write_i32(chunk.pos.z);
    encoder.write_len(chunk.sections.len())?;
    for section in chunk.sections.values() {
        encode_chunk_section(encoder, section)?;
    }
    encoder.write_bytes(&chunk.biomes)?;
    Ok(())
}

pub(crate) fn decode_chunk_column(
    decoder: &mut Decoder<'_>,
) -> Result<ChunkColumn, ProtocolCodecError> {
    let chunk_pos = mc_core::ChunkPos::new(decoder.read_i32()?, decoder.read_i32()?);
    let section_len = decoder.read_len()?;
    let mut sections = BTreeMap::new();
    for _ in 0..section_len {
        let section = decode_chunk_section(decoder)?;
        sections.insert(section.y, section);
    }
    Ok(ChunkColumn {
        pos: chunk_pos,
        sections,
        biomes: decoder.read_bytes()?,
    })
}

pub(crate) fn encode_world_meta(
    encoder: &mut Encoder,
    meta: &WorldMeta,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(&meta.level_name)?;
    encoder.write_u64(meta.seed);
    encode_block_pos(encoder, meta.spawn);
    encode_dimension_id(encoder, meta.dimension);
    encoder.write_i64(meta.age);
    encoder.write_i64(meta.time);
    encoder.write_string(&meta.level_type)?;
    encoder.write_u8(meta.game_mode);
    encoder.write_u8(meta.difficulty);
    encoder.write_u8(meta.max_players);
    Ok(())
}

pub(crate) fn decode_world_meta(
    decoder: &mut Decoder<'_>,
) -> Result<WorldMeta, ProtocolCodecError> {
    Ok(WorldMeta {
        level_name: decoder.read_string()?,
        seed: decoder.read_u64()?,
        spawn: decode_block_pos(decoder)?,
        dimension: decode_dimension_id(decoder)?,
        age: decoder.read_i64()?,
        time: decoder.read_i64()?,
        level_type: decoder.read_string()?,
        game_mode: decoder.read_u8()?,
        difficulty: decoder.read_u8()?,
        max_players: decoder.read_u8()?,
    })
}

pub(crate) fn encode_world_snapshot(
    encoder: &mut Encoder,
    snapshot: &WorldSnapshot,
) -> Result<(), ProtocolCodecError> {
    encode_world_meta(encoder, &snapshot.meta)?;
    encoder.write_len(snapshot.chunks.len())?;
    for chunk in snapshot.chunks.values() {
        encode_chunk_column(encoder, chunk)?;
    }
    encoder.write_len(snapshot.players.len())?;
    for player in snapshot.players.values() {
        encode_player_snapshot(encoder, player)?;
    }
    Ok(())
}

pub(crate) fn decode_world_snapshot(
    decoder: &mut Decoder<'_>,
) -> Result<WorldSnapshot, ProtocolCodecError> {
    let meta = decode_world_meta(decoder)?;
    let chunk_len = decoder.read_len()?;
    let mut chunks = BTreeMap::new();
    for _ in 0..chunk_len {
        let chunk = decode_chunk_column(decoder)?;
        chunks.insert(chunk.pos, chunk);
    }
    let player_len = decoder.read_len()?;
    let mut players = BTreeMap::new();
    for _ in 0..player_len {
        let player = decode_player_snapshot(decoder)?;
        players.insert(player.id, player);
    }
    Ok(WorldSnapshot {
        meta,
        chunks,
        players,
    })
}

pub(crate) fn encode_player_snapshot(
    encoder: &mut Encoder,
    player: &PlayerSnapshot,
) -> Result<(), ProtocolCodecError> {
    encode_player_id(encoder, player.id);
    encoder.write_string(&player.username)?;
    encode_vec3(encoder, player.position);
    encoder.write_f32(player.yaw);
    encoder.write_f32(player.pitch);
    encoder.write_bool(player.on_ground);
    encode_dimension_id(encoder, player.dimension);
    encoder.write_f32(player.health);
    encoder.write_i16(player.food);
    encoder.write_f32(player.food_saturation);
    encode_player_inventory(encoder, &player.inventory)?;
    encoder.write_u8(player.selected_hotbar_slot);
    Ok(())
}

pub(crate) fn decode_player_snapshot(
    decoder: &mut Decoder<'_>,
) -> Result<PlayerSnapshot, ProtocolCodecError> {
    Ok(PlayerSnapshot {
        id: decode_player_id(decoder)?,
        username: decoder.read_string()?,
        position: decode_vec3(decoder)?,
        yaw: decoder.read_f32()?,
        pitch: decoder.read_f32()?,
        on_ground: decoder.read_bool()?,
        dimension: decode_dimension_id(decoder)?,
        health: decoder.read_f32()?,
        food: decoder.read_i16()?,
        food_saturation: decoder.read_f32()?,
        inventory: decode_player_inventory(decoder)?,
        selected_hotbar_slot: decoder.read_u8()?,
    })
}

pub(crate) fn encode_core_command(
    encoder: &mut Encoder,
    command: &CoreCommand,
) -> Result<(), ProtocolCodecError> {
    match command {
        CoreCommand::LoginStart {
            connection_id,
            username,
            player_id,
        } => {
            encoder.write_u8(1);
            encode_connection_id(encoder, *connection_id);
            encoder.write_string(username)?;
            encode_player_id(encoder, *player_id);
        }
        CoreCommand::UpdateClientView {
            player_id,
            view_distance,
        } => {
            encoder.write_u8(2);
            encode_player_id(encoder, *player_id);
            encoder.write_u8(*view_distance);
        }
        CoreCommand::ClientStatus {
            player_id,
            action_id,
        } => {
            encoder.write_u8(3);
            encode_player_id(encoder, *player_id);
            encoder.write_i8(*action_id);
        }
        CoreCommand::MoveIntent {
            player_id,
            position,
            yaw,
            pitch,
            on_ground,
        } => {
            encoder.write_u8(4);
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
        CoreCommand::KeepAliveResponse {
            player_id,
            keep_alive_id,
        } => {
            encoder.write_u8(5);
            encode_player_id(encoder, *player_id);
            encoder.write_i32(*keep_alive_id);
        }
        CoreCommand::SetHeldSlot { player_id, slot } => {
            encoder.write_u8(6);
            encode_player_id(encoder, *player_id);
            encoder.write_i16(*slot);
        }
        CoreCommand::CreativeInventorySet {
            player_id,
            slot,
            stack,
        } => {
            encoder.write_u8(7);
            encode_player_id(encoder, *player_id);
            encode_inventory_slot(encoder, *slot);
            encode_option(encoder, stack.as_ref(), encode_item_stack)?;
        }
        CoreCommand::DigBlock {
            player_id,
            position,
            status,
            face,
        } => {
            encoder.write_u8(8);
            encode_player_id(encoder, *player_id);
            encode_block_pos(encoder, *position);
            encoder.write_u8(*status);
            encode_option(encoder, face.as_ref(), |encoder, face| {
                encode_block_face(encoder, *face);
                Ok(())
            })?;
        }
        CoreCommand::PlaceBlock {
            player_id,
            hand,
            position,
            face,
            held_item,
        } => {
            encoder.write_u8(9);
            encode_player_id(encoder, *player_id);
            encode_interaction_hand(encoder, *hand);
            encode_block_pos(encoder, *position);
            encode_option(encoder, face.as_ref(), |encoder, face| {
                encode_block_face(encoder, *face);
                Ok(())
            })?;
            encode_option(encoder, held_item.as_ref(), encode_item_stack)?;
        }
        CoreCommand::Disconnect { player_id } => {
            encoder.write_u8(10);
            encode_player_id(encoder, *player_id);
        }
    }
    Ok(())
}

pub(crate) fn decode_core_command(
    decoder: &mut Decoder<'_>,
) -> Result<CoreCommand, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(CoreCommand::LoginStart {
            connection_id: decode_connection_id(decoder)?,
            username: decoder.read_string()?,
            player_id: decode_player_id(decoder)?,
        }),
        2 => Ok(CoreCommand::UpdateClientView {
            player_id: decode_player_id(decoder)?,
            view_distance: decoder.read_u8()?,
        }),
        3 => Ok(CoreCommand::ClientStatus {
            player_id: decode_player_id(decoder)?,
            action_id: decoder.read_i8()?,
        }),
        4 => Ok(CoreCommand::MoveIntent {
            player_id: decode_player_id(decoder)?,
            position: decode_option(decoder, decode_vec3)?,
            yaw: decode_option(decoder, |decoder| decoder.read_f32())?,
            pitch: decode_option(decoder, |decoder| decoder.read_f32())?,
            on_ground: decoder.read_bool()?,
        }),
        5 => Ok(CoreCommand::KeepAliveResponse {
            player_id: decode_player_id(decoder)?,
            keep_alive_id: decoder.read_i32()?,
        }),
        6 => Ok(CoreCommand::SetHeldSlot {
            player_id: decode_player_id(decoder)?,
            slot: decoder.read_i16()?,
        }),
        7 => Ok(CoreCommand::CreativeInventorySet {
            player_id: decode_player_id(decoder)?,
            slot: decode_inventory_slot(decoder)?,
            stack: decode_option(decoder, decode_item_stack)?,
        }),
        8 => Ok(CoreCommand::DigBlock {
            player_id: decode_player_id(decoder)?,
            position: decode_block_pos(decoder)?,
            status: decoder.read_u8()?,
            face: decode_option(decoder, decode_block_face)?,
        }),
        9 => Ok(CoreCommand::PlaceBlock {
            player_id: decode_player_id(decoder)?,
            hand: decode_interaction_hand(decoder)?,
            position: decode_block_pos(decoder)?,
            face: decode_option(decoder, decode_block_face)?,
            held_item: decode_option(decoder, decode_item_stack)?,
        }),
        10 => Ok(CoreCommand::Disconnect {
            player_id: decode_player_id(decoder)?,
        }),
        _ => Err(ProtocolCodecError::InvalidValue("invalid core command tag")),
    }
}

pub(crate) fn encode_core_event(
    encoder: &mut Encoder,
    event: &CoreEvent,
) -> Result<(), ProtocolCodecError> {
    match event {
        CoreEvent::LoginAccepted {
            player_id,
            entity_id,
            player,
        } => {
            encoder.write_u8(1);
            encode_player_id(encoder, *player_id);
            encode_entity_id(encoder, *entity_id);
            encode_player_snapshot(encoder, player)?;
        }
        CoreEvent::PlayBootstrap {
            player,
            entity_id,
            world_meta,
            view_distance,
        } => {
            encoder.write_u8(2);
            encode_player_snapshot(encoder, player)?;
            encode_entity_id(encoder, *entity_id);
            encode_world_meta(encoder, world_meta)?;
            encoder.write_u8(*view_distance);
        }
        CoreEvent::ChunkBatch { chunks } => {
            encoder.write_u8(3);
            encoder.write_len(chunks.len())?;
            for chunk in chunks {
                encode_chunk_column(encoder, chunk)?;
            }
        }
        CoreEvent::EntitySpawned { entity_id, player } => {
            encoder.write_u8(4);
            encode_entity_id(encoder, *entity_id);
            encode_player_snapshot(encoder, player)?;
        }
        CoreEvent::EntityMoved { entity_id, player } => {
            encoder.write_u8(5);
            encode_entity_id(encoder, *entity_id);
            encode_player_snapshot(encoder, player)?;
        }
        CoreEvent::EntityDespawned { entity_ids } => {
            encoder.write_u8(6);
            encoder.write_len(entity_ids.len())?;
            for entity_id in entity_ids {
                encode_entity_id(encoder, *entity_id);
            }
        }
        CoreEvent::InventoryContents {
            container,
            inventory,
        } => {
            encoder.write_u8(7);
            encode_inventory_container(encoder, *container);
            encode_player_inventory(encoder, inventory)?;
        }
        CoreEvent::InventorySlotChanged {
            container,
            slot,
            stack,
        } => {
            encoder.write_u8(8);
            encode_inventory_container(encoder, *container);
            encode_inventory_slot(encoder, *slot);
            encode_option(encoder, stack.as_ref(), encode_item_stack)?;
        }
        CoreEvent::SelectedHotbarSlotChanged { slot } => {
            encoder.write_u8(9);
            encoder.write_u8(*slot);
        }
        CoreEvent::BlockChanged { position, block } => {
            encoder.write_u8(10);
            encode_block_pos(encoder, *position);
            encode_block_state(encoder, block)?;
        }
        CoreEvent::KeepAliveRequested { keep_alive_id } => {
            encoder.write_u8(11);
            encoder.write_i32(*keep_alive_id);
        }
        CoreEvent::Disconnect { reason } => {
            encoder.write_u8(12);
            encoder.write_string(reason)?;
        }
    }
    Ok(())
}

pub(crate) fn decode_core_event(
    decoder: &mut Decoder<'_>,
) -> Result<CoreEvent, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(CoreEvent::LoginAccepted {
            player_id: decode_player_id(decoder)?,
            entity_id: decode_entity_id(decoder)?,
            player: decode_player_snapshot(decoder)?,
        }),
        2 => Ok(CoreEvent::PlayBootstrap {
            player: decode_player_snapshot(decoder)?,
            entity_id: decode_entity_id(decoder)?,
            world_meta: decode_world_meta(decoder)?,
            view_distance: decoder.read_u8()?,
        }),
        3 => {
            let len = decoder.read_len()?;
            let mut chunks = Vec::with_capacity(len);
            for _ in 0..len {
                chunks.push(decode_chunk_column(decoder)?);
            }
            Ok(CoreEvent::ChunkBatch { chunks })
        }
        4 => Ok(CoreEvent::EntitySpawned {
            entity_id: decode_entity_id(decoder)?,
            player: decode_player_snapshot(decoder)?,
        }),
        5 => Ok(CoreEvent::EntityMoved {
            entity_id: decode_entity_id(decoder)?,
            player: decode_player_snapshot(decoder)?,
        }),
        6 => {
            let len = decoder.read_len()?;
            let mut entity_ids = Vec::with_capacity(len);
            for _ in 0..len {
                entity_ids.push(decode_entity_id(decoder)?);
            }
            Ok(CoreEvent::EntityDespawned { entity_ids })
        }
        7 => Ok(CoreEvent::InventoryContents {
            container: decode_inventory_container(decoder)?,
            inventory: decode_player_inventory(decoder)?,
        }),
        8 => Ok(CoreEvent::InventorySlotChanged {
            container: decode_inventory_container(decoder)?,
            slot: decode_inventory_slot(decoder)?,
            stack: decode_option(decoder, decode_item_stack)?,
        }),
        9 => Ok(CoreEvent::SelectedHotbarSlotChanged {
            slot: decoder.read_u8()?,
        }),
        10 => Ok(CoreEvent::BlockChanged {
            position: decode_block_pos(decoder)?,
            block: decode_block_state(decoder)?,
        }),
        11 => Ok(CoreEvent::KeepAliveRequested {
            keep_alive_id: decoder.read_i32()?,
        }),
        12 => Ok(CoreEvent::Disconnect {
            reason: decoder.read_string()?,
        }),
        _ => Err(ProtocolCodecError::InvalidValue("invalid core event tag")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CURRENT_PLUGIN_ABI, PLUGIN_ENVELOPE_HEADER_LEN, PROTOCOL_FLAG_RESPONSE, ProtocolCodecError,
        ProtocolRequest, ProtocolResponse, decode_protocol_request, decode_protocol_response,
        encode_protocol_request, encode_protocol_response,
    };
    use mc_core::{
        BlockPos, CapabilitySet, ChunkColumn, ConnectionId, CoreCommand, CoreEvent, EntityId,
        GameplayProfileId, InventoryContainer, InventorySlot, ItemStack, PlayerId, PlayerInventory,
        PlayerSnapshot, PluginGenerationId, SessionCapabilitySet, Vec3, WorldMeta,
    };
    use mc_proto_common::{
        ConnectionPhase, Edition, HandshakeIntent, HandshakeNextState, LoginRequest,
        PlayEncodingContext, ProtocolDescriptor, ServerListStatus, StatusRequest, TransportKind,
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

    fn sample_descriptor() -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: "je-1_7_10".to_string(),
            transport: TransportKind::Tcp,
            edition: Edition::Je,
            version_name: "1.7.10".to_string(),
            protocol_number: 5,
        }
    }

    fn sample_event() -> CoreEvent {
        let mut chunk = ChunkColumn::new(mc_core::ChunkPos::new(0, 0));
        chunk.set_block(1, 64, 1, mc_core::BlockState::stone());
        CoreEvent::PlayBootstrap {
            player: sample_player(),
            entity_id: EntityId(7),
            world_meta: sample_world_meta(),
            view_distance: 2,
        }
    }

    fn sample_command() -> CoreCommand {
        CoreCommand::CreativeInventorySet {
            player_id: sample_player_id(),
            slot: InventorySlot::Hotbar(1),
            stack: Some(ItemStack::new("minecraft:glass", 16, 0)),
        }
    }

    #[test]
    fn protocol_header_rejects_wrong_version_kind_and_length() {
        let request = encode_protocol_request(&ProtocolRequest::Describe)
            .expect("describe request should encode");

        let mut wrong_version = request.clone();
        wrong_version[0] = 9;
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
        let mut capabilities = CapabilitySet::new();
        let _ = capabilities.insert("protocol.je");
        let _ = capabilities.insert("runtime.reload.protocol");
        let player = sample_player();
        let descriptor = sample_descriptor();
        let requests_and_responses = vec![
            (
                ProtocolRequest::Describe,
                ProtocolResponse::Descriptor(descriptor.clone()),
            ),
            (
                ProtocolRequest::CapabilitySet,
                ProtocolResponse::CapabilitySet(capabilities),
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
                        version: descriptor.clone(),
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
                    player: player.clone(),
                },
                ProtocolResponse::Frame(vec![8, 9]),
            ),
            (
                ProtocolRequest::DecodePlay {
                    player_id: sample_player_id(),
                    frame: vec![10, 11],
                },
                ProtocolResponse::CoreCommand(Some(sample_command())),
            ),
            (
                ProtocolRequest::EncodePlayEvent {
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
                    session: super::ProtocolSessionSnapshot {
                        phase: ConnectionPhase::Play,
                        player_id: Some(sample_player_id()),
                        entity_id: Some(EntityId(7)),
                    },
                },
                ProtocolResponse::SessionTransferBlob(vec![15, 16]),
            ),
            (
                ProtocolRequest::ImportSessionState {
                    session: super::ProtocolSessionSnapshot {
                        phase: ConnectionPhase::Play,
                        player_id: Some(sample_player_id()),
                        entity_id: Some(EntityId(7)),
                    },
                    blob: vec![17, 18],
                },
                ProtocolResponse::Empty,
            ),
        ];

        for (request, response) in requests_and_responses {
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
                player_id: sample_player_id(),
                frame: vec![1],
            },
            &ProtocolResponse::CoreCommand(Some(sample_command())),
        )
        .expect("response should encode");
        let mut truncated = response.clone();
        let _ = truncated.pop();
        let error = decode_protocol_response(
            &ProtocolRequest::DecodePlay {
                player_id: sample_player_id(),
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
                player_id: sample_player_id(),
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
        let mut protocol = CapabilitySet::new();
        let _ = protocol.insert("protocol.je.1_12_2");
        let capability_set = SessionCapabilitySet {
            protocol,
            gameplay: CapabilitySet::new(),
            gameplay_profile: GameplayProfileId::new("canonical"),
            protocol_generation: Some(PluginGenerationId(3)),
            gameplay_generation: Some(PluginGenerationId(4)),
        };
        assert!(capability_set.protocol.contains("protocol.je.1_12_2"));
        assert_eq!(capability_set.gameplay_profile.as_str(), "canonical");
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
            container: InventoryContainer::Player,
            slot: InventorySlot::Offhand,
            stack: Some(ItemStack::new("minecraft:shield", 1, 0)),
        };
        let request = ProtocolRequest::EncodePlayEvent {
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
        let response = ProtocolResponse::CoreCommand(Some(login_command));
        let decode_play = ProtocolRequest::DecodePlay {
            player_id: sample_player_id(),
            frame: vec![0x10],
        };
        let encoded_response =
            encode_protocol_response(&decode_play, &response).expect("command should encode");
        let decoded_response = decode_protocol_response(&decode_play, &encoded_response)
            .expect("command should decode");
        assert_eq!(decoded_response, response);
    }
}
