#![allow(clippy::multiple_crate_versions)]
use mc_core::CapabilitySet;
use mc_plugin_api::{AuthDescriptor, AuthMode, BedrockAuthResult};
use mc_plugin_sdk_rust::{RustAuthPlugin, StaticPluginManifest, export_auth_plugin};
use md5::{Digest, Md5};
use uuid::Uuid;

pub const BEDROCK_OFFLINE_AUTH_PROFILE_ID: &str = "bedrock-offline-v1";
pub const BEDROCK_OFFLINE_AUTH_PLUGIN_ID: &str = "auth-bedrock-offline";

#[derive(Default)]
pub struct BedrockOfflineAuthPlugin;

impl RustAuthPlugin for BedrockOfflineAuthPlugin {
    fn descriptor(&self) -> AuthDescriptor {
        AuthDescriptor {
            auth_profile: BEDROCK_OFFLINE_AUTH_PROFILE_ID.to_string(),
            mode: AuthMode::BedrockOffline,
        }
    }

    fn capability_set(&self) -> CapabilitySet {
        let mut capabilities = CapabilitySet::new();
        let _ = capabilities.insert("auth.bedrock");
        let _ = capabilities.insert("auth.bedrock.offline");
        let _ = capabilities.insert("auth.profile.bedrock-offline-v1");
        let _ = capabilities.insert("runtime.reload.auth");
        if let Some(build_tag) = option_env!("REVY_PLUGIN_BUILD_TAG") {
            let _ = capabilities.insert(format!("build-tag:{build_tag}"));
        }
        capabilities
    }

    fn authenticate_offline(&self, _username: &str) -> Result<mc_core::PlayerId, String> {
        Err("bedrock offline auth plugin only handles bedrock auth requests".to_string())
    }

    fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, String> {
        let mut hasher = Md5::new();
        hasher.update(format!("BedrockOfflinePlayer:{display_name}").as_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest);
        bytes[6] = (bytes[6] & 0x0f) | 0x30;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Ok(BedrockAuthResult {
            player_id: mc_core::PlayerId(Uuid::from_bytes(bytes)),
            display_name: display_name.to_string(),
            xuid: None,
            identity_uuid: None,
            skin_claims_blob: Vec::new(),
        })
    }
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::auth(
    BEDROCK_OFFLINE_AUTH_PLUGIN_ID,
    "Bedrock Offline Authentication Plugin",
    &[
        "auth.profile:bedrock-offline-v1",
        "auth.mode:bedrock-offline",
        "runtime.reload.auth",
    ],
);

export_auth_plugin!(BedrockOfflineAuthPlugin, MANIFEST);
