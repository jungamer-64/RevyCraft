#![allow(clippy::multiple_crate_versions)]
use mc_plugin_api::codec::auth::{AuthDescriptor, AuthMode};
use mc_plugin_sdk_rust::auth::RustAuthPlugin;
use mc_plugin_sdk_rust::capabilities::auth_capabilities;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use reqwest::StatusCode;
use revy_voxel_core::{AuthCapability, AuthCapabilitySet, PlayerId};
use serde::Deserialize;
use std::time::Duration;
use uuid::Uuid;

pub const MOJANG_ONLINE_AUTH_PROFILE_ID: &str = "mojang-online-v1";
pub const MOJANG_ONLINE_AUTH_PLUGIN_ID: &str = "auth-mojang-online";
const SESSION_SERVER_URL: &str = "https://sessionserver.mojang.com/session/minecraft/hasJoined";

#[derive(Default)]
pub struct MojangOnlineAuthPlugin;

#[derive(Deserialize)]
struct HasJoinedResponse {
    id: String,
}

impl RustAuthPlugin for MojangOnlineAuthPlugin {
    fn descriptor(&self) -> AuthDescriptor {
        AuthDescriptor {
            auth_profile: MOJANG_ONLINE_AUTH_PROFILE_ID.into(),
            mode: AuthMode::Online,
        }
    }

    fn capability_set(&self) -> AuthCapabilitySet {
        auth_capabilities(&[AuthCapability::RuntimeReload])
    }

    fn authenticate_offline(&self, _username: &str) -> Result<PlayerId, String> {
        Err("online auth plugin cannot handle offline-mode authentication".to_string())
    }

    fn authenticate_online(&self, username: &str, server_hash: &str) -> Result<PlayerId, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|error| format!("failed to build session client: {error}"))?;
        let response = client
            .get(SESSION_SERVER_URL)
            .query(&[("username", username), ("serverId", server_hash)])
            .send()
            .map_err(|error| format!("session verification request failed: {error}"))?;
        match response.status() {
            StatusCode::OK => {
                let payload: HasJoinedResponse = response
                    .json()
                    .map_err(|error| format!("invalid session verification response: {error}"))?;
                let player_id = Uuid::parse_str(&payload.id).map_err(|error| {
                    format!("invalid Mojang profile id `{}`: {error}", payload.id)
                })?;
                Ok(PlayerId(player_id))
            }
            StatusCode::NO_CONTENT | StatusCode::NOT_FOUND => {
                Err("session verification rejected".to_string())
            }
            status => Err(format!("session verification failed with HTTP {status}")),
        }
    }
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::auth(
    MOJANG_ONLINE_AUTH_PLUGIN_ID,
    "Mojang Online Authentication Plugin",
    MOJANG_ONLINE_AUTH_PROFILE_ID,
);

export_plugin!(auth, MojangOnlineAuthPlugin, MANIFEST);
