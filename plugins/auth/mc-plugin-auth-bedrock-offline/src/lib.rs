#![allow(clippy::multiple_crate_versions)]
use mc_core::CapabilitySet;
use mc_plugin_api::codec::auth::{AuthDescriptor, AuthMode, BedrockAuthResult};
use mc_plugin_sdk_rust::capabilities::capability_set as build_capability_set;
use mc_plugin_sdk_rust::auth::RustAuthPlugin;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
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
        build_capability_set(&[
            "auth.bedrock",
            "auth.bedrock.offline",
            "auth.profile.bedrock-offline-v1",
            "runtime.reload.auth",
        ])
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

export_plugin!(auth, BedrockOfflineAuthPlugin, MANIFEST);
