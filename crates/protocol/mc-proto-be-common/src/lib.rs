use base64::Engine;
use bedrockrs_proto::info::MAGIC as BEDROCK_MAGIC;
use mc_core::BlockPos;
use mc_proto_common::{Edition, HandshakeIntent, HandshakeNextState, ProtocolError};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use vek::Vec3;

pub const RAKNET_UNCONNECTED_PING: u8 = 0x01;
pub const RAKNET_OPEN_CONNECTION_REQUEST_1: u8 = 0x05;
pub const RAKNET_OPEN_CONNECTION_REQUEST_2: u8 = 0x07;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedBedrockLogin {
    pub display_name: String,
    pub chain_jwts: Vec<String>,
    pub client_data_jwt: String,
}

#[derive(Debug, Error)]
pub enum BedrockLoginError {
    #[error("bedrock login payload too short")]
    TooShort,
    #[error("bedrock login payload length overflow")]
    LengthOverflow,
    #[error("bedrock login chain json was invalid: {0}")]
    InvalidChainJson(#[from] serde_json::Error),
    #[error("bedrock login jwt payload was invalid")]
    InvalidJwtPayload,
    #[error("bedrock login did not include any jwt chain entries")]
    MissingChain,
    #[error("bedrock login did not provide a display name")]
    MissingDisplayName,
}

#[derive(Deserialize)]
struct ChainDocument {
    chain: Vec<String>,
}

fn has_magic_at(frame: &[u8], offset: usize) -> bool {
    frame
        .get(offset..offset + BEDROCK_MAGIC.len())
        .is_some_and(|slice| slice == BEDROCK_MAGIC)
}

#[must_use]
pub fn detects_bedrock_datagram(frame: &[u8]) -> bool {
    let Some(packet_id) = frame.first().copied() else {
        return false;
    };
    match packet_id {
        RAKNET_UNCONNECTED_PING => frame.len() >= 25 && has_magic_at(frame, 9),
        RAKNET_OPEN_CONNECTION_REQUEST_1 | RAKNET_OPEN_CONNECTION_REQUEST_2 => {
            frame.len() >= 17 && has_magic_at(frame, 1)
        }
        _ => false,
    }
}

pub fn bedrock_probe_intent() -> HandshakeIntent {
    HandshakeIntent {
        edition: Edition::Be,
        protocol_number: 0,
        server_host: String::new(),
        server_port: 0,
        next_state: HandshakeNextState::Login,
    }
}

pub fn parse_bedrock_login_payload(bytes: &[u8]) -> Result<ParsedBedrockLogin, BedrockLoginError> {
    if bytes.len() < 8 {
        return Err(BedrockLoginError::TooShort);
    }
    let chain_len = u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .map_err(|_| BedrockLoginError::TooShort)?,
    );
    let chain_len = usize::try_from(chain_len).map_err(|_| BedrockLoginError::LengthOverflow)?;
    let chain_end = 4_usize
        .checked_add(chain_len)
        .ok_or(BedrockLoginError::LengthOverflow)?;
    if bytes.len() < chain_end + 4 {
        return Err(BedrockLoginError::TooShort);
    }
    let chain_json = std::str::from_utf8(&bytes[4..chain_end])
        .map_err(|_| BedrockLoginError::InvalidJwtPayload)?;
    let chain: ChainDocument = serde_json::from_str(chain_json)?;

    let token_len = u32::from_le_bytes(
        bytes[chain_end..chain_end + 4]
            .try_into()
            .map_err(|_| BedrockLoginError::TooShort)?,
    );
    let token_len = usize::try_from(token_len).map_err(|_| BedrockLoginError::LengthOverflow)?;
    let token_end = chain_end
        .checked_add(4)
        .and_then(|value| value.checked_add(token_len))
        .ok_or(BedrockLoginError::LengthOverflow)?;
    if bytes.len() < token_end {
        return Err(BedrockLoginError::TooShort);
    }
    let client_data_jwt = std::str::from_utf8(&bytes[chain_end + 4..token_end])
        .map_err(|_| BedrockLoginError::InvalidJwtPayload)?
        .to_string();

    if chain.chain.is_empty() {
        return Err(BedrockLoginError::MissingChain);
    }
    let display_name = extract_display_name(&client_data_jwt).or_else(|| {
        chain
            .chain
            .last()
            .and_then(|jwt| extract_chain_display_name(jwt).ok())
    });
    let Some(display_name) = display_name else {
        return Err(BedrockLoginError::MissingDisplayName);
    };

    Ok(ParsedBedrockLogin {
        display_name,
        chain_jwts: chain.chain,
        client_data_jwt,
    })
}

fn extract_display_name(jwt: &str) -> Option<String> {
    decode_jwt_payload(jwt).ok().and_then(|payload| {
        payload
            .get("DisplayName")
            .and_then(Value::as_str)
            .or_else(|| payload.get("ThirdPartyName").and_then(Value::as_str))
            .map(ToString::to_string)
    })
}

fn extract_chain_display_name(jwt: &str) -> Result<String, BedrockLoginError> {
    let payload = decode_jwt_payload(jwt)?;
    payload
        .get("extraData")
        .and_then(|extra| extra.get("displayName"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or(BedrockLoginError::MissingDisplayName)
}

fn decode_jwt_payload(jwt: &str) -> Result<Value, BedrockLoginError> {
    let mut parts = jwt.split('.');
    let _header = parts.next().ok_or(BedrockLoginError::InvalidJwtPayload)?;
    let payload = parts.next().ok_or(BedrockLoginError::InvalidJwtPayload)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| BedrockLoginError::InvalidJwtPayload)?;
    serde_json::from_slice(&decoded).map_err(|_| BedrockLoginError::InvalidJwtPayload)
}

#[must_use]
pub fn block_pos_to_network(
    position: BlockPos,
) -> bedrockrs_proto::v662::types::NetworkBlockPosition {
    bedrockrs_proto::v662::types::NetworkBlockPosition {
        x: position.x,
        y: position.y.max(0) as u32,
        z: position.z,
    }
}

#[must_use]
pub fn block_pos_from_network(
    position: bedrockrs_proto::v662::types::NetworkBlockPosition,
) -> BlockPos {
    BlockPos::new(
        position.x,
        i32::try_from(position.y).unwrap_or(i32::MAX),
        position.z,
    )
}

#[must_use]
pub fn vec3_to_bedrock(position: mc_core::Vec3) -> Vec3<f32> {
    Vec3::new(position.x as f32, position.y as f32, position.z as f32)
}

pub fn protocol_error(message: &'static str) -> ProtocolError {
    ProtocolError::InvalidPacket(message)
}

#[cfg(test)]
mod tests {
    use super::{ParsedBedrockLogin, detects_bedrock_datagram, parse_bedrock_login_payload};
    use base64::Engine;
    use serde_json::json;

    fn test_jwt(payload: serde_json::Value) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        format!("{header}.{payload}.")
    }

    #[test]
    fn parses_connection_request_blob() {
        let chain = json!({
            "chain": [
                test_jwt(json!({"extraData":{"displayName":"ChainName"}}))
            ]
        })
        .to_string();
        let client_jwt = test_jwt(json!({"DisplayName":"ClientName"}));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(chain.len() as u32).to_le_bytes());
        bytes.extend_from_slice(chain.as_bytes());
        bytes.extend_from_slice(&(client_jwt.len() as u32).to_le_bytes());
        bytes.extend_from_slice(client_jwt.as_bytes());

        let parsed = parse_bedrock_login_payload(&bytes).expect("login payload should parse");
        assert_eq!(
            parsed,
            ParsedBedrockLogin {
                display_name: "ClientName".to_string(),
                chain_jwts: vec![test_jwt(json!({"extraData":{"displayName":"ChainName"}}))],
                client_data_jwt: client_jwt,
            }
        );
    }

    #[test]
    fn recognises_raknet_bedrock_probe() {
        let mut datagram = Vec::new();
        datagram.push(0x01);
        datagram.extend_from_slice(&123_i64.to_be_bytes());
        datagram.extend_from_slice(&bedrockrs_proto::info::MAGIC);
        datagram.extend_from_slice(&456_i64.to_be_bytes());
        assert!(detects_bedrock_datagram(&datagram));
    }
}
