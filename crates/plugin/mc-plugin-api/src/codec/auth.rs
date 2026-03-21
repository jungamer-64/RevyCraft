use crate::abi::{CURRENT_PLUGIN_ABI, PluginKind};
use crate::codec::internal::binary::{
    Decoder, Encoder, EnvelopeHeader, PROTOCOL_FLAG_RESPONSE, ProtocolCodecError,
    decode_capability_set, decode_envelope, decode_player_id, encode_capability_set,
    encode_envelope, encode_player_id,
};
use mc_core::{CapabilitySet, PlayerId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AuthOpCode {
    Describe = 1,
    CapabilitySet = 2,
    AuthenticateOffline = 3,
    AuthenticateOnline = 4,
    AuthenticateBedrockOffline = 5,
    AuthenticateBedrockXbl = 6,
}

impl TryFrom<u8> for AuthOpCode {
    type Error = ProtocolCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Describe),
            2 => Ok(Self::CapabilitySet),
            3 => Ok(Self::AuthenticateOffline),
            4 => Ok(Self::AuthenticateOnline),
            5 => Ok(Self::AuthenticateBedrockOffline),
            6 => Ok(Self::AuthenticateBedrockXbl),
            _ => Err(ProtocolCodecError::InvalidValue("invalid auth op code")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMode {
    Offline,
    Online,
    BedrockOffline,
    BedrockXbl,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthDescriptor {
    pub auth_profile: String,
    pub mode: AuthMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BedrockAuthResult {
    pub player_id: PlayerId,
    pub display_name: String,
    pub xuid: Option<String>,
    pub identity_uuid: Option<String>,
    pub skin_claims_blob: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthRequest {
    Describe,
    CapabilitySet,
    AuthenticateOffline {
        username: String,
    },
    AuthenticateOnline {
        username: String,
        server_hash: String,
    },
    AuthenticateBedrockOffline {
        display_name: String,
    },
    AuthenticateBedrockXbl {
        chain_jwts: Vec<String>,
        client_data_jwt: String,
    },
}

impl AuthRequest {
    #[must_use]
    pub const fn op_code(&self) -> AuthOpCode {
        match self {
            Self::Describe => AuthOpCode::Describe,
            Self::CapabilitySet => AuthOpCode::CapabilitySet,
            Self::AuthenticateOffline { .. } => AuthOpCode::AuthenticateOffline,
            Self::AuthenticateOnline { .. } => AuthOpCode::AuthenticateOnline,
            Self::AuthenticateBedrockOffline { .. } => AuthOpCode::AuthenticateBedrockOffline,
            Self::AuthenticateBedrockXbl { .. } => AuthOpCode::AuthenticateBedrockXbl,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthResponse {
    Descriptor(AuthDescriptor),
    CapabilitySet(CapabilitySet),
    AuthenticatedPlayer(PlayerId),
    AuthenticatedBedrockPlayer(BedrockAuthResult),
}

/// Encodes an auth request into the plugin protocol envelope.
///
/// # Errors
///
/// Returns an error when the request payload exceeds the protocol length limits or contains
/// values that cannot be serialized.
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
        &payload,
    )
}

/// Decodes an auth request from the plugin protocol envelope.
///
/// # Errors
///
/// Returns an error when the envelope is malformed, the plugin kind/opcode is invalid, or the
/// auth payload cannot be decoded.
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

/// Encodes an auth response for the provided auth request.
///
/// # Errors
///
/// Returns an error when the response does not match the request opcode, exceeds protocol
/// length limits, or contains values that cannot be serialized.
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
        &payload,
    )
}

/// Decodes an auth response for the provided auth request.
///
/// # Errors
///
/// Returns an error when the envelope is malformed, the response opcode does not match the
/// request, or the auth payload cannot be decoded.
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
        AuthRequest::AuthenticateOnline {
            username,
            server_hash,
        } => {
            encoder.write_string(username)?;
            encoder.write_string(server_hash)
        }
        AuthRequest::AuthenticateBedrockOffline { display_name } => {
            encoder.write_string(display_name)
        }
        AuthRequest::AuthenticateBedrockXbl {
            chain_jwts,
            client_data_jwt,
        } => {
            encoder.write_len(chain_jwts.len())?;
            for jwt in chain_jwts {
                encoder.write_string(jwt)?;
            }
            encoder.write_string(client_data_jwt)
        }
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
        AuthOpCode::AuthenticateOnline => Ok(AuthRequest::AuthenticateOnline {
            username: decoder.read_string()?,
            server_hash: decoder.read_string()?,
        }),
        AuthOpCode::AuthenticateBedrockOffline => Ok(AuthRequest::AuthenticateBedrockOffline {
            display_name: decoder.read_string()?,
        }),
        AuthOpCode::AuthenticateBedrockXbl => {
            let chain_len = decoder.read_len()?;
            let mut chain_jwts = Vec::with_capacity(chain_len);
            for _ in 0..chain_len {
                chain_jwts.push(decoder.read_string()?);
            }
            Ok(AuthRequest::AuthenticateBedrockXbl {
                chain_jwts,
                client_data_jwt: decoder.read_string()?,
            })
        }
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
        (
            AuthOpCode::AuthenticateOffline | AuthOpCode::AuthenticateOnline,
            AuthResponse::AuthenticatedPlayer(player_id),
        ) => {
            encode_player_id(encoder, *player_id);
            Ok(())
        }
        (
            AuthOpCode::AuthenticateBedrockOffline | AuthOpCode::AuthenticateBedrockXbl,
            AuthResponse::AuthenticatedBedrockPlayer(result),
        ) => encode_bedrock_auth_result(encoder, result),
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
        AuthOpCode::AuthenticateOffline | AuthOpCode::AuthenticateOnline => Ok(
            AuthResponse::AuthenticatedPlayer(decode_player_id(decoder)?),
        ),
        AuthOpCode::AuthenticateBedrockOffline | AuthOpCode::AuthenticateBedrockXbl => Ok(
            AuthResponse::AuthenticatedBedrockPlayer(decode_bedrock_auth_result(decoder)?),
        ),
    }
}

fn encode_auth_mode(encoder: &mut Encoder, mode: AuthMode) {
    encoder.write_u8(match mode {
        AuthMode::Offline => 1,
        AuthMode::Online => 2,
        AuthMode::BedrockOffline => 3,
        AuthMode::BedrockXbl => 4,
    });
}

fn decode_auth_mode(decoder: &mut Decoder<'_>) -> Result<AuthMode, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(AuthMode::Offline),
        2 => Ok(AuthMode::Online),
        3 => Ok(AuthMode::BedrockOffline),
        4 => Ok(AuthMode::BedrockXbl),
        _ => Err(ProtocolCodecError::InvalidValue("invalid auth mode")),
    }
}

fn encode_bedrock_auth_result(
    encoder: &mut Encoder,
    result: &BedrockAuthResult,
) -> Result<(), ProtocolCodecError> {
    encode_player_id(encoder, result.player_id);
    encoder.write_string(&result.display_name)?;
    encode_optional_string(encoder, result.xuid.as_deref())?;
    encode_optional_string(encoder, result.identity_uuid.as_deref())?;
    encoder.write_bytes(&result.skin_claims_blob)
}

fn decode_bedrock_auth_result(
    decoder: &mut Decoder<'_>,
) -> Result<BedrockAuthResult, ProtocolCodecError> {
    Ok(BedrockAuthResult {
        player_id: decode_player_id(decoder)?,
        display_name: decoder.read_string()?,
        xuid: decode_optional_string(decoder)?,
        identity_uuid: decode_optional_string(decoder)?,
        skin_claims_blob: decoder.read_bytes()?,
    })
}

fn encode_optional_string(
    encoder: &mut Encoder,
    value: Option<&str>,
) -> Result<(), ProtocolCodecError> {
    if let Some(value) = value {
        encoder.write_bool(true);
        encoder.write_string(value)
    } else {
        encoder.write_bool(false);
        Ok(())
    }
}

fn decode_optional_string(decoder: &mut Decoder<'_>) -> Result<Option<String>, ProtocolCodecError> {
    if decoder.read_bool()? {
        Ok(Some(decoder.read_string()?))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthDescriptor, AuthMode, AuthRequest, AuthResponse, BedrockAuthResult,
        decode_auth_request, decode_auth_response, encode_auth_request, encode_auth_response,
    };
    use mc_core::{CapabilitySet, PlayerId};
    use uuid::Uuid;

    #[test]
    fn auth_descriptor_roundtrip_preserves_mode() {
        let request = AuthRequest::Describe;
        let response = AuthResponse::Descriptor(AuthDescriptor {
            auth_profile: "bedrock-xbl-v1".to_string(),
            mode: AuthMode::BedrockXbl,
        });
        let bytes = encode_auth_response(&request, &response).expect("descriptor should encode");
        let decoded = decode_auth_response(&request, &bytes).expect("descriptor should decode");
        assert_eq!(decoded, response);
    }

    #[test]
    fn auth_capability_set_roundtrip() {
        let mut capabilities = CapabilitySet::new();
        let _ = capabilities.insert("auth.bedrock");
        let request = AuthRequest::CapabilitySet;
        let response = AuthResponse::CapabilitySet(capabilities);
        let bytes =
            encode_auth_response(&request, &response).expect("capability set should encode");
        let decoded = decode_auth_response(&request, &bytes).expect("capability set should decode");
        assert_eq!(decoded, response);
    }

    #[test]
    fn online_auth_request_roundtrip() {
        let request = AuthRequest::AuthenticateOnline {
            username: "alex".to_string(),
            server_hash: "hash".to_string(),
        };
        let encoded = encode_auth_request(&request).expect("request should encode");
        let decoded = decode_auth_request(&encoded).expect("request should decode");
        assert_eq!(decoded, request);
    }

    #[test]
    fn bedrock_xbl_request_and_response_roundtrip() {
        let request = AuthRequest::AuthenticateBedrockXbl {
            chain_jwts: vec!["a".to_string(), "b".to_string()],
            client_data_jwt: "c".to_string(),
        };
        let encoded = encode_auth_request(&request).expect("request should encode");
        let decoded = decode_auth_request(&encoded).expect("request should decode");
        assert_eq!(decoded, request);

        let response = AuthResponse::AuthenticatedBedrockPlayer(BedrockAuthResult {
            player_id: PlayerId(Uuid::from_u128(42)),
            display_name: "Steve".to_string(),
            xuid: Some("123".to_string()),
            identity_uuid: Some(Uuid::from_u128(7).to_string()),
            skin_claims_blob: vec![1, 2, 3],
        });
        let encoded =
            encode_auth_response(&request, &response).expect("bedrock auth response should encode");
        let decoded =
            decode_auth_response(&request, &encoded).expect("bedrock auth response should decode");
        assert_eq!(decoded, response);
    }

    #[test]
    fn malformed_auth_response_is_rejected() {
        let request = AuthRequest::AuthenticateBedrockOffline {
            display_name: "Builder".to_string(),
        };
        let bytes = encode_auth_response(
            &request,
            &AuthResponse::AuthenticatedBedrockPlayer(BedrockAuthResult {
                player_id: PlayerId(Uuid::from_u128(9)),
                display_name: "Builder".to_string(),
                xuid: None,
                identity_uuid: None,
                skin_claims_blob: vec![],
            }),
        )
        .expect("response should encode");
        let mut truncated = bytes;
        let _ = truncated.pop();
        assert!(decode_auth_response(&request, &truncated).is_err());
    }
}
