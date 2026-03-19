use crate::protocol_codec::{
    Decoder, Encoder, EnvelopeHeader, decode_capability_set, decode_envelope, decode_player_id,
    encode_capability_set, encode_envelope, encode_player_id,
};
use crate::{CURRENT_PLUGIN_ABI, PROTOCOL_FLAG_RESPONSE, PluginKind, ProtocolCodecError};
use mc_core::{CapabilitySet, PlayerId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AuthOpCode {
    Describe = 1,
    CapabilitySet = 2,
    AuthenticateOffline = 3,
}

impl TryFrom<u8> for AuthOpCode {
    type Error = ProtocolCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Describe),
            2 => Ok(Self::CapabilitySet),
            3 => Ok(Self::AuthenticateOffline),
            _ => Err(ProtocolCodecError::InvalidValue("invalid auth op code")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMode {
    Offline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthDescriptor {
    pub auth_profile: String,
    pub mode: AuthMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthRequest {
    Describe,
    CapabilitySet,
    AuthenticateOffline { username: String },
}

impl AuthRequest {
    #[must_use]
    pub const fn op_code(&self) -> AuthOpCode {
        match self {
            Self::Describe => AuthOpCode::Describe,
            Self::CapabilitySet => AuthOpCode::CapabilitySet,
            Self::AuthenticateOffline { .. } => AuthOpCode::AuthenticateOffline,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthResponse {
    Descriptor(AuthDescriptor),
    CapabilitySet(CapabilitySet),
    AuthenticatedPlayer(PlayerId),
}

pub fn encode_auth_request(request: &AuthRequest) -> Result<Vec<u8>, ProtocolCodecError> {
    let mut payload = Encoder::default();
    encode_auth_request_payload(&mut payload, request)?;
    let payload = payload.into_inner();
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::Auth,
            op_code: request.op_code() as u8,
            flags: 0,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        payload,
    )
}

pub fn decode_auth_request(bytes: &[u8]) -> Result<AuthRequest, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::Auth {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "auth request had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE != 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "auth request unexpectedly set response flag",
        ));
    }
    let mut decoder = Decoder::new(payload);
    let request = decode_auth_request_payload(&mut decoder, AuthOpCode::try_from(header.op_code)?)?;
    decoder.finish()?;
    Ok(request)
}

pub fn encode_auth_response(
    request: &AuthRequest,
    response: &AuthResponse,
) -> Result<Vec<u8>, ProtocolCodecError> {
    let mut payload = Encoder::default();
    encode_auth_response_payload(&mut payload, request.op_code(), response)?;
    let payload = payload.into_inner();
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::Auth,
            op_code: request.op_code() as u8,
            flags: PROTOCOL_FLAG_RESPONSE,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        payload,
    )
}

pub fn decode_auth_response(
    request: &AuthRequest,
    bytes: &[u8],
) -> Result<AuthResponse, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::Auth {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "auth response had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE == 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "auth response was missing response flag",
        ));
    }
    if AuthOpCode::try_from(header.op_code)? != request.op_code() {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "auth response opcode did not match request",
        ));
    }
    let mut decoder = Decoder::new(payload);
    let response = decode_auth_response_payload(&mut decoder, request.op_code())?;
    decoder.finish()?;
    Ok(response)
}

fn encode_auth_request_payload(
    encoder: &mut Encoder,
    request: &AuthRequest,
) -> Result<(), ProtocolCodecError> {
    match request {
        AuthRequest::Describe | AuthRequest::CapabilitySet => Ok(()),
        AuthRequest::AuthenticateOffline { username } => encoder.write_string(username),
    }
}

fn decode_auth_request_payload(
    decoder: &mut Decoder<'_>,
    op_code: AuthOpCode,
) -> Result<AuthRequest, ProtocolCodecError> {
    match op_code {
        AuthOpCode::Describe => Ok(AuthRequest::Describe),
        AuthOpCode::CapabilitySet => Ok(AuthRequest::CapabilitySet),
        AuthOpCode::AuthenticateOffline => Ok(AuthRequest::AuthenticateOffline {
            username: decoder.read_string()?,
        }),
    }
}

fn encode_auth_response_payload(
    encoder: &mut Encoder,
    op_code: AuthOpCode,
    response: &AuthResponse,
) -> Result<(), ProtocolCodecError> {
    match (op_code, response) {
        (AuthOpCode::Describe, AuthResponse::Descriptor(descriptor)) => {
            encoder.write_string(&descriptor.auth_profile)?;
            encode_auth_mode(encoder, descriptor.mode);
            Ok(())
        }
        (AuthOpCode::CapabilitySet, AuthResponse::CapabilitySet(capabilities)) => {
            encode_capability_set(encoder, capabilities)
        }
        (AuthOpCode::AuthenticateOffline, AuthResponse::AuthenticatedPlayer(player_id)) => {
            encode_player_id(encoder, *player_id);
            Ok(())
        }
        _ => Err(ProtocolCodecError::InvalidValue(
            "auth response did not match opcode",
        )),
    }
}

fn decode_auth_response_payload(
    decoder: &mut Decoder<'_>,
    op_code: AuthOpCode,
) -> Result<AuthResponse, ProtocolCodecError> {
    match op_code {
        AuthOpCode::Describe => Ok(AuthResponse::Descriptor(AuthDescriptor {
            auth_profile: decoder.read_string()?,
            mode: decode_auth_mode(decoder)?,
        })),
        AuthOpCode::CapabilitySet => {
            Ok(AuthResponse::CapabilitySet(decode_capability_set(decoder)?))
        }
        AuthOpCode::AuthenticateOffline => Ok(AuthResponse::AuthenticatedPlayer(decode_player_id(
            decoder,
        )?)),
    }
}

fn encode_auth_mode(encoder: &mut Encoder, mode: AuthMode) {
    encoder.write_u8(match mode {
        AuthMode::Offline => 1,
    });
}

fn decode_auth_mode(decoder: &mut Decoder<'_>) -> Result<AuthMode, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(AuthMode::Offline),
        _ => Err(ProtocolCodecError::InvalidValue("invalid auth mode")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthDescriptor, AuthMode, AuthRequest, AuthResponse, decode_auth_request,
        decode_auth_response, encode_auth_request, encode_auth_response,
    };
    use mc_core::{CapabilitySet, PlayerId};
    use uuid::Uuid;

    #[test]
    fn auth_request_roundtrip() {
        let request = AuthRequest::AuthenticateOffline {
            username: "alice".to_string(),
        };
        let encoded = encode_auth_request(&request).expect("request should encode");
        let decoded = decode_auth_request(&encoded).expect("request should decode");
        assert_eq!(decoded, request);
    }

    #[test]
    fn auth_response_roundtrip() {
        let request = AuthRequest::Describe;
        let response = AuthResponse::Descriptor(AuthDescriptor {
            auth_profile: "offline-v1".to_string(),
            mode: AuthMode::Offline,
        });
        let encoded = encode_auth_response(&request, &response).expect("response should encode");
        let decoded = decode_auth_response(&request, &encoded).expect("response should decode");
        assert_eq!(decoded, response);
    }

    #[test]
    fn auth_capability_roundtrip() {
        let request = AuthRequest::CapabilitySet;
        let mut capability_set = CapabilitySet::new();
        let _ = capability_set.insert("auth.offline");
        let response = AuthResponse::CapabilitySet(capability_set);
        let encoded = encode_auth_response(&request, &response).expect("response should encode");
        let decoded = decode_auth_response(&request, &encoded).expect("response should decode");
        assert_eq!(decoded, response);
    }

    #[test]
    fn auth_player_roundtrip() {
        let request = AuthRequest::AuthenticateOffline {
            username: "alice".to_string(),
        };
        let response = AuthResponse::AuthenticatedPlayer(PlayerId(Uuid::from_u128(99)));
        let encoded = encode_auth_response(&request, &response).expect("response should encode");
        let decoded = decode_auth_response(&request, &encoded).expect("response should decode");
        assert_eq!(decoded, response);
    }
}
