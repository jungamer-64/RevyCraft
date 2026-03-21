pub use crate::login::{BedrockLoginError, ParsedBedrockLogin};

pub fn parse_bedrock_login_payload(bytes: &[u8]) -> Result<ParsedBedrockLogin, BedrockLoginError> {
    crate::login::parse_bedrock_login_payload(bytes)
}
