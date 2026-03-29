#![allow(clippy::multiple_crate_versions)]
use base64::Engine;
use bedrock_jwt::verifier::{
    build_public_key_from_b64, decode_b64_url_nopad, decode_header_get_x5u, jose_sig_to_der,
    verify_chain,
};
use bedrockrs_proto::info::MOJANG_PUBLIC_KEY;
use mc_plugin_api::codec::auth::{AuthDescriptor, AuthMode, BedrockAuthResult};
use mc_plugin_sdk_rust::auth::RustAuthPlugin;
use mc_plugin_sdk_rust::capabilities::auth_capabilities;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use p384::ecdsa::{Signature as EcdsaSignature, VerifyingKey, signature::Verifier};
use mc_plugin_sdk_rust::{AuthCapability, AuthCapabilitySet, PlayerId};
use serde_json::Value;
use uuid::Uuid;

pub const BEDROCK_XBL_AUTH_PROFILE_ID: &str = "bedrock-xbl-v1";
pub const BEDROCK_XBL_AUTH_PLUGIN_ID: &str = "auth-bedrock-xbl";

#[derive(Default)]
pub struct BedrockXblAuthPlugin;

impl RustAuthPlugin for BedrockXblAuthPlugin {
    fn descriptor(&self) -> AuthDescriptor {
        AuthDescriptor {
            auth_profile: BEDROCK_XBL_AUTH_PROFILE_ID.into(),
            mode: AuthMode::BedrockXbl,
        }
    }

    fn capability_set(&self) -> AuthCapabilitySet {
        auth_capabilities(&[AuthCapability::RuntimeReload])
    }

    fn authenticate_offline(&self, _username: &str) -> Result<PlayerId, String> {
        Err("bedrock xbl auth plugin only handles bedrock auth requests".to_string())
    }

    fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, String> {
        let token_refs = chain_jwts.iter().map(String::as_str).collect::<Vec<_>>();
        let claims = verify_chain(&token_refs, MOJANG_PUBLIC_KEY)
            .map_err(|error| format!("bedrock jwt chain verification failed: {error}"))?;
        let identity_public_key = identity_public_key(chain_jwts)
            .map_err(|error| format!("missing bedrock identity public key: {error}"))?;
        let client_payload = verify_client_data_jwt(client_data_jwt, &identity_public_key)
            .map_err(|error| format!("bedrock client data verification failed: {error}"))?;
        let player_id = Uuid::parse_str(&claims.uuid)
            .map(PlayerId)
            .map_err(|error| format!("invalid bedrock identity uuid `{}`: {error}", claims.uuid))?;
        let display_name = client_payload
            .get("DisplayName")
            .and_then(Value::as_str)
            .unwrap_or(&claims.display_name)
            .to_string();
        Ok(BedrockAuthResult {
            player_id,
            display_name,
            xuid: (!claims.xuid.is_empty()).then_some(claims.xuid),
            identity_uuid: Some(claims.uuid),
            skin_claims_blob: serde_json::to_vec(&client_payload)
                .map_err(|error| format!("failed to serialize bedrock client claims: {error}"))?,
        })
    }
}

fn identity_public_key(chain_jwts: &[String]) -> Result<String, String> {
    let Some(last) = chain_jwts.last() else {
        return Err("chain was empty".to_string());
    };
    let payload = decode_jwt_payload(last)?;
    payload
        .get("identityPublicKey")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| "identityPublicKey claim was missing".to_string())
}

fn verify_client_data_jwt(
    client_data_jwt: &str,
    identity_public_key_b64: &str,
) -> Result<Value, String> {
    let parts = client_data_jwt.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err("invalid jwt format".to_string());
    }
    let header_key = decode_header_get_x5u(parts[0]).map_err(|error| error.to_string())?;
    let verifying_key_b64 = if header_key.is_empty() {
        identity_public_key_b64.to_string()
    } else {
        header_key
    };
    let public_key =
        build_public_key_from_b64(&verifying_key_b64).map_err(|error| error.to_string())?;
    let signing_input = format!("{}.{}", parts[0], parts[1]);
    let sig_bytes = decode_b64_url_nopad(parts[2]).map_err(|error| error.to_string())?;
    let der_sig = jose_sig_to_der(&sig_bytes).map_err(|error| error.to_string())?;
    let verifying_key = VerifyingKey::from(&public_key);
    let signature = EcdsaSignature::from_der(&der_sig).map_err(|error| error.to_string())?;
    verifying_key
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|error| error.to_string())?;
    decode_jwt_payload(client_data_jwt)
}

fn decode_jwt_payload(jwt: &str) -> Result<Value, String> {
    let parts = jwt.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err("invalid jwt format".to_string());
    }
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&payload).map_err(|error| error.to_string())
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::auth(
    BEDROCK_XBL_AUTH_PLUGIN_ID,
    "Bedrock XBL Authentication Plugin",
    BEDROCK_XBL_AUTH_PROFILE_ID,
);

export_plugin!(auth, BedrockXblAuthPlugin, MANIFEST);

#[cfg(test)]
mod tests {
    use super::{BedrockXblAuthPlugin, decode_jwt_payload};
    use base64::Engine;
    use mc_plugin_sdk_rust::auth::RustAuthPlugin;
    use serde_json::json;

    fn unsigned_jwt(header: &serde_json::Value, payload: &serde_json::Value) -> String {
        let header =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string().as_bytes());
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        format!("{header}.{payload}.")
    }

    #[test]
    fn rejects_invalid_chain() {
        let plugin = BedrockXblAuthPlugin;
        let chain = vec![unsigned_jwt(
            &json!({"alg":"none"}),
            &json!({"identityPublicKey":"invalid","extraData":{"displayName":"Builder","identity":"00000000-0000-0000-0000-000000000042","XUID":"123"}}),
        )];
        let invalid_client_data =
            unsigned_jwt(&json!({"alg":"none"}), &json!({"DisplayName":"Builder"}));
        let result = plugin.authenticate_bedrock_xbl(&chain, &invalid_client_data);
        assert!(result.is_err());
        let payload = decode_jwt_payload(&invalid_client_data).expect("payload should decode");
        assert_eq!(payload["DisplayName"], "Builder");
    }
}
