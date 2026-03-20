use crate::RuntimeError;
use mc_plugin_api::abi::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use mc_plugin_host::{PluginAbiRange, PluginFailureAction, PluginFailureMatrix};
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
    Properties(PathBuf),
}

impl ServerConfigSource {
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the source cannot be materialized into a
    /// concrete [`ServerConfig`].
    pub fn load(&self) -> Result<ServerConfig, RuntimeError> {
        match self {
            Self::Inline(config) => Ok(config.clone()),
            Self::Properties(path) => ServerConfig::from_properties(path),
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
                "level-type={value} is not supported; only FLAT is implemented"
            )))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub server_ip: Option<IpAddr>,
    pub server_port: u16,
    pub be_enabled: bool,
    pub motd: String,
    pub max_players: u8,
    pub online_mode: bool,
    pub level_name: String,
    pub level_type: LevelType,
    pub game_mode: u8,
    pub difficulty: u8,
    pub view_distance: u8,
    pub default_adapter: String,
    pub enabled_adapters: Option<Vec<String>>,
    pub default_bedrock_adapter: String,
    pub enabled_bedrock_adapters: Option<Vec<String>>,
    pub storage_profile: String,
    pub auth_profile: String,
    pub bedrock_auth_profile: String,
    pub default_gameplay_profile: String,
    pub gameplay_profile_map: HashMap<String, String>,
    pub plugins_dir: PathBuf,
    pub plugin_allowlist: Option<Vec<String>>,
    pub plugin_failure_policy_protocol: PluginFailureAction,
    pub plugin_failure_policy_gameplay: PluginFailureAction,
    pub plugin_failure_policy_storage: PluginFailureAction,
    pub plugin_failure_policy_auth: PluginFailureAction,
    pub plugin_reload_watch: bool,
    pub topology_reload_watch: bool,
    pub topology_drain_grace_secs: u64,
    pub plugin_abi_min: PluginAbiVersion,
    pub plugin_abi_max: PluginAbiVersion,
    pub world_dir: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let failure_matrix = PluginFailureMatrix::default();
        Self {
            server_ip: None,
            server_port: 25565,
            be_enabled: false,
            motd: "Multi-version Rust server".to_string(),
            max_players: 20,
            online_mode: false,
            level_name: "world".to_string(),
            level_type: LevelType::Flat,
            game_mode: 0,
            difficulty: 1,
            view_distance: 2,
            default_adapter: "je-1_7_10".to_string(),
            enabled_adapters: None,
            default_bedrock_adapter: BEDROCK_BASELINE_ADAPTER_ID.to_string(),
            enabled_bedrock_adapters: None,
            storage_profile: "je-anvil-1_7_10".to_string(),
            auth_profile: "offline-v1".to_string(),
            bedrock_auth_profile: BEDROCK_OFFLINE_AUTH_PROFILE_ID.to_string(),
            default_gameplay_profile: "canonical".to_string(),
            gameplay_profile_map: HashMap::new(),
            plugins_dir: PathBuf::from("runtime").join("plugins"),
            plugin_allowlist: None,
            plugin_failure_policy_protocol: failure_matrix.protocol,
            plugin_failure_policy_gameplay: failure_matrix.gameplay,
            plugin_failure_policy_storage: failure_matrix.storage,
            plugin_failure_policy_auth: failure_matrix.auth,
            plugin_reload_watch: false,
            topology_reload_watch: false,
            topology_drain_grace_secs: DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS,
            plugin_abi_min: CURRENT_PLUGIN_ABI,
            plugin_abi_max: CURRENT_PLUGIN_ABI,
            world_dir: PathBuf::from("runtime").join("world"),
        }
    }
}

impl ServerConfig {
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when `server.properties` cannot be read or
    /// parsed, or when it contains unsupported configuration values.
    pub fn from_properties(path: &Path) -> Result<Self, RuntimeError> {
        let mut config = Self::default();
        if path.exists() {
            let contents = fs::read_to_string(path)?;
            for raw_line in contents.lines() {
                apply_property_line(&mut config, raw_line)?;
            }
        }
        finalize_relative_paths(&mut config, path.parent().unwrap_or_else(|| Path::new(".")));
        Ok(config)
    }

    #[must_use]
    pub fn bind_addr(&self) -> SocketAddr {
        SocketAddr::new(
            self.server_ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            self.server_port,
        )
    }

    pub(crate) fn effective_enabled_adapters(&self) -> Vec<String> {
        self.enabled_adapters
            .as_ref()
            .map_or_else(|| vec![self.default_adapter.clone()], Clone::clone)
    }

    pub(crate) fn effective_enabled_bedrock_adapters(&self) -> Vec<String> {
        self.enabled_bedrock_adapters
            .as_ref()
            .map_or_else(|| vec![self.default_bedrock_adapter.clone()], Clone::clone)
    }

    pub(crate) fn apply_topology_from(&mut self, other: &Self) -> bool {
        let previous = self.clone();
        self.server_ip = other.server_ip;
        self.server_port = other.server_port;
        self.be_enabled = other.be_enabled;
        self.motd.clone_from(&other.motd);
        self.max_players = other.max_players;
        self.default_adapter.clone_from(&other.default_adapter);
        self.enabled_adapters.clone_from(&other.enabled_adapters);
        self.default_bedrock_adapter
            .clone_from(&other.default_bedrock_adapter);
        self.enabled_bedrock_adapters
            .clone_from(&other.enabled_bedrock_adapters);
        self.topology_reload_watch = other.topology_reload_watch;
        self.topology_drain_grace_secs = other.topology_drain_grace_secs;
        previous != *self
    }

    #[must_use]
    pub fn plugin_host_config(&self) -> mc_plugin_host::config::ServerConfig {
        mc_plugin_host::config::ServerConfig {
            be_enabled: self.be_enabled,
            storage_profile: self.storage_profile.clone(),
            auth_profile: self.auth_profile.clone(),
            bedrock_auth_profile: self.bedrock_auth_profile.clone(),
            default_gameplay_profile: self.default_gameplay_profile.clone(),
            gameplay_profile_map: self.gameplay_profile_map.clone(),
            plugins_dir: self.plugins_dir.clone(),
            plugin_allowlist: self.plugin_allowlist.clone(),
            plugin_failure_policy_protocol: self.plugin_failure_policy_protocol,
            plugin_failure_policy_gameplay: self.plugin_failure_policy_gameplay,
            plugin_failure_policy_storage: self.plugin_failure_policy_storage,
            plugin_failure_policy_auth: self.plugin_failure_policy_auth,
            plugin_abi_min: self.plugin_abi_min,
            plugin_abi_max: self.plugin_abi_max,
        }
    }
}

fn apply_property_line(config: &mut ServerConfig, raw_line: &str) -> Result<(), RuntimeError> {
    let line = raw_line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(());
    }
    let Some((key, value)) = line.split_once('=') else {
        return Ok(());
    };
    apply_property(config, key.trim(), value.trim())
}

fn apply_property(config: &mut ServerConfig, key: &str, value: &str) -> Result<(), RuntimeError> {
    match key {
        "server-ip" => {
            config.server_ip = parse_server_ip(value)?;
        }
        "server-port" => config.server_port = parse_u16(value, "server-port")?,
        "be-enabled" => config.be_enabled = parse_bool_flag(value),
        "motd" => config.motd = value.to_string(),
        "max-players" => config.max_players = parse_u8(value, "max-players")?,
        "online-mode" => config.online_mode = parse_bool_flag(value),
        "level-name" => config.level_name = value.to_string(),
        "level-type" => config.level_type = LevelType::parse(value)?,
        "gamemode" => config.game_mode = parse_u8(value, "gamemode")?,
        "difficulty" => config.difficulty = parse_u8(value, "difficulty")?,
        "view-distance" => config.view_distance = parse_u8(value, "view-distance")?,
        "default-adapter" => config.default_adapter = value.to_string(),
        "enabled-adapters" => config.enabled_adapters = parse_enabled_adapters(value),
        "default-bedrock-adapter" => config.default_bedrock_adapter = value.to_string(),
        "enabled-bedrock-adapters" => {
            config.enabled_bedrock_adapters = parse_enabled_adapters(value);
        }
        "storage-profile" => config.storage_profile = value.to_string(),
        "auth-profile" => config.auth_profile = value.to_string(),
        "bedrock-auth-profile" => config.bedrock_auth_profile = value.to_string(),
        "default-gameplay-profile" => config.default_gameplay_profile = value.to_string(),
        "gameplay-profile-map" => {
            config.gameplay_profile_map = parse_gameplay_profile_map(value)?;
        }
        "plugins-dir" => config.plugins_dir = PathBuf::from(value),
        "plugin-allowlist" => config.plugin_allowlist = parse_enabled_adapters(value),
        "plugin-failure-policy" => {
            return Err(RuntimeError::Config(
                "plugin-failure-policy is no longer supported; use kind-specific plugin-failure-policy-* keys".to_string(),
            ));
        }
        "plugin-failure-policy-protocol" => {
            config.plugin_failure_policy_protocol = PluginFailureMatrix::parse_protocol(value)?;
        }
        "plugin-failure-policy-gameplay" => {
            config.plugin_failure_policy_gameplay = PluginFailureMatrix::parse_gameplay(value)?;
        }
        "plugin-failure-policy-storage" => {
            config.plugin_failure_policy_storage = PluginFailureMatrix::parse_storage(value)?;
        }
        "plugin-failure-policy-auth" => {
            config.plugin_failure_policy_auth = PluginFailureMatrix::parse_auth(value)?;
        }
        "plugin-reload-watch" => config.plugin_reload_watch = parse_bool_flag(value),
        "topology-reload-watch" => config.topology_reload_watch = parse_bool_flag(value),
        "topology-drain-grace-secs" => {
            config.topology_drain_grace_secs = parse_u64(value, "topology-drain-grace-secs")?;
        }
        "plugin-abi-min" => config.plugin_abi_min = PluginAbiRange::parse_version(value)?,
        "plugin-abi-max" => config.plugin_abi_max = PluginAbiRange::parse_version(value)?,
        unknown => eprintln!("warning: ignoring unknown server.properties key `{unknown}`"),
    }
    Ok(())
}

fn parse_server_ip(value: &str) -> Result<Option<IpAddr>, RuntimeError> {
    if value.is_empty() {
        Ok(None)
    } else {
        value
            .parse()
            .map(Some)
            .map_err(|_| RuntimeError::Config("invalid server-ip".to_string()))
    }
}

const fn parse_bool_flag(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 4
        && matches!(bytes[0], b't' | b'T')
        && matches!(bytes[1], b'r' | b'R')
        && matches!(bytes[2], b'u' | b'U')
        && matches!(bytes[3], b'e' | b'E')
}

fn parse_u8(value: &str, key: &str) -> Result<u8, RuntimeError> {
    value
        .parse()
        .map_err(|_| RuntimeError::Config(format!("invalid {key}")))
}

fn parse_u16(value: &str, key: &str) -> Result<u16, RuntimeError> {
    value
        .parse()
        .map_err(|_| RuntimeError::Config(format!("invalid {key}")))
}

fn parse_u64(value: &str, key: &str) -> Result<u64, RuntimeError> {
    value
        .parse()
        .map_err(|_| RuntimeError::Config(format!("invalid {key}")))
}

fn finalize_relative_paths(config: &mut ServerConfig, parent: &Path) {
    config.world_dir = parent.join(&config.level_name);
    if config.plugins_dir.is_relative() {
        config.plugins_dir = parent.join(&config.plugins_dir);
    }
}

fn parse_enabled_adapters(value: &str) -> Option<Vec<String>> {
    let adapters = value
        .split(',')
        .map(str::trim)
        .filter(|adapter_id| !adapter_id.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if adapters.is_empty() {
        return None;
    }
    Some(adapters)
}

fn parse_gameplay_profile_map(value: &str) -> Result<HashMap<String, String>, RuntimeError> {
    let mut map = HashMap::new();
    for entry in value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let Some((adapter_id, profile_id)) = entry.split_once(':') else {
            return Err(RuntimeError::Config(format!(
                "invalid gameplay-profile-map entry `{entry}`"
            )));
        };
        let adapter_id = adapter_id.trim();
        let profile_id = profile_id.trim();
        if adapter_id.is_empty() || profile_id.is_empty() {
            return Err(RuntimeError::Config(format!(
                "invalid gameplay-profile-map entry `{entry}`"
            )));
        }
        if map
            .insert(adapter_id.to_string(), profile_id.to_string())
            .is_some()
        {
            return Err(RuntimeError::Config(format!(
                "duplicate gameplay profile mapping for adapter `{adapter_id}`"
            )));
        }
    }
    Ok(map)
}
