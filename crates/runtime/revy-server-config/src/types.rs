use crate::error::ServerConfigError;
use mc_plugin_contract::plugin::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use revy_server_types::{AdminPermission, PluginFailureMatrix};
use revy_voxel_semantic::{
    AdapterId, AdminSurfaceProfileId, AuthProfileId, GameplayProfileId, StorageProfileId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

pub const BEDROCK_BASELINE_ADAPTER_ID: &str = "be-924";
pub const BEDROCK_OFFLINE_AUTH_PROFILE_ID: &str = "bedrock-offline-v1";
pub const DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS: u64 = 30;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LevelType {
    Flat,
}

impl LevelType {
    pub(crate) fn parse(value: &str) -> Result<Self, ServerConfigError> {
        if value.eq_ignore_ascii_case("flat") {
            Ok(Self::Flat)
        } else {
            Err(ServerConfigError::Unsupported(format!(
                "level_type={value} is not supported; only `flat` is implemented"
            )))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapConfig {
    pub online_mode: bool,
    pub level_name: String,
    pub level_type: LevelType,
    pub game_mode: u8,
    pub difficulty: u8,
    pub view_distance: u8,
    pub world_dir: PathBuf,
    pub storage_profile: StorageProfileId,
    pub plugins_dir: PathBuf,
    pub plugin_abi_min: PluginAbiVersion,
    pub plugin_abi_max: PluginAbiVersion,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            online_mode: false,
            level_name: "world".to_string(),
            level_type: LevelType::Flat,
            game_mode: 0,
            difficulty: 1,
            view_distance: 2,
            world_dir: PathBuf::from("runtime").join("world"),
            storage_profile: StorageProfileId::new("je-anvil-1_7_10"),
            plugins_dir: PathBuf::from("runtime").join("plugins"),
            plugin_abi_min: CURRENT_PLUGIN_ABI,
            plugin_abi_max: CURRENT_PLUGIN_ABI,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub server_ip: Option<IpAddr>,
    pub server_port: u16,
    pub motd: String,
    pub max_players: u32,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            server_ip: None,
            server_port: 25565,
            motd: "Multi-version Rust server".to_string(),
            max_players: 20,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyConfig {
    pub be_enabled: bool,
    pub default_adapter: AdapterId,
    pub enabled_adapters: Option<Vec<AdapterId>>,
    pub default_bedrock_adapter: AdapterId,
    pub enabled_bedrock_adapters: Option<Vec<AdapterId>>,
    pub reload_watch: bool,
    pub drain_grace_secs: u64,
}

impl Default for TopologyConfig {
    fn default() -> Self {
        Self {
            be_enabled: false,
            default_adapter: AdapterId::new("je-5"),
            enabled_adapters: None,
            default_bedrock_adapter: AdapterId::new(BEDROCK_BASELINE_ADAPTER_ID),
            enabled_bedrock_adapters: None,
            reload_watch: false,
            drain_grace_secs: DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginBufferLimits {
    pub protocol_response_bytes: usize,
    pub gameplay_response_bytes: usize,
    pub storage_response_bytes: usize,
    pub auth_response_bytes: usize,
    pub admin_surface_response_bytes: usize,
    pub callback_payload_bytes: usize,
    pub metadata_bytes: usize,
}

impl Default for PluginBufferLimits {
    fn default() -> Self {
        const KIB: usize = 1024;
        const MIB: usize = 1024 * KIB;
        Self {
            protocol_response_bytes: 4 * MIB,
            gameplay_response_bytes: 1 * MIB,
            storage_response_bytes: 32 * MIB,
            auth_response_bytes: 256 * KIB,
            admin_surface_response_bytes: 1 * MIB,
            callback_payload_bytes: 1 * MIB,
            metadata_bytes: 64 * KIB,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginsConfig {
    pub allowlist: Option<Vec<String>>,
    pub reload_watch: bool,
    pub buffer_limits: PluginBufferLimits,
    pub failure_policy: PluginFailureMatrix,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            allowlist: None,
            reload_watch: false,
            buffer_limits: PluginBufferLimits::default(),
            failure_policy: PluginFailureMatrix::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilesConfig {
    pub auth: AuthProfileId,
    pub bedrock_auth: AuthProfileId,
    pub default_gameplay: GameplayProfileId,
    pub gameplay_map: HashMap<AdapterId, GameplayProfileId>,
}

impl Default for ProfilesConfig {
    fn default() -> Self {
        Self {
            auth: AuthProfileId::new("offline-v1"),
            bedrock_auth: AuthProfileId::new(BEDROCK_OFFLINE_AUTH_PROFILE_ID),
            default_gameplay: GameplayProfileId::new("canonical"),
            gameplay_map: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminConfig {
    pub surfaces: HashMap<String, AdminSurfaceConfig>,
    pub principals: HashMap<String, AdminPrincipalConfig>,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            surfaces: HashMap::new(),
            principals: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSurfaceConfig {
    pub profile: AdminSurfaceProfileId,
    pub config: Option<PathBuf>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminPrincipalConfig {
    pub permissions: Vec<AdminPermission>,
}

impl Debug for AdminPrincipalConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminPrincipalConfig")
            .field("permissions", &self.permissions)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bootstrap: BootstrapConfig,
    pub network: NetworkConfig,
    pub topology: TopologyConfig,
    pub plugins: PluginsConfig,
    pub profiles: ProfilesConfig,
    pub admin: AdminConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bootstrap: BootstrapConfig::default(),
            network: NetworkConfig::default(),
            topology: TopologyConfig::default(),
            plugins: PluginsConfig::default(),
            profiles: ProfilesConfig::default(),
            admin: AdminConfig::default(),
        }
    }
}

impl ServerConfig {
    #[must_use]
    pub fn bind_addr(&self) -> SocketAddr {
        SocketAddr::new(
            self.network
                .server_ip
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            self.network.server_port,
        )
    }

    #[must_use]
    pub fn effective_enabled_adapters(&self) -> Vec<AdapterId> {
        self.topology
            .enabled_adapters
            .as_ref()
            .map_or_else(|| vec![self.topology.default_adapter.clone()], Clone::clone)
    }

    #[must_use]
    pub fn effective_enabled_bedrock_adapters(&self) -> Vec<AdapterId> {
        self.topology.enabled_bedrock_adapters.as_ref().map_or_else(
            || vec![self.topology.default_bedrock_adapter.clone()],
            Clone::clone,
        )
    }
}
