#![allow(clippy::multiple_crate_versions)]
use mc_plugin_api::codec::auth::{AuthDescriptor, AuthMode, BedrockAuthResult};
use mc_plugin_sdk_rust::auth::RustAuthPlugin;
use mc_plugin_sdk_rust::capabilities::auth_capabilities;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use revy_voxel_core::{AuthCapability, AuthCapabilitySet};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const BEDROCK_OFFLINE_AUTH_PROFILE_ID: &str = "bedrock-offline-v1";
pub const BEDROCK_OFFLINE_AUTH_PLUGIN_ID: &str = "auth-bedrock-offline";

#[derive(Default)]
pub struct BedrockOfflineAuthPlugin;

impl RustAuthPlugin for BedrockOfflineAuthPlugin {
    fn descriptor(&self) -> AuthDescriptor {
        AuthDescriptor {
            auth_profile: BEDROCK_OFFLINE_AUTH_PROFILE_ID.into(),
            mode: AuthMode::BedrockOffline,
        }
    }

    fn capability_set(&self) -> AuthCapabilitySet {
        auth_capabilities(&[AuthCapability::RuntimeReload])
    }

    fn authenticate_offline(&self, _username: &str) -> Result<revy_voxel_core::PlayerId, String> {
        Err("bedrock offline auth plugin only handles bedrock auth requests".to_string())
    }

    fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, String> {
        let mut hasher = Sha256::new();
        hasher.update(format!("BedrockOfflinePlayer:{display_name}").as_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x30;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Ok(BedrockAuthResult {
            player_id: revy_voxel_core::PlayerId(Uuid::from_bytes(bytes)),
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
    BEDROCK_OFFLINE_AUTH_PROFILE_ID,
);

export_plugin!(auth, BedrockOfflineAuthPlugin, MANIFEST);
