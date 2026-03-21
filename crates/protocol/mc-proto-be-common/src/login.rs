use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

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

pub(crate) fn parse_bedrock_login_payload(
    bytes: &[u8],
) -> Result<ParsedBedrockLogin, BedrockLoginError> {
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
