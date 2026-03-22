use crate::RuntimeError;
use mc_plugin_api::abi::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use mc_plugin_api::codec::admin_ui::AdminPermission;
use mc_plugin_host::config::{
    BootstrapConfig as PluginHostBootstrapConfig,
    RuntimeSelectionConfig as PluginHostRuntimeSelectionConfig,
};
use mc_plugin_host::host::{PluginAbiRange, PluginFailureMatrix};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

pub(crate) const BEDROCK_BASELINE_ADAPTER_ID: &str = "be-26_3";
pub(crate) const BEDROCK_OFFLINE_AUTH_PROFILE_ID: &str = "bedrock-offline-v1";
pub(crate) const DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS: u64 = 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerConfigSource {
    Inline(ServerConfig),
    Toml(PathBuf),
}

impl ServerConfigSource {
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the source cannot be materialized into a
    /// concrete [`ServerConfig`].
    pub fn load(&self) -> Result<ServerConfig, RuntimeError> {
        match self {
            Self::Inline(config) => Ok(config.clone()),
            Self::Toml(path) => ServerConfig::from_toml(path),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LevelType {
    Flat,
}

impl LevelType {
    fn parse(value: &str) -> Result<Self, RuntimeError> {
        if value.eq_ignore_ascii_case("flat") {
            Ok(Self::Flat)
        } else {
            Err(RuntimeError::Unsupported(format!(
                "level_type={value} is not supported; only `flat` is implemented"
            )))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapConfig {
    pub online_mode: bool,
    pub level_name: String,
    pub level_type: LevelType,
    pub game_mode: u8,
    pub difficulty: u8,
    pub view_distance: u8,
    pub world_dir: PathBuf,
    pub storage_profile: String,
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
            storage_profile: "je-anvil-1_7_10".to_string(),
            plugins_dir: PathBuf::from("runtime").join("plugins"),
            plugin_abi_min: CURRENT_PLUGIN_ABI,
            plugin_abi_max: CURRENT_PLUGIN_ABI,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkConfig {
    pub server_ip: Option<IpAddr>,
    pub server_port: u16,
    pub motd: String,
    pub max_players: u8,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopologyConfig {
    pub be_enabled: bool,
    pub default_adapter: String,
    pub enabled_adapters: Option<Vec<String>>,
    pub default_bedrock_adapter: String,
    pub enabled_bedrock_adapters: Option<Vec<String>>,
    pub reload_watch: bool,
    pub drain_grace_secs: u64,
}

impl Default for TopologyConfig {
    fn default() -> Self {
        Self {
            be_enabled: false,
            default_adapter: "je-1_7_10".to_string(),
            enabled_adapters: None,
            default_bedrock_adapter: BEDROCK_BASELINE_ADAPTER_ID.to_string(),
            enabled_bedrock_adapters: None,
            reload_watch: false,
            drain_grace_secs: DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginsConfig {
    pub allowlist: Option<Vec<String>>,
    pub reload_watch: bool,
    pub failure_policy: PluginFailureMatrix,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            allowlist: None,
            reload_watch: false,
            failure_policy: PluginFailureMatrix::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfilesConfig {
    pub auth: String,
    pub bedrock_auth: String,
    pub default_gameplay: String,
    pub gameplay_map: HashMap<String, String>,
}

impl Default for ProfilesConfig {
    fn default() -> Self {
        Self {
            auth: "offline-v1".to_string(),
            bedrock_auth: BEDROCK_OFFLINE_AUTH_PROFILE_ID.to_string(),
            default_gameplay: "canonical".to_string(),
            gameplay_map: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminConfig {
    pub ui_profile: String,
    pub local_console_permissions: Vec<AdminPermission>,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            ui_profile: "console-v1".to_string(),
            local_console_permissions: all_admin_permissions().to_vec(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveConfig {
    pub network: NetworkConfig,
    pub topology: TopologyConfig,
    pub plugins: PluginsConfig,
    pub profiles: ProfilesConfig,
    pub admin: AdminConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when `server.toml` cannot be read or parsed, or
    /// when it contains unsupported configuration values.
    pub fn from_toml(path: &Path) -> Result<Self, RuntimeError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let document: ServerConfigDocument =
            toml::from_str(&fs::read_to_string(path)?).map_err(|error| {
                RuntimeError::Config(format!(
                    "failed to parse config {}: {error}",
                    path.display()
                ))
            })?;
        let level_name = document
            .bootstrap
            .level_name
            .clone()
            .unwrap_or_else(|| "world".to_string());
        Ok(Self {
            bootstrap: BootstrapConfig {
                online_mode: document.bootstrap.online_mode.unwrap_or(false),
                level_name: level_name.clone(),
                level_type: LevelType::parse(
                    document.bootstrap.level_type.as_deref().unwrap_or("flat"),
                )?,
                game_mode: document.bootstrap.game_mode.unwrap_or(0),
                difficulty: document.bootstrap.difficulty.unwrap_or(1),
                view_distance: document.bootstrap.view_distance.unwrap_or(2),
                world_dir: resolve_world_dir(
                    parent,
                    document.bootstrap.world_dir.as_deref(),
                    Some(level_name.as_str()),
                ),
                storage_profile: document
                    .bootstrap
                    .storage_profile
                    .unwrap_or_else(|| "je-anvil-1_7_10".to_string()),
                plugins_dir: resolve_config_path(
                    parent,
                    document.bootstrap.plugins_dir.as_deref(),
                    Path::new("plugins"),
                ),
                plugin_abi_min: parse_plugin_abi(
                    document.bootstrap.plugin_abi_min.as_deref(),
                    "bootstrap.plugin_abi_min",
                )?
                .unwrap_or(CURRENT_PLUGIN_ABI),
                plugin_abi_max: parse_plugin_abi(
                    document.bootstrap.plugin_abi_max.as_deref(),
                    "bootstrap.plugin_abi_max",
                )?
                .unwrap_or(CURRENT_PLUGIN_ABI),
            },
            network: NetworkConfig {
                server_ip: parse_server_ip(document.network.server_ip.as_deref())?,
                server_port: document.network.server_port.unwrap_or(25565),
                motd: document
                    .network
                    .motd
                    .unwrap_or_else(|| "Multi-version Rust server".to_string()),
                max_players: document.network.max_players.unwrap_or(20),
            },
            topology: TopologyConfig {
                be_enabled: document.topology.be_enabled.unwrap_or(false),
                default_adapter: document
                    .topology
                    .default_adapter
                    .unwrap_or_else(|| "je-1_7_10".to_string()),
                enabled_adapters: normalize_optional_vec(document.topology.enabled_adapters),
                default_bedrock_adapter: document
                    .topology
                    .default_bedrock_adapter
                    .unwrap_or_else(|| BEDROCK_BASELINE_ADAPTER_ID.to_string()),
                enabled_bedrock_adapters: normalize_optional_vec(
                    document.topology.enabled_bedrock_adapters,
                ),
                reload_watch: document.topology.reload_watch.unwrap_or(false),
                drain_grace_secs: document
                    .topology
                    .drain_grace_secs
                    .unwrap_or(DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS),
            },
            plugins: PluginsConfig {
                allowlist: normalize_optional_vec(document.plugins.allowlist),
                reload_watch: document.plugins.reload_watch.unwrap_or(false),
                failure_policy: PluginFailureMatrix {
                    protocol: parse_failure_policy(
                        document.plugins.failure_policy.protocol.as_deref(),
                        PluginFailureMatrix::parse_protocol,
                        PluginFailureMatrix::default().protocol,
                    )?,
                    gameplay: parse_failure_policy(
                        document.plugins.failure_policy.gameplay.as_deref(),
                        PluginFailureMatrix::parse_gameplay,
                        PluginFailureMatrix::default().gameplay,
                    )?,
                    storage: parse_failure_policy(
                        document.plugins.failure_policy.storage.as_deref(),
                        PluginFailureMatrix::parse_storage,
                        PluginFailureMatrix::default().storage,
                    )?,
                    auth: parse_failure_policy(
                        document.plugins.failure_policy.auth.as_deref(),
                        PluginFailureMatrix::parse_auth,
                        PluginFailureMatrix::default().auth,
                    )?,
                    admin_ui: parse_failure_policy(
                        document.plugins.failure_policy.admin_ui.as_deref(),
                        PluginFailureMatrix::parse_admin_ui,
                        PluginFailureMatrix::default().admin_ui,
                    )?,
                },
            },
            profiles: ProfilesConfig {
                auth: document
                    .profiles
                    .auth
                    .unwrap_or_else(|| "offline-v1".to_string()),
                bedrock_auth: document
                    .profiles
                    .bedrock_auth
                    .unwrap_or_else(|| BEDROCK_OFFLINE_AUTH_PROFILE_ID.to_string()),
                default_gameplay: document
                    .profiles
                    .default_gameplay
                    .unwrap_or_else(|| "canonical".to_string()),
                gameplay_map: document.profiles.gameplay_map,
            },
            admin: AdminConfig {
                ui_profile: document
                    .admin
                    .ui_profile
                    .unwrap_or_else(|| "console-v1".to_string()),
                local_console_permissions: parse_admin_permissions(
                    document.admin.local_console_permissions,
                )?,
            },
        })
    }

    #[must_use]
    pub fn bind_addr(&self) -> SocketAddr {
        SocketAddr::new(
            self.network
                .server_ip
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            self.network.server_port,
        )
    }

    pub(crate) fn effective_enabled_adapters(&self) -> Vec<String> {
        self.topology
            .enabled_adapters
            .as_ref()
            .map_or_else(|| vec![self.topology.default_adapter.clone()], Clone::clone)
    }

    pub(crate) fn effective_enabled_bedrock_adapters(&self) -> Vec<String> {
        self.topology.enabled_bedrock_adapters.as_ref().map_or_else(
            || vec![self.topology.default_bedrock_adapter.clone()],
            Clone::clone,
        )
    }

    #[must_use]
    pub fn live_config(&self) -> LiveConfig {
        LiveConfig {
            network: self.network.clone(),
            topology: self.topology.clone(),
            plugins: self.plugins.clone(),
            profiles: self.profiles.clone(),
            admin: self.admin.clone(),
        }
    }

    #[must_use]
    pub fn plugin_host_bootstrap_config(&self) -> PluginHostBootstrapConfig {
        PluginHostBootstrapConfig {
            storage_profile: self.bootstrap.storage_profile.clone(),
            plugins_dir: self.bootstrap.plugins_dir.clone(),
            plugin_abi_min: self.bootstrap.plugin_abi_min,
            plugin_abi_max: self.bootstrap.plugin_abi_max,
        }
    }

    #[must_use]
    pub fn plugin_host_runtime_selection_config(&self) -> PluginHostRuntimeSelectionConfig {
        PluginHostRuntimeSelectionConfig {
            be_enabled: self.topology.be_enabled,
            auth_profile: self.profiles.auth.clone(),
            bedrock_auth_profile: self.profiles.bedrock_auth.clone(),
            default_gameplay_profile: self.profiles.default_gameplay.clone(),
            gameplay_profile_map: self.profiles.gameplay_map.clone(),
            admin_ui_profile: self.admin.ui_profile.clone(),
            plugin_allowlist: self.plugins.allowlist.clone(),
            plugin_failure_policy_protocol: self.plugins.failure_policy.protocol,
            plugin_failure_policy_gameplay: self.plugins.failure_policy.gameplay,
            plugin_failure_policy_storage: self.plugins.failure_policy.storage,
            plugin_failure_policy_auth: self.plugins.failure_policy.auth,
            plugin_failure_policy_admin_ui: self.plugins.failure_policy.admin_ui,
        }
    }

    #[must_use]
    pub fn plugin_host_config(&self) -> mc_plugin_host::config::ServerConfig {
        mc_plugin_host::config::ServerConfig {
            be_enabled: self.topology.be_enabled,
            storage_profile: self.bootstrap.storage_profile.clone(),
            auth_profile: self.profiles.auth.clone(),
            bedrock_auth_profile: self.profiles.bedrock_auth.clone(),
            default_gameplay_profile: self.profiles.default_gameplay.clone(),
            gameplay_profile_map: self.profiles.gameplay_map.clone(),
            admin_ui_profile: self.admin.ui_profile.clone(),
            plugins_dir: self.bootstrap.plugins_dir.clone(),
            plugin_allowlist: self.plugins.allowlist.clone(),
            plugin_failure_policy_protocol: self.plugins.failure_policy.protocol,
            plugin_failure_policy_gameplay: self.plugins.failure_policy.gameplay,
            plugin_failure_policy_storage: self.plugins.failure_policy.storage,
            plugin_failure_policy_auth: self.plugins.failure_policy.auth,
            plugin_failure_policy_admin_ui: self.plugins.failure_policy.admin_ui,
            plugin_abi_min: self.bootstrap.plugin_abi_min,
            plugin_abi_max: self.bootstrap.plugin_abi_max,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ServerConfigDocument {
    bootstrap: BootstrapDocument,
    network: NetworkDocument,
    topology: TopologyDocument,
    plugins: PluginsDocument,
    profiles: ProfilesDocument,
    admin: AdminDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct BootstrapDocument {
    online_mode: Option<bool>,
    level_name: Option<String>,
    level_type: Option<String>,
    game_mode: Option<u8>,
    difficulty: Option<u8>,
    view_distance: Option<u8>,
    world_dir: Option<PathBuf>,
    storage_profile: Option<String>,
    plugins_dir: Option<PathBuf>,
    plugin_abi_min: Option<String>,
    plugin_abi_max: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct NetworkDocument {
    server_ip: Option<String>,
    server_port: Option<u16>,
    motd: Option<String>,
    max_players: Option<u8>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TopologyDocument {
    be_enabled: Option<bool>,
    default_adapter: Option<String>,
    enabled_adapters: Option<Vec<String>>,
    default_bedrock_adapter: Option<String>,
    enabled_bedrock_adapters: Option<Vec<String>>,
    reload_watch: Option<bool>,
    drain_grace_secs: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PluginsDocument {
    allowlist: Option<Vec<String>>,
    reload_watch: Option<bool>,
    failure_policy: FailurePolicyDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FailurePolicyDocument {
    protocol: Option<String>,
    gameplay: Option<String>,
    storage: Option<String>,
    auth: Option<String>,
    admin_ui: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ProfilesDocument {
    auth: Option<String>,
    bedrock_auth: Option<String>,
    default_gameplay: Option<String>,
    gameplay_map: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AdminDocument {
    ui_profile: Option<String>,
    local_console_permissions: Option<Vec<String>>,
}

fn normalize_optional_vec(values: Option<Vec<String>>) -> Option<Vec<String>> {
    match values {
        Some(values) if values.is_empty() => None,
        other => other,
    }
}

fn resolve_world_dir(parent: &Path, explicit: Option<&Path>, level_name: Option<&str>) -> PathBuf {
    let world_dir = explicit
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(level_name.unwrap_or("world")));
    if world_dir.is_relative() {
        parent.join(world_dir)
    } else {
        world_dir
    }
}

fn resolve_config_path(parent: &Path, explicit: Option<&Path>, default_relative: &Path) -> PathBuf {
    let path = explicit
        .map(PathBuf::from)
        .unwrap_or_else(|| default_relative.to_path_buf());
    if path.is_relative() {
        parent.join(path)
    } else {
        path
    }
}

fn parse_server_ip(value: Option<&str>) -> Result<Option<IpAddr>, RuntimeError> {
    match value {
        None | Some("") => Ok(None),
        Some(value) => value
            .parse()
            .map(Some)
            .map_err(|_| RuntimeError::Config("invalid network.server_ip".to_string())),
    }
}

fn parse_failure_policy<F, E>(
    value: Option<&str>,
    parser: F,
    default: mc_plugin_host::host::PluginFailureAction,
) -> Result<mc_plugin_host::host::PluginFailureAction, RuntimeError>
where
    F: Fn(&str) -> Result<mc_plugin_host::host::PluginFailureAction, E>,
    E: Into<RuntimeError>,
{
    match value {
        Some(value) => parser(value).map_err(Into::into),
        None => Ok(default),
    }
}

const fn all_admin_permissions() -> [AdminPermission; 6] {
    [
        AdminPermission::Status,
        AdminPermission::Sessions,
        AdminPermission::ReloadConfig,
        AdminPermission::ReloadPlugins,
        AdminPermission::ReloadTopology,
        AdminPermission::Shutdown,
    ]
}

fn parse_admin_permissions(
    values: Option<Vec<String>>,
) -> Result<Vec<AdminPermission>, RuntimeError> {
    let Some(values) = values else {
        return Ok(all_admin_permissions().to_vec());
    };
    let mut permissions = Vec::new();
    for value in values {
        let permission = match value.as_str() {
            "status" => AdminPermission::Status,
            "sessions" => AdminPermission::Sessions,
            "reload-config" | "reload_config" => AdminPermission::ReloadConfig,
            "reload-plugins" | "reload_plugins" => AdminPermission::ReloadPlugins,
            "reload-topology" | "reload_topology" => AdminPermission::ReloadTopology,
            "shutdown" => AdminPermission::Shutdown,
            _ => {
                return Err(RuntimeError::Config(format!(
                    "unsupported admin.local_console_permissions entry `{value}`"
                )));
            }
        };
        if !permissions.contains(&permission) {
            permissions.push(permission);
        }
    }
    Ok(permissions)
}

fn parse_plugin_abi(
    value: Option<&str>,
    key: &str,
) -> Result<Option<PluginAbiVersion>, RuntimeError> {
    value
        .map(|value| {
            PluginAbiRange::parse_version(value)
                .map_err(|_| RuntimeError::Config(format!("invalid {key} `{value}`")))
        })
        .transpose()
}
