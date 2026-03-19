use crate::RuntimeError;
use crate::host::{PluginAbiRange, PluginFailurePolicy};
use mc_plugin_api::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

pub(crate) const BEDROCK_BASELINE_ADAPTER_ID: &str = "be-26_3";
pub(crate) const BEDROCK_OFFLINE_AUTH_PROFILE_ID: &str = "bedrock-offline-v1";

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
    pub plugin_failure_policy: PluginFailurePolicy,
    pub plugin_reload_watch: bool,
    pub plugin_abi_min: PluginAbiVersion,
    pub plugin_abi_max: PluginAbiVersion,
    pub world_dir: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
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
            plugins_dir: cwd.join("runtime").join("plugins"),
            plugin_allowlist: None,
            plugin_failure_policy: PluginFailurePolicy::Quarantine,
            plugin_reload_watch: false,
            plugin_abi_min: CURRENT_PLUGIN_ABI,
            plugin_abi_max: CURRENT_PLUGIN_ABI,
            world_dir: cwd.join("runtime").join("world"),
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
                let line = raw_line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let value = value.trim();
                match key.trim() {
                    "server-ip" => {
                        if value.is_empty() {
                            config.server_ip = None;
                        } else {
                            config.server_ip = Some(value.parse().map_err(|_| {
                                RuntimeError::Config("invalid server-ip".to_string())
                            })?);
                        }
                    }
                    "server-port" => {
                        config.server_port = value
                            .parse()
                            .map_err(|_| RuntimeError::Config("invalid server-port".to_string()))?;
                    }
                    "be-enabled" => {
                        config.be_enabled = value.eq_ignore_ascii_case("true");
                    }
                    "motd" => config.motd = value.to_string(),
                    "max-players" => {
                        config.max_players = value
                            .parse()
                            .map_err(|_| RuntimeError::Config("invalid max-players".to_string()))?;
                    }
                    "online-mode" => {
                        config.online_mode = value.eq_ignore_ascii_case("true");
                    }
                    "level-name" => config.level_name = value.to_string(),
                    "level-type" => {
                        config.level_type = LevelType::parse(value)?;
                    }
                    "gamemode" => {
                        config.game_mode = value
                            .parse()
                            .map_err(|_| RuntimeError::Config("invalid gamemode".to_string()))?;
                    }
                    "difficulty" => {
                        config.difficulty = value
                            .parse()
                            .map_err(|_| RuntimeError::Config("invalid difficulty".to_string()))?;
                    }
                    "view-distance" => {
                        config.view_distance = value.parse().map_err(|_| {
                            RuntimeError::Config("invalid view-distance".to_string())
                        })?;
                    }
                    "default-adapter" => {
                        config.default_adapter = value.to_string();
                    }
                    "enabled-adapters" => {
                        config.enabled_adapters = parse_enabled_adapters(value)?;
                    }
                    "default-bedrock-adapter" => {
                        config.default_bedrock_adapter = value.to_string();
                    }
                    "enabled-bedrock-adapters" => {
                        config.enabled_bedrock_adapters = parse_enabled_adapters(value)?;
                    }
                    "storage-profile" => {
                        config.storage_profile = value.to_string();
                    }
                    "auth-profile" => {
                        config.auth_profile = value.to_string();
                    }
                    "bedrock-auth-profile" => {
                        config.bedrock_auth_profile = value.to_string();
                    }
                    "default-gameplay-profile" => {
                        config.default_gameplay_profile = value.to_string();
                    }
                    "gameplay-profile-map" => {
                        config.gameplay_profile_map = parse_gameplay_profile_map(value)?;
                    }
                    "plugins-dir" => {
                        config.plugins_dir = PathBuf::from(value);
                    }
                    "plugin-allowlist" => {
                        config.plugin_allowlist = parse_enabled_adapters(value)?;
                    }
                    "plugin-failure-policy" => {
                        config.plugin_failure_policy = PluginFailurePolicy::parse(value)?;
                    }
                    "plugin-reload-watch" => {
                        config.plugin_reload_watch = value.eq_ignore_ascii_case("true");
                    }
                    "plugin-abi-min" => {
                        config.plugin_abi_min = PluginAbiRange::parse_version(value)?;
                    }
                    "plugin-abi-max" => {
                        config.plugin_abi_max = PluginAbiRange::parse_version(value)?;
                    }
                    unknown => {
                        eprintln!("warning: ignoring unknown server.properties key `{unknown}`");
                    }
                }
            }
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        config.world_dir = parent.join(&config.level_name);
        if config.plugins_dir.is_relative() {
            config.plugins_dir = parent.join(&config.plugins_dir);
        }
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
        match &self.enabled_adapters {
            Some(enabled_adapters) => enabled_adapters.clone(),
            None => vec![self.default_adapter.clone()],
        }
    }

    pub(crate) fn effective_enabled_bedrock_adapters(&self) -> Vec<String> {
        match &self.enabled_bedrock_adapters {
            Some(enabled_adapters) => enabled_adapters.clone(),
            None => vec![self.default_bedrock_adapter.clone()],
        }
    }
}

fn parse_enabled_adapters(value: &str) -> Result<Option<Vec<String>>, RuntimeError> {
    let adapters = value
        .split(',')
        .map(str::trim)
        .filter(|adapter_id| !adapter_id.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if adapters.is_empty() {
        return Ok(None);
    }
    Ok(Some(adapters))
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
