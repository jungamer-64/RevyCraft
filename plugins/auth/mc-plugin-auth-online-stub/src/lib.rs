#![allow(clippy::multiple_crate_versions)]
use mc_core::{CapabilitySet, PlayerId};
use mc_plugin_api::codec::auth::{AuthDescriptor, AuthMode};
use mc_plugin_sdk_rust::auth::RustAuthPlugin;
use mc_plugin_sdk_rust::capabilities::capability_set as build_capability_set;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use uuid::Uuid;

pub const ONLINE_STUB_AUTH_PROFILE_ID: &str = "mojang-online-v1";
pub const ONLINE_STUB_AUTH_PLUGIN_ID: &str = "auth-online-stub";

#[derive(Default)]
pub struct OnlineStubAuthPlugin;

impl RustAuthPlugin for OnlineStubAuthPlugin {
    fn descriptor(&self) -> AuthDescriptor {
        AuthDescriptor {
            auth_profile: ONLINE_STUB_AUTH_PROFILE_ID.to_string(),
            mode: AuthMode::Online,
        }
    }

    fn capability_set(&self) -> CapabilitySet {
        build_capability_set(&[
            "auth.online",
            "auth.profile.mojang-online-v1",
            "runtime.reload.auth",
        ])
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
    &[
        "auth.profile:mojang-online-v1",
        "auth.mode:online",
        "runtime.reload.auth",
    ],
);

export_plugin!(auth, OnlineStubAuthPlugin, MANIFEST);
