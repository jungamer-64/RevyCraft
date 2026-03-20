#![allow(clippy::multiple_crate_versions)]
use mc_core::{CapabilitySet, PlayerId};
use mc_plugin_api::codec::auth::{AuthDescriptor, AuthMode};
use mc_plugin_sdk_rust::auth::{RustAuthPlugin, export_auth_plugin};
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use md5::{Digest, Md5};
use uuid::Uuid;

pub const OFFLINE_AUTH_PROFILE_ID: &str = "offline-v1";
pub const OFFLINE_AUTH_PLUGIN_ID: &str = "auth-offline";

#[derive(Default)]
pub struct OfflineAuthPlugin;

impl RustAuthPlugin for OfflineAuthPlugin {
    fn descriptor(&self) -> AuthDescriptor {
        AuthDescriptor {
            auth_profile: OFFLINE_AUTH_PROFILE_ID.to_string(),
            mode: AuthMode::Offline,
        }
    }

    fn capability_set(&self) -> CapabilitySet {
        let mut capabilities = CapabilitySet::new();
        let _ = capabilities.insert("auth.offline");
        let _ = capabilities.insert("auth.profile.offline-v1");
        let _ = capabilities.insert("runtime.reload.auth");
        if let Some(build_tag) = option_env!("REVY_PLUGIN_BUILD_TAG") {
            let _ = capabilities.insert(format!("build-tag:{build_tag}"));
        }
        capabilities
    }

    fn authenticate_offline(&self, username: &str) -> Result<PlayerId, String> {
        let mut hasher = Md5::new();
        hasher.update(format!("OfflinePlayer:{username}").as_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest);
        bytes[6] = (bytes[6] & 0x0f) | 0x30;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Ok(PlayerId(Uuid::from_bytes(bytes)))
    }

    fn authenticate_online(&self, _username: &str, _server_hash: &str) -> Result<PlayerId, String> {
        Err("offline auth plugin cannot handle online-mode authentication".to_string())
    }
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::auth(
    OFFLINE_AUTH_PLUGIN_ID,
    "Offline Authentication Plugin",
    &[
        "auth.profile:offline-v1",
        "auth.mode:offline",
        "runtime.reload.auth",
    ],
);

export_auth_plugin!(OfflineAuthPlugin, MANIFEST);
