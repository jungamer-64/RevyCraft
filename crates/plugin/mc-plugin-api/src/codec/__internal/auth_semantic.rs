use crate::codec::__internal::binary::{Decoder, Encoder, ProtocolCodecError};
use crate::codec::__internal::shared::{
    decode_capability_set, decode_player_id, encode_capability_set, encode_player_id,
};
use crate::codec::auth::{
    AuthDescriptor, AuthMode, AuthOpCode, AuthRequest, AuthResponse, BedrockAuthResult,
};

pub(crate) fn encode_auth_request_payload(
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

pub(crate) fn decode_auth_request_payload(
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

pub(crate) fn encode_auth_response_payload(
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

pub(crate) fn decode_auth_response_payload(
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
