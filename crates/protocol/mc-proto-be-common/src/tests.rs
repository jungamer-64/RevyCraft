use crate::login::{ParsedBedrockLogin, parse_bedrock_login_payload};
use crate::probe::detects_bedrock_datagram;
use base64::Engine;
use serde_json::json;

fn test_jwt(payload: &serde_json::Value) -> String {
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
    let payload =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
    format!("{header}.{payload}.")
}

#[test]
fn parses_connection_request_blob() {
    let chain = json!({
        "chain": [
            test_jwt(&json!({"extraData":{"displayName":"ChainName"}}))
        ]
    })
    .to_string();
    let client_jwt = test_jwt(&json!({"DisplayName":"ClientName"}));
    let mut bytes = Vec::new();
    bytes.extend_from_slice(
        &u32::try_from(chain.len())
            .expect("chain length should fit into u32")
            .to_le_bytes(),
    );
    bytes.extend_from_slice(chain.as_bytes());
    bytes.extend_from_slice(
        &u32::try_from(client_jwt.len())
            .expect("client jwt length should fit into u32")
            .to_le_bytes(),
    );
    bytes.extend_from_slice(client_jwt.as_bytes());

    let parsed = parse_bedrock_login_payload(&bytes).expect("login payload should parse");
    assert_eq!(
        parsed,
        ParsedBedrockLogin {
            display_name: "ClientName".to_string(),
            chain_jwts: vec![test_jwt(&json!({"extraData":{"displayName":"ChainName"}}))],
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
