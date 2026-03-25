use crate::codec::__internal::binary::{Decoder, Encoder, ProtocolCodecError};
use crate::codec::__internal::shared::{
    decode_capability_announcement, decode_connection_id, decode_connection_phase,
    decode_core_command, decode_core_event, decode_entity_id, decode_option, decode_player_id,
    decode_player_snapshot, encode_capability_announcement, encode_connection_id,
    encode_connection_phase, encode_core_command, encode_core_event, encode_entity_id,
    encode_option, encode_player_id, encode_player_snapshot,
};
use crate::codec::protocol::{
    ProtocolOpCode, ProtocolRequest, ProtocolResponse, ProtocolSessionSnapshot,
    WireFrameDecodeResult,
};
use mc_proto_common::{
    BedrockListenerDescriptor, Edition, HandshakeIntent, HandshakeNextState, LoginRequest,
    PlayEncodingContext, ProtocolDescriptor, ServerListStatus, StatusRequest, TransportKind,
    WireFormatKind,
};

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

fn encode_wire_format_kind(encoder: &mut Encoder, wire_format: WireFormatKind) {
    encoder.write_u8(match wire_format {
        WireFormatKind::MinecraftFramed => 1,
        WireFormatKind::RawPacketStream => 2,
    });
}

fn decode_wire_format_kind(
    decoder: &mut Decoder<'_>,
) -> Result<WireFormatKind, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(WireFormatKind::MinecraftFramed),
        2 => Ok(WireFormatKind::RawPacketStream),
        _ => Err(ProtocolCodecError::InvalidValue("invalid wire format kind")),
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

fn encode_protocol_descriptor(
    encoder: &mut Encoder,
    descriptor: &ProtocolDescriptor,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(&descriptor.adapter_id)?;
    encode_transport_kind(encoder, descriptor.transport);
    encode_wire_format_kind(encoder, descriptor.wire_format);
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
        wire_format: decode_wire_format_kind(decoder)?,
        edition: decode_edition(decoder)?,
        version_name: decoder.read_string()?,
        protocol_number: decoder.read_i32()?,
    })
}

fn encode_bedrock_listener_descriptor(
    encoder: &mut Encoder,
    descriptor: &BedrockListenerDescriptor,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(&descriptor.game_version)?;
    encoder.write_u8(descriptor.raknet_version);
    Ok(())
}

fn decode_bedrock_listener_descriptor(
    decoder: &mut Decoder<'_>,
) -> Result<BedrockListenerDescriptor, ProtocolCodecError> {
    Ok(BedrockListenerDescriptor {
        game_version: decoder.read_string()?,
        raknet_version: decoder.read_u8()?,
    })
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

fn encode_status_request(encoder: &mut Encoder, request: &StatusRequest) {
    match request {
        StatusRequest::Query => encoder.write_u8(1),
        StatusRequest::Ping { payload } => {
            encoder.write_u8(2);
            encoder.write_i64(*payload);
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
    encode_connection_id(encoder, snapshot.connection_id);
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
        connection_id: decode_connection_id(decoder)?,
        phase: decode_connection_phase(decoder)?,
        player_id: decode_option(decoder, decode_player_id)?,
        entity_id: decode_option(decoder, decode_entity_id)?,
    })
}

fn encode_wire_frame_decode_result(
    encoder: &mut Encoder,
    result: &WireFrameDecodeResult,
) -> Result<(), ProtocolCodecError> {
    encoder.write_len(result.bytes_consumed)?;
    encoder.write_bytes(&result.frame)
}

fn decode_wire_frame_decode_result(
    decoder: &mut Decoder<'_>,
) -> Result<WireFrameDecodeResult, ProtocolCodecError> {
    Ok(WireFrameDecodeResult {
        bytes_consumed: decoder.read_len()?,
        frame: decoder.read_bytes()?,
    })
}

pub(crate) fn encode_protocol_request_payload(
    encoder: &mut Encoder,
    request: &ProtocolRequest,
) -> Result<(), ProtocolCodecError> {
    match request {
        ProtocolRequest::Describe
        | ProtocolRequest::DescribeBedrockListener
        | ProtocolRequest::CapabilitySet => Ok(()),
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
        ProtocolRequest::DecodePlay { session, frame } => {
            encode_protocol_session_snapshot(encoder, session)?;
            encoder.write_bytes(frame)
        }
        ProtocolRequest::EncodePlayEvent {
            session,
            event,
            context,
        } => {
            encode_protocol_session_snapshot(encoder, session)?;
            encode_core_event(encoder, event)?;
            encode_play_encoding_context(encoder, context);
            Ok(())
        }
        ProtocolRequest::SessionClosed { session }
        | ProtocolRequest::ExportSessionState { session } => {
            encode_protocol_session_snapshot(encoder, session)
        }
        ProtocolRequest::ImportSessionState { session, blob } => {
            encode_protocol_session_snapshot(encoder, session)?;
            encoder.write_bytes(blob)
        }
        ProtocolRequest::EncodeWireFrame { payload } => encoder.write_bytes(payload),
        ProtocolRequest::TryDecodeWireFrame { buffer } => encoder.write_bytes(buffer),
    }
}

pub(crate) fn decode_protocol_request_payload(
    decoder: &mut Decoder<'_>,
    op_code: ProtocolOpCode,
) -> Result<ProtocolRequest, ProtocolCodecError> {
    match op_code {
        ProtocolOpCode::Describe => Ok(ProtocolRequest::Describe),
        ProtocolOpCode::DescribeBedrockListener => Ok(ProtocolRequest::DescribeBedrockListener),
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
            session: decode_protocol_session_snapshot(decoder)?,
            frame: decoder.read_bytes()?,
        }),
        ProtocolOpCode::EncodePlayEvent => Ok(ProtocolRequest::EncodePlayEvent {
            session: decode_protocol_session_snapshot(decoder)?,
            event: decode_core_event(decoder)?,
            context: decode_play_encoding_context(decoder)?,
        }),
        ProtocolOpCode::SessionClosed => Ok(ProtocolRequest::SessionClosed {
            session: decode_protocol_session_snapshot(decoder)?,
        }),
        ProtocolOpCode::ExportSessionState => Ok(ProtocolRequest::ExportSessionState {
            session: decode_protocol_session_snapshot(decoder)?,
        }),
        ProtocolOpCode::ImportSessionState => Ok(ProtocolRequest::ImportSessionState {
            session: decode_protocol_session_snapshot(decoder)?,
            blob: decoder.read_bytes()?,
        }),
        ProtocolOpCode::EncodeWireFrame => Ok(ProtocolRequest::EncodeWireFrame {
            payload: decoder.read_bytes()?,
        }),
        ProtocolOpCode::TryDecodeWireFrame => Ok(ProtocolRequest::TryDecodeWireFrame {
            buffer: decoder.read_bytes()?,
        }),
    }
}

pub(crate) fn encode_protocol_response_payload(
    encoder: &mut Encoder,
    op_code: ProtocolOpCode,
    response: &ProtocolResponse,
) -> Result<(), ProtocolCodecError> {
    match (op_code, response) {
        (ProtocolOpCode::Describe, ProtocolResponse::Descriptor(descriptor)) => {
            encode_protocol_descriptor(encoder, descriptor)
        }
        (
            ProtocolOpCode::DescribeBedrockListener,
            ProtocolResponse::BedrockListenerDescriptor(descriptor),
        ) => encode_option(
            encoder,
            descriptor.as_ref(),
            encode_bedrock_listener_descriptor,
        ),
        (ProtocolOpCode::CapabilitySet, ProtocolResponse::CapabilitySet(capabilities)) => {
            encode_capability_announcement(encoder, capabilities)
        }
        (ProtocolOpCode::TryRoute, ProtocolResponse::HandshakeIntent(intent)) => {
            encode_option(encoder, intent.as_ref(), encode_handshake_intent)
        }
        (ProtocolOpCode::DecodeStatus, ProtocolResponse::StatusRequest(request)) => {
            encode_status_request(encoder, request);
            Ok(())
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
        (ProtocolOpCode::EncodeWireFrame, ProtocolResponse::Frame(frame)) => {
            encoder.write_bytes(frame)
        }
        (ProtocolOpCode::TryDecodeWireFrame, ProtocolResponse::WireFrameDecodeResult(result)) => {
            encode_option(encoder, result.as_ref(), encode_wire_frame_decode_result)
        }
        (
            ProtocolOpCode::ImportSessionState | ProtocolOpCode::SessionClosed,
            ProtocolResponse::Empty,
        ) => Ok(()),
        _ => Err(ProtocolCodecError::InvalidValue(
            "protocol response did not match opcode",
        )),
    }
}

pub(crate) fn decode_protocol_response_payload(
    decoder: &mut Decoder<'_>,
    op_code: ProtocolOpCode,
) -> Result<ProtocolResponse, ProtocolCodecError> {
    match op_code {
        ProtocolOpCode::Describe => Ok(ProtocolResponse::Descriptor(decode_protocol_descriptor(
            decoder,
        )?)),
        ProtocolOpCode::DescribeBedrockListener => Ok(ProtocolResponse::BedrockListenerDescriptor(
            decode_option(decoder, decode_bedrock_listener_descriptor)?,
        )),
        ProtocolOpCode::CapabilitySet => Ok(ProtocolResponse::CapabilitySet(
            decode_capability_announcement(decoder)?,
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
        | ProtocolOpCode::EncodeLoginSuccess
        | ProtocolOpCode::EncodeWireFrame => Ok(ProtocolResponse::Frame(decoder.read_bytes()?)),
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
        ProtocolOpCode::TryDecodeWireFrame => Ok(ProtocolResponse::WireFrameDecodeResult(
            decode_option(decoder, decode_wire_frame_decode_result)?,
        )),
        ProtocolOpCode::ImportSessionState | ProtocolOpCode::SessionClosed => {
            Ok(ProtocolResponse::Empty)
        }
    }
}
