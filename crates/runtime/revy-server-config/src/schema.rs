use revy_voxel_semantic::{
    AdapterId, AdminSurfaceProfileId, AuthProfileId, GameplayProfileId, StorageProfileId,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ServerConfigDocument {
    #[serde(rename = "static")]
    pub(crate) static_config: StaticDocument,
    pub(crate) live: LiveDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct StaticDocument {
    pub(crate) bootstrap: StaticBootstrapDocument,
    pub(crate) plugins: StaticPluginsDocument,
    pub(crate) admin: StaticAdminDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct StaticBootstrapDocument {
    pub(crate) online_mode: Option<bool>,
    pub(crate) level_name: Option<String>,
    pub(crate) level_type: Option<String>,
    pub(crate) game_mode: Option<u8>,
    pub(crate) difficulty: Option<u8>,
    pub(crate) view_distance: Option<u8>,
    pub(crate) world_dir: Option<PathBuf>,
    pub(crate) storage_profile: Option<StorageProfileId>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct StaticPluginsDocument {
    pub(crate) plugins_dir: Option<PathBuf>,
    pub(crate) plugin_abi_min: Option<String>,
    pub(crate) plugin_abi_max: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct StaticAdminDocument {
    pub(crate) principals: HashMap<String, AdminPrincipalDocument>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LiveDocument {
    pub(crate) network: NetworkDocument,
    pub(crate) topology: TopologyDocument,
    pub(crate) plugins: PluginsDocument,
    pub(crate) profiles: ProfilesDocument,
    pub(crate) admin: LiveAdminDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct NetworkDocument {
    pub(crate) server_ip: Option<String>,
    pub(crate) server_port: Option<u16>,
    pub(crate) motd: Option<String>,
    pub(crate) max_players: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct TopologyDocument {
    pub(crate) be_enabled: Option<bool>,
    pub(crate) default_adapter: Option<AdapterId>,
    pub(crate) enabled_adapters: Option<Vec<AdapterId>>,
    pub(crate) default_bedrock_adapter: Option<AdapterId>,
    pub(crate) enabled_bedrock_adapters: Option<Vec<AdapterId>>,
    pub(crate) reload_watch: Option<bool>,
    pub(crate) drain_grace_secs: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct PluginsDocument {
    pub(crate) allowlist: Option<Vec<String>>,
    pub(crate) reload_watch: Option<bool>,
    pub(crate) buffer_limits: PluginBufferLimitsDocument,
    pub(crate) failure_policy: FailurePolicyDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct PluginBufferLimitsDocument {
    pub(crate) protocol_response_bytes: Option<usize>,
    pub(crate) gameplay_response_bytes: Option<usize>,
    pub(crate) storage_response_bytes: Option<usize>,
    pub(crate) auth_response_bytes: Option<usize>,
    pub(crate) admin_surface_response_bytes: Option<usize>,
    pub(crate) callback_payload_bytes: Option<usize>,
    pub(crate) metadata_bytes: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FailurePolicyDocument {
    pub(crate) protocol: Option<String>,
    pub(crate) gameplay: Option<String>,
    pub(crate) storage: Option<String>,
    pub(crate) auth: Option<String>,
    pub(crate) admin_surface: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ProfilesDocument {
    pub(crate) auth: Option<AuthProfileId>,
    pub(crate) bedrock_auth: Option<AuthProfileId>,
    pub(crate) default_gameplay: Option<GameplayProfileId>,
    pub(crate) gameplay_map: HashMap<AdapterId, GameplayProfileId>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LiveAdminDocument {
    pub(crate) surfaces: HashMap<String, AdminSurfaceDocument>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct AdminPrincipalDocument {
    pub(crate) permissions: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct AdminSurfaceDocument {
    pub(crate) profile: Option<AdminSurfaceProfileId>,
    pub(crate) config: Option<PathBuf>,
}
