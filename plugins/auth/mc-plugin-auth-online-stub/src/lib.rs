#![allow(clippy::multiple_crate_versions)]
use mc_plugin_api::codec::auth::{AuthDescriptor, AuthMode};
use mc_plugin_sdk_rust::auth::RustAuthPlugin;
use mc_plugin_sdk_rust::capabilities::auth_capabilities;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use mc_plugin_sdk_rust::{AuthCapability, AuthCapabilitySet, PlayerId};
use uuid::Uuid;

pub const ONLINE_STUB_AUTH_PROFILE_ID: &str = "mojang-online-v1";
pub const ONLINE_STUB_AUTH_PLUGIN_ID: &str = "auth-online-stub";

#[derive(Default)]
pub struct OnlineStubAuthPlugin;

impl RustAuthPlugin for OnlineStubAuthPlugin {
    fn descriptor(&self) -> AuthDescriptor {
        AuthDescriptor {
            auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.into(),
            mode: AuthMode::Online,
        }
    }

    fn capability_set(&self) -> AuthCapabilitySet {
        auth_capabilities(&[AuthCapability::RuntimeReload])
    }

    fn authenticate_offline(&self, _username: &str) -> Result<PlayerId, String> {
        Err("online auth stub cannot handle offline-mode authentication".to_string())
    }

    fn authenticate_online(&self, username: &str, server_hash: &str) -> Result<PlayerId, String> {
        Ok(PlayerId(Uuid::new_v3(
            &Uuid::NAMESPACE_URL,
            format!("{username}:{server_hash}").as_bytes(),
        )))
    }
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::auth(
    ONLINE_STUB_AUTH_PLUGIN_ID,
    "Online Authentication Stub Plugin",
    ONLINE_STUB_AUTH_PROFILE_ID,
);

export_plugin!(auth, OnlineStubAuthPlugin, MANIFEST);
