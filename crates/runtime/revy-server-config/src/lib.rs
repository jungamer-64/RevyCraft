use mc_plugin_api::abi::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use mc_plugin_api::{
    AdapterId, AdminSurfaceProfileId, AuthProfileId, GameplayProfileId, StorageProfileId,
};
pub use revy_server_types::{AdminPermission, PluginFailureAction, PluginFailureMatrix};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::fs;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const BEDROCK_BASELINE_ADAPTER_ID: &str = "be-924";
pub const BEDROCK_OFFLINE_AUTH_PROFILE_ID: &str = "bedrock-offline-v1";
pub const DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS: u64 = 30;
#[derive(Debug, Error)]
pub enum ServerConfigError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported configuration: {0}")]
    Unsupported(String),
    #[error("configuration error: {0}")]
    Config(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerConfigSource {
    Inline(ServerConfig),
    Toml(PathBuf),
}

impl ServerConfigSource {
    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when the source cannot be materialized.
    pub fn load(&self) -> Result<ValidatedServerConfig, ServerConfigError> {
        match self {
            Self::Inline(config) => config.clone().validate_owned(),
            Self::Toml(path) => ServerConfig::from_toml(path),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LevelType {
    Flat,
}

impl LevelType {
    fn parse(value: &str) -> Result<Self, ServerConfigError> {
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
pub struct StaticConfig {
    pub bootstrap: BootstrapConfig,
}

impl StaticConfig {
    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when the candidate changes restart-only state.
    pub fn validate_reload_compatibility(&self, candidate: &Self) -> Result<(), ServerConfigError> {
        if candidate.bootstrap != self.bootstrap {
            return Err(ServerConfigError::Config(
                "bootstrap config changes require a restart".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveConfig {
    pub network: NetworkConfig,
    pub topology: TopologyConfig,
    pub plugins: PluginsConfig,
    pub profiles: ProfilesConfig,
    pub admin: AdminConfig,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedServerConfig(ServerConfig);

impl ValidatedServerConfig {
    #[must_use]
    pub const fn as_inner(&self) -> &ServerConfig {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> ServerConfig {
        self.0
    }
}

impl AsRef<ServerConfig> for ValidatedServerConfig {
    fn as_ref(&self) -> &ServerConfig {
        self.as_inner()
    }
}

impl Deref for ValidatedServerConfig {
    type Target = ServerConfig;

    fn deref(&self) -> &Self::Target {
        self.as_inner()
    }
}

impl From<ValidatedServerConfig> for ServerConfig {
    fn from(config: ValidatedServerConfig) -> Self {
        config.into_inner()
    }
}

impl TryFrom<ServerConfig> for ValidatedServerConfig {
    type Error = ServerConfigError;

    fn try_from(config: ServerConfig) -> Result<Self, Self::Error> {
        config.validate()?;
        Ok(Self(config))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopologyReloadPlan {
    pub next_active_config: ServerConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreReloadPlan {
    pub next_active_config: ServerConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullReloadPlan {
    pub next_active_config: ServerConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NormalizedServerConfig {
    server: ServerConfig,
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

impl From<ServerConfig> for NormalizedServerConfig {
    fn from(server: ServerConfig) -> Self {
        Self { server }
    }
}

impl NormalizedServerConfig {
    fn into_validated(self) -> Result<ValidatedServerConfig, ServerConfigError> {
        self.server.validate_owned()
    }

    fn static_config(&self) -> StaticConfig {
        StaticConfig {
            bootstrap: self.server.bootstrap.clone(),
        }
    }

    fn live_config(&self) -> LiveConfig {
        LiveConfig {
            network: self.server.network.clone(),
            topology: self.server.topology.clone(),
            plugins: self.server.plugins.clone(),
            profiles: self.server.profiles.clone(),
            admin: self.server.admin.clone(),
        }
    }
}

impl ServerConfig {
    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when `server.toml` cannot be read or parsed.
    pub fn from_toml(path: &Path) -> Result<ValidatedServerConfig, ServerConfigError> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        ServerConfigDocument::from_path(path)?
            .normalize(parent)?
            .into_validated()
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

    #[must_use]
    pub fn live_config(&self) -> LiveConfig {
        NormalizedServerConfig::from(self.clone()).live_config()
    }

    #[must_use]
    pub fn static_config(&self) -> StaticConfig {
        NormalizedServerConfig::from(self.clone()).static_config()
    }

    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when the candidate changes restart-only state.
    pub fn plan_topology_reload(
        &self,
        candidate: &Self,
    ) -> Result<TopologyReloadPlan, ServerConfigError> {
        self.static_config()
            .validate_reload_compatibility(&candidate.static_config())?;
        let mut next_active_config = self.clone();
        next_active_config.network.clone_from(&candidate.network);
        next_active_config.topology.clone_from(&candidate.topology);
        Ok(TopologyReloadPlan { next_active_config })
    }

    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when the candidate changes restart-only state.
    pub fn plan_core_reload(&self, candidate: &Self) -> Result<CoreReloadPlan, ServerConfigError> {
        validate_core_reload_static_compatibility(
            &self.static_config(),
            &candidate.static_config(),
        )?;
        let mut next_active_config = self.clone();
        next_active_config
            .bootstrap
            .level_name
            .clone_from(&candidate.bootstrap.level_name);
        next_active_config.bootstrap.game_mode = candidate.bootstrap.game_mode;
        next_active_config.bootstrap.difficulty = candidate.bootstrap.difficulty;
        next_active_config.bootstrap.view_distance = candidate.bootstrap.view_distance;
        next_active_config.network.max_players = candidate.network.max_players;
        Ok(CoreReloadPlan { next_active_config })
    }

    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when the candidate changes restart-only state.
    pub fn plan_full_reload(&self, candidate: &Self) -> Result<FullReloadPlan, ServerConfigError> {
        validate_core_reload_static_compatibility(
            &self.static_config(),
            &candidate.static_config(),
        )?;
        Ok(FullReloadPlan {
            next_active_config: candidate.clone(),
        })
    }

    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when validated fields are inconsistent.
    pub fn validate(&self) -> Result<(), ServerConfigError> {
        validate_plugin_abi_range(self.bootstrap.plugin_abi_min, self.bootstrap.plugin_abi_max)?;
        validate_enabled_adapters(
            self.topology.enabled_adapters.as_deref(),
            &self.topology.default_adapter,
            "enabled-adapters",
            "default-adapter",
        )?;
        validate_enabled_adapters(
            self.topology.enabled_bedrock_adapters.as_deref(),
            &self.topology.default_bedrock_adapter,
            "enabled-bedrock-adapters",
            "default-bedrock-adapter",
        )?;
        validate_admin_surfaces(&self.admin.surfaces)?;
        validate_admin_principals(&self.admin.principals)
    }

    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when config-only invariants are violated.
    pub fn validate_owned(self) -> Result<ValidatedServerConfig, ServerConfigError> {
        ValidatedServerConfig::try_from(self)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ServerConfigDocument {
    #[serde(rename = "static")]
    static_config: StaticDocument,
    live: LiveDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StaticDocument {
    bootstrap: StaticBootstrapDocument,
    plugins: StaticPluginsDocument,
    admin: StaticAdminDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StaticBootstrapDocument {
    online_mode: Option<bool>,
    level_name: Option<String>,
    level_type: Option<String>,
    game_mode: Option<u8>,
    difficulty: Option<u8>,
    view_distance: Option<u8>,
    world_dir: Option<PathBuf>,
    storage_profile: Option<StorageProfileId>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StaticPluginsDocument {
    plugins_dir: Option<PathBuf>,
    plugin_abi_min: Option<String>,
    plugin_abi_max: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StaticAdminDocument {
    principals: HashMap<String, AdminPrincipalDocument>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LiveDocument {
    network: NetworkDocument,
    topology: TopologyDocument,
    plugins: PluginsDocument,
    profiles: ProfilesDocument,
    admin: LiveAdminDocument,
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
    default_adapter: Option<AdapterId>,
    enabled_adapters: Option<Vec<AdapterId>>,
    default_bedrock_adapter: Option<AdapterId>,
    enabled_bedrock_adapters: Option<Vec<AdapterId>>,
    reload_watch: Option<bool>,
    drain_grace_secs: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PluginsDocument {
    allowlist: Option<Vec<String>>,
    reload_watch: Option<bool>,
    buffer_limits: PluginBufferLimitsDocument,
    failure_policy: FailurePolicyDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PluginBufferLimitsDocument {
    protocol_response_bytes: Option<usize>,
    gameplay_response_bytes: Option<usize>,
    storage_response_bytes: Option<usize>,
    auth_response_bytes: Option<usize>,
    admin_surface_response_bytes: Option<usize>,
    callback_payload_bytes: Option<usize>,
    metadata_bytes: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FailurePolicyDocument {
    protocol: Option<String>,
    gameplay: Option<String>,
    storage: Option<String>,
    auth: Option<String>,
    admin_surface: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ProfilesDocument {
    auth: Option<AuthProfileId>,
    bedrock_auth: Option<AuthProfileId>,
    default_gameplay: Option<GameplayProfileId>,
    gameplay_map: HashMap<AdapterId, GameplayProfileId>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LiveAdminDocument {
    surfaces: HashMap<String, AdminSurfaceDocument>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AdminPrincipalDocument {
    permissions: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AdminSurfaceDocument {
    profile: Option<AdminSurfaceProfileId>,
    config: Option<PathBuf>,
}

impl ServerConfigDocument {
    fn from_path(path: &Path) -> Result<Self, ServerConfigError> {
        let contents = fs::read_to_string(path).map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                ServerConfigError::Config(format!(
                    "server config path `{}` was not found",
                    path.display()
                ))
            } else {
                ServerConfigError::Io(error)
            }
        })?;
        toml::from_str(&contents).map_err(|error| {
            ServerConfigError::Config(format!(
                "failed to parse config {}: {error}",
                path.display()
            ))
        })
    }

    fn normalize(self, parent: &Path) -> Result<NormalizedServerConfig, ServerConfigError> {
        let level_name = self
            .static_config
            .bootstrap
            .level_name
            .clone()
            .unwrap_or_else(|| "world".to_string());
        Ok(NormalizedServerConfig::from(ServerConfig {
            bootstrap: BootstrapConfig {
                online_mode: self.static_config.bootstrap.online_mode.unwrap_or(false),
                level_name: level_name.clone(),
                level_type: LevelType::parse(
                    self.static_config
                        .bootstrap
                        .level_type
                        .as_deref()
                        .unwrap_or("flat"),
                )?,
                game_mode: self.static_config.bootstrap.game_mode.unwrap_or(0),
                difficulty: self.static_config.bootstrap.difficulty.unwrap_or(1),
                view_distance: self.static_config.bootstrap.view_distance.unwrap_or(2),
                world_dir: resolve_world_dir(
                    parent,
                    self.static_config.bootstrap.world_dir.as_deref(),
                    Some(level_name.as_str()),
                ),
                storage_profile: self
                    .static_config
                    .bootstrap
                    .storage_profile
                    .unwrap_or_else(|| StorageProfileId::new("je-anvil-1_7_10")),
                plugins_dir: resolve_config_path(
                    parent,
                    self.static_config.plugins.plugins_dir.as_deref(),
                    Path::new("plugins"),
                ),
                plugin_abi_min: parse_plugin_abi(
                    self.static_config.plugins.plugin_abi_min.as_deref(),
                    "static.plugins.plugin_abi_min",
                )?
                .unwrap_or(CURRENT_PLUGIN_ABI),
                plugin_abi_max: parse_plugin_abi(
                    self.static_config.plugins.plugin_abi_max.as_deref(),
                    "static.plugins.plugin_abi_max",
                )?
                .unwrap_or(CURRENT_PLUGIN_ABI),
            },
            network: NetworkConfig {
                server_ip: parse_server_ip(self.live.network.server_ip.as_deref())?,
                server_port: self.live.network.server_port.unwrap_or(25565),
                motd: self
                    .live
                    .network
                    .motd
                    .unwrap_or_else(|| "Multi-version Rust server".to_string()),
                max_players: self.live.network.max_players.unwrap_or(20),
            },
            topology: TopologyConfig {
                be_enabled: self.live.topology.be_enabled.unwrap_or(false),
                default_adapter: self
                    .live
                    .topology
                    .default_adapter
                    .unwrap_or_else(|| AdapterId::new("je-5")),
                enabled_adapters: normalize_optional_vec(self.live.topology.enabled_adapters),
                default_bedrock_adapter: self
                    .live
                    .topology
                    .default_bedrock_adapter
                    .unwrap_or_else(|| AdapterId::new(BEDROCK_BASELINE_ADAPTER_ID)),
                enabled_bedrock_adapters: normalize_optional_vec(
                    self.live.topology.enabled_bedrock_adapters,
                ),
                reload_watch: self.live.topology.reload_watch.unwrap_or(false),
                drain_grace_secs: self
                    .live
                    .topology
                    .drain_grace_secs
                    .unwrap_or(DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS),
            },
            plugins: PluginsConfig {
                allowlist: normalize_optional_vec(self.live.plugins.allowlist),
                reload_watch: self.live.plugins.reload_watch.unwrap_or(false),
                buffer_limits: parse_plugin_buffer_limits(self.live.plugins.buffer_limits),
                failure_policy: PluginFailureMatrix {
                    protocol: parse_failure_policy(
                        self.live.plugins.failure_policy.protocol.as_deref(),
                        parse_protocol_failure_policy,
                        PluginFailureMatrix::default().protocol,
                    )?,
                    gameplay: parse_failure_policy(
                        self.live.plugins.failure_policy.gameplay.as_deref(),
                        parse_gameplay_failure_policy,
                        PluginFailureMatrix::default().gameplay,
                    )?,
                    storage: parse_failure_policy(
                        self.live.plugins.failure_policy.storage.as_deref(),
                        parse_storage_failure_policy,
                        PluginFailureMatrix::default().storage,
                    )?,
                    auth: parse_failure_policy(
                        self.live.plugins.failure_policy.auth.as_deref(),
                        parse_auth_failure_policy,
                        PluginFailureMatrix::default().auth,
                    )?,
                    admin_surface: parse_failure_policy(
                        self.live.plugins.failure_policy.admin_surface.as_deref(),
                        parse_admin_surface_failure_policy,
                        PluginFailureMatrix::default().admin_surface,
                    )?,
                },
            },
            profiles: ProfilesConfig {
                auth: self
                    .live
                    .profiles
                    .auth
                    .unwrap_or_else(|| AuthProfileId::new("offline-v1")),
                bedrock_auth: self
                    .live
                    .profiles
                    .bedrock_auth
                    .unwrap_or_else(|| AuthProfileId::new(BEDROCK_OFFLINE_AUTH_PROFILE_ID)),
                default_gameplay: self
                    .live
                    .profiles
                    .default_gameplay
                    .unwrap_or_else(|| GameplayProfileId::new("canonical")),
                gameplay_map: self.live.profiles.gameplay_map,
            },
            admin: AdminConfig {
                surfaces: parse_admin_surface_config(parent, self.live.admin.surfaces)?,
                principals: parse_admin_principal_config(self.static_config.admin.principals)?,
            },
        }))
    }
}

fn parse_admin_surface_config(
    parent: &Path,
    document: HashMap<String, AdminSurfaceDocument>,
) -> Result<HashMap<String, AdminSurfaceConfig>, ServerConfigError> {
    let mut surfaces = HashMap::new();
    let mut entries = document.into_iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    for (instance_id, surface) in entries {
        let profile = surface.profile.ok_or_else(|| {
            ServerConfigError::Config(format!(
                "live.admin.surfaces.{instance_id}.profile is required"
            ))
        })?;
        let config = surface
            .config
            .map(|path| resolve_config_path(parent, Some(path.as_path()), Path::new("")));
        surfaces.insert(instance_id, AdminSurfaceConfig { profile, config });
    }
    Ok(surfaces)
}

fn parse_admin_principal_config(
    document: HashMap<String, AdminPrincipalDocument>,
) -> Result<HashMap<String, AdminPrincipalConfig>, ServerConfigError> {
    let mut principals = HashMap::new();
    let mut principal_entries = document.into_iter().collect::<Vec<_>>();
    principal_entries.sort_by(|left, right| left.0.cmp(&right.0));
    for (principal_id, principal) in principal_entries {
        let permissions = parse_admin_permissions(
            principal.permissions,
            &format!("static.admin.principals.{principal_id}.permissions"),
            None,
            true,
        )?;
        principals.insert(principal_id, AdminPrincipalConfig { permissions });
    }
    Ok(principals)
}

fn validate_core_reload_static_compatibility(
    active: &StaticConfig,
    candidate: &StaticConfig,
) -> Result<(), ServerConfigError> {
    let active_bootstrap = &active.bootstrap;
    let candidate_bootstrap = &candidate.bootstrap;
    let restart_required_bootstrap_diff = active_bootstrap.online_mode
        != candidate_bootstrap.online_mode
        || active_bootstrap.level_type != candidate_bootstrap.level_type
        || active_bootstrap.world_dir != candidate_bootstrap.world_dir
        || active_bootstrap.storage_profile != candidate_bootstrap.storage_profile
        || active_bootstrap.plugins_dir != candidate_bootstrap.plugins_dir
        || active_bootstrap.plugin_abi_min != candidate_bootstrap.plugin_abi_min
        || active_bootstrap.plugin_abi_max != candidate_bootstrap.plugin_abi_max;
    if restart_required_bootstrap_diff {
        return Err(ServerConfigError::Config(
            "bootstrap config changes require a restart".to_string(),
        ));
    }

    Ok(())
}

fn normalize_optional_vec<T>(values: Option<Vec<T>>) -> Option<Vec<T>> {
    match values {
        Some(values) if values.is_empty() => None,
        other => other,
    }
}

fn parse_plugin_buffer_limits(document: PluginBufferLimitsDocument) -> PluginBufferLimits {
    let defaults = PluginBufferLimits::default();
    PluginBufferLimits {
        protocol_response_bytes: document
            .protocol_response_bytes
            .unwrap_or(defaults.protocol_response_bytes),
        gameplay_response_bytes: document
            .gameplay_response_bytes
            .unwrap_or(defaults.gameplay_response_bytes),
        storage_response_bytes: document
            .storage_response_bytes
            .unwrap_or(defaults.storage_response_bytes),
        auth_response_bytes: document
            .auth_response_bytes
            .unwrap_or(defaults.auth_response_bytes),
        admin_surface_response_bytes: document
            .admin_surface_response_bytes
            .unwrap_or(defaults.admin_surface_response_bytes),
        callback_payload_bytes: document
            .callback_payload_bytes
            .unwrap_or(defaults.callback_payload_bytes),
        metadata_bytes: document.metadata_bytes.unwrap_or(defaults.metadata_bytes),
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
    if path.as_os_str().is_empty() {
        return parent.to_path_buf();
    }
    if path.is_relative() {
        parent.join(path)
    } else {
        path
    }
}

fn parse_server_ip(value: Option<&str>) -> Result<Option<IpAddr>, ServerConfigError> {
    match value {
        None | Some("") => Ok(None),
        Some(value) => value
            .parse()
            .map(Some)
            .map_err(|_| ServerConfigError::Config("invalid live.network.server_ip".to_string())),
    }
}

fn validate_plugin_abi_range(
    min: PluginAbiVersion,
    max: PluginAbiVersion,
) -> Result<(), ServerConfigError> {
    if min > max {
        return Err(ServerConfigError::Config(format!(
            "static.plugins.plugin_abi_min `{min}` must be <= static.plugins.plugin_abi_max `{max}`"
        )));
    }
    if CURRENT_PLUGIN_ABI < min || CURRENT_PLUGIN_ABI > max {
        return Err(ServerConfigError::Config(format!(
            "plugin ABI range `{min}..={max}` does not include current host ABI `{CURRENT_PLUGIN_ABI}`"
        )));
    }
    Ok(())
}

fn validate_enabled_adapters(
    values: Option<&[AdapterId]>,
    default_adapter: &AdapterId,
    values_key: &str,
    default_key: &str,
) -> Result<(), ServerConfigError> {
    let Some(values) = values else {
        return Ok(());
    };

    let mut seen = HashSet::new();
    for adapter_id in values {
        if !seen.insert(adapter_id.clone()) {
            return Err(ServerConfigError::Config(format!(
                "{values_key} contains duplicate adapter `{adapter_id}`"
            )));
        }
    }
    if !values
        .iter()
        .any(|adapter_id| adapter_id == default_adapter)
    {
        return Err(ServerConfigError::Config(format!(
            "{default_key} `{default_adapter}` must be included in {values_key}"
        )));
    }
    Ok(())
}

fn validate_admin_principals(
    principals: &HashMap<String, AdminPrincipalConfig>,
) -> Result<(), ServerConfigError> {
    let mut principal_entries = principals.iter().collect::<Vec<_>>();
    principal_entries.sort_by(|left, right| left.0.cmp(right.0));
    for (principal_id, principal) in principal_entries {
        if principal.permissions.is_empty() {
            return Err(ServerConfigError::Config(format!(
                "static.admin.principals.{principal_id}.permissions must not be empty"
            )));
        }
    }
    Ok(())
}

fn validate_admin_surfaces(
    surfaces: &HashMap<String, AdminSurfaceConfig>,
) -> Result<(), ServerConfigError> {
    let mut entries = surfaces.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    for (instance_id, surface) in entries {
        if surface.profile.as_str().trim().is_empty() {
            return Err(ServerConfigError::Config(format!(
                "live.admin.surfaces.{instance_id}.profile must not be empty"
            )));
        }
        if let Some(config) = &surface.config
            && !config.is_file()
        {
            return Err(ServerConfigError::Config(format!(
                "live.admin.surfaces.{instance_id}.config `{}` was not found",
                config.display()
            )));
        }
    }
    Ok(())
}

fn parse_failure_policy<F, E>(
    value: Option<&str>,
    parser: F,
    default: PluginFailureAction,
) -> Result<PluginFailureAction, ServerConfigError>
where
    F: Fn(&str) -> Result<PluginFailureAction, E>,
    E: Into<ServerConfigError>,
{
    match value {
        Some(value) => parser(value).map_err(Into::into),
        None => Ok(default),
    }
}

fn parse_failure_action_with_allowed(
    value: &str,
    key: &str,
    allowed: &[PluginFailureAction],
) -> Result<PluginFailureAction, ServerConfigError> {
    let action = if value.eq_ignore_ascii_case("quarantine") {
        PluginFailureAction::Quarantine
    } else if value.eq_ignore_ascii_case("skip") {
        PluginFailureAction::Skip
    } else if value.eq_ignore_ascii_case("fail-fast") {
        PluginFailureAction::FailFast
    } else {
        return Err(ServerConfigError::Config(format!(
            "unsupported {key} `{value}`"
        )));
    };
    if allowed.contains(&action) {
        Ok(action)
    } else {
        Err(ServerConfigError::Config(format!(
            "unsupported {key} `{value}`"
        )))
    }
}

fn parse_protocol_failure_policy(value: &str) -> Result<PluginFailureAction, ServerConfigError> {
    parse_failure_action_with_allowed(
        value,
        "plugin-failure-policy-protocol",
        &[
            PluginFailureAction::Quarantine,
            PluginFailureAction::Skip,
            PluginFailureAction::FailFast,
        ],
    )
}

fn parse_gameplay_failure_policy(value: &str) -> Result<PluginFailureAction, ServerConfigError> {
    parse_failure_action_with_allowed(
        value,
        "plugin-failure-policy-gameplay",
        &[
            PluginFailureAction::Quarantine,
            PluginFailureAction::Skip,
            PluginFailureAction::FailFast,
        ],
    )
}

fn parse_storage_failure_policy(value: &str) -> Result<PluginFailureAction, ServerConfigError> {
    parse_failure_action_with_allowed(
        value,
        "plugin-failure-policy-storage",
        &[PluginFailureAction::Skip, PluginFailureAction::FailFast],
    )
}

fn parse_auth_failure_policy(value: &str) -> Result<PluginFailureAction, ServerConfigError> {
    parse_failure_action_with_allowed(
        value,
        "plugin-failure-policy-auth",
        &[PluginFailureAction::Skip, PluginFailureAction::FailFast],
    )
}

fn parse_admin_surface_failure_policy(
    value: &str,
) -> Result<PluginFailureAction, ServerConfigError> {
    parse_failure_action_with_allowed(
        value,
        "plugin-failure-policy-admin-surface",
        &[
            PluginFailureAction::Quarantine,
            PluginFailureAction::Skip,
            PluginFailureAction::FailFast,
        ],
    )
}

fn parse_admin_permissions(
    values: Option<Vec<String>>,
    key: &str,
    default: Option<Vec<AdminPermission>>,
    require_nonempty: bool,
) -> Result<Vec<AdminPermission>, ServerConfigError> {
    let values = match values {
        Some(values) => values,
        None => {
            let permissions = default.unwrap_or_default();
            if require_nonempty && permissions.is_empty() {
                return Err(ServerConfigError::Config(format!(
                    "{key} must not be empty"
                )));
            }
            return Ok(permissions);
        }
    };
    let mut permissions = Vec::new();
    for value in values {
        let permission = match value.as_str() {
            "status" => AdminPermission::Status,
            "sessions" => AdminPermission::Sessions,
            "reload-runtime" => AdminPermission::ReloadRuntime,
            "upgrade-runtime" => AdminPermission::UpgradeRuntime,
            "shutdown" => AdminPermission::Shutdown,
            _ => {
                return Err(ServerConfigError::Config(format!(
                    "unsupported {key} entry `{value}`"
                )));
            }
        };
        if !permissions.contains(&permission) {
            permissions.push(permission);
        }
    }
    if require_nonempty && permissions.is_empty() {
        return Err(ServerConfigError::Config(format!(
            "{key} must not be empty"
        )));
    }
    Ok(permissions)
}

fn parse_plugin_abi(
    value: Option<&str>,
    key: &str,
) -> Result<Option<PluginAbiVersion>, ServerConfigError> {
    value
        .map(|value| parse_plugin_abi_version(value, key))
        .transpose()
}

fn parse_plugin_abi_version(value: &str, key: &str) -> Result<PluginAbiVersion, ServerConfigError> {
    let Some((major, minor)) = value.split_once('.') else {
        return Err(ServerConfigError::Config(format!(
            "invalid {key} `{value}`"
        )));
    };
    let major = major
        .parse()
        .map_err(|_| ServerConfigError::Config(format!("invalid {key} `{value}`")))?;
    let minor = minor
        .parse()
        .map_err(|_| ServerConfigError::Config(format!("invalid {key} `{value}`")))?;
    Ok(PluginAbiVersion { major, minor })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn configured_server_config() -> ServerConfig {
        let mut config = ServerConfig::default();
        config.bootstrap.level_name = "active-world".to_string();
        config.bootstrap.world_dir = PathBuf::from("runtime").join("active-world");
        config.bootstrap.plugins_dir = PathBuf::from("runtime").join("active-plugins");
        config.network.server_port = 25570;
        config.network.motd = "active-motd".to_string();
        config.network.max_players = 16;
        config.topology.default_adapter = AdapterId::new("je-47");
        config.topology.enabled_adapters = Some(vec![AdapterId::new("je-47")]);
        config.plugins.allowlist = Some(vec!["proto-initial".to_string()]);
        config.profiles.default_gameplay = GameplayProfileId::new("canonical");
        config.admin.surfaces.insert(
            "console".to_string(),
            AdminSurfaceConfig {
                profile: AdminSurfaceProfileId::new("console-v1"),
                config: None,
            },
        );
        config.admin.surfaces.insert(
            "remote".to_string(),
            AdminSurfaceConfig {
                profile: AdminSurfaceProfileId::new("grpc-v1"),
                config: Some(PathBuf::from("runtime").join("admin-grpc.toml")),
            },
        );
        config.admin.principals.insert(
            "ops".to_string(),
            AdminPrincipalConfig {
                permissions: vec![AdminPermission::Status],
            },
        );
        config
    }

    fn assert_config_error_contains(error: ServerConfigError, expected_fragment: &str) {
        match error {
            ServerConfigError::Config(message) => {
                assert!(
                    message.contains(expected_fragment),
                    "unexpected config error: {message}"
                );
            }
            other => panic!("unexpected config error: {other:?}"),
        }
    }

    #[test]
    fn topology_reload_plan_updates_only_network_and_topology() -> Result<(), ServerConfigError> {
        let active = configured_server_config();
        let mut candidate = active.clone();
        candidate.network.server_port = 25571;
        candidate.network.motd = "candidate-motd".to_string();
        candidate.network.max_players = 24;
        candidate.topology.be_enabled = true;
        candidate.topology.default_adapter = AdapterId::new("je-5");
        candidate.topology.enabled_adapters = Some(vec![AdapterId::new("je-5")]);
        candidate.plugins.allowlist = Some(vec!["proto-candidate".to_string()]);
        candidate.profiles.default_gameplay = GameplayProfileId::new("readonly");
        candidate.admin.surfaces.insert(
            "console".to_string(),
            AdminSurfaceConfig {
                profile: AdminSurfaceProfileId::new("console-v2"),
                config: None,
            },
        );

        let plan = active.plan_topology_reload(&candidate)?;

        assert_eq!(plan.next_active_config.network, candidate.network);
        assert_eq!(plan.next_active_config.topology, candidate.topology);
        assert_eq!(plan.next_active_config.bootstrap, active.bootstrap);
        assert_eq!(plan.next_active_config.plugins, active.plugins);
        assert_eq!(plan.next_active_config.profiles, active.profiles);
        assert_eq!(plan.next_active_config.admin, active.admin);
        Ok(())
    }

    #[test]
    fn topology_reload_plan_rejects_bootstrap_diff() {
        let active = configured_server_config();
        let mut candidate = active.clone();
        candidate.bootstrap.world_dir = PathBuf::from("runtime").join("other-world");

        let error = active
            .plan_topology_reload(&candidate)
            .expect_err("topology reload should reject bootstrap diffs");
        assert_config_error_contains(error, "bootstrap config changes require a restart");
    }

    #[test]
    fn topology_reload_plan_ignores_admin_surface_diff() -> Result<(), ServerConfigError> {
        let active = configured_server_config();
        let mut candidate = active.clone();
        candidate.admin.surfaces.insert(
            "remote".to_string(),
            AdminSurfaceConfig {
                profile: AdminSurfaceProfileId::new("grpc-v1"),
                config: Some(PathBuf::from("runtime").join("other-grpc-admin-surface.toml")),
            },
        );
        candidate.admin.principals.insert(
            "backup".to_string(),
            AdminPrincipalConfig {
                permissions: vec![AdminPermission::Sessions],
            },
        );

        let plan = active.plan_topology_reload(&candidate)?;
        assert_eq!(plan.next_active_config.admin, active.admin);
        Ok(())
    }

    #[test]
    fn core_reload_plan_updates_only_core_reloadable_fields() -> Result<(), ServerConfigError> {
        let active = configured_server_config();
        let mut candidate = active.clone();
        candidate.bootstrap.level_name = "candidate-world".to_string();
        candidate.bootstrap.game_mode = 1;
        candidate.bootstrap.difficulty = 3;
        candidate.bootstrap.view_distance = 5;
        candidate.network.max_players = 31;
        candidate.network.motd = "candidate-motd".to_string();
        candidate.plugins.allowlist = Some(vec!["proto-candidate".to_string()]);
        candidate.profiles.default_gameplay = GameplayProfileId::new("readonly");
        candidate.admin.surfaces.insert(
            "console".to_string(),
            AdminSurfaceConfig {
                profile: AdminSurfaceProfileId::new("console-v2"),
                config: None,
            },
        );

        let plan = active.plan_core_reload(&candidate)?;

        assert_eq!(
            plan.next_active_config.bootstrap.level_name,
            candidate.bootstrap.level_name
        );
        assert_eq!(
            plan.next_active_config.bootstrap.game_mode,
            candidate.bootstrap.game_mode
        );
        assert_eq!(
            plan.next_active_config.bootstrap.difficulty,
            candidate.bootstrap.difficulty
        );
        assert_eq!(
            plan.next_active_config.bootstrap.view_distance,
            candidate.bootstrap.view_distance
        );
        assert_eq!(
            plan.next_active_config.network.max_players,
            candidate.network.max_players
        );
        assert_eq!(plan.next_active_config.network.motd, active.network.motd);
        assert_eq!(plan.next_active_config.topology, active.topology);
        assert_eq!(plan.next_active_config.plugins, active.plugins);
        assert_eq!(plan.next_active_config.profiles, active.profiles);
        assert_eq!(plan.next_active_config.admin, active.admin);
        Ok(())
    }

    #[test]
    fn core_reload_plan_rejects_restart_required_bootstrap_diff() {
        let active = configured_server_config();
        let mut candidate = active.clone();
        candidate.bootstrap.plugins_dir = PathBuf::from("runtime").join("other-plugins");

        let error = active
            .plan_core_reload(&candidate)
            .expect_err("core reload should reject plugins_dir diffs");
        assert_config_error_contains(error, "bootstrap config changes require a restart");
    }

    #[test]
    fn core_reload_plan_ignores_admin_surface_diff() -> Result<(), ServerConfigError> {
        let active = configured_server_config();
        let mut candidate = active.clone();
        candidate.admin.surfaces.insert(
            "remote".to_string(),
            AdminSurfaceConfig {
                profile: AdminSurfaceProfileId::new("grpc-v2"),
                config: Some(PathBuf::from("runtime").join("admin-grpc.toml")),
            },
        );
        candidate.admin.principals.insert(
            "backup".to_string(),
            AdminPrincipalConfig {
                permissions: vec![AdminPermission::Sessions],
            },
        );

        let plan = active.plan_core_reload(&candidate)?;
        assert_eq!(plan.next_active_config.admin, active.admin);
        Ok(())
    }

    #[test]
    fn full_reload_plan_adopts_candidate_config() -> Result<(), ServerConfigError> {
        let active = configured_server_config();
        let mut candidate = active.clone();
        candidate.bootstrap.level_name = "candidate-world".to_string();
        candidate.bootstrap.game_mode = 1;
        candidate.bootstrap.difficulty = 3;
        candidate.bootstrap.view_distance = 5;
        candidate.network.max_players = 31;
        candidate.network.motd = "candidate-motd".to_string();
        candidate.plugins.allowlist = Some(vec!["proto-candidate".to_string()]);
        candidate.profiles.default_gameplay = GameplayProfileId::new("readonly");
        candidate.admin.surfaces.insert(
            "console".to_string(),
            AdminSurfaceConfig {
                profile: AdminSurfaceProfileId::new("console-v2"),
                config: None,
            },
        );

        let plan = active.plan_full_reload(&candidate)?;

        assert_eq!(plan.next_active_config, candidate);
        Ok(())
    }

    #[test]
    fn validate_rejects_plugin_abi_range_when_min_exceeds_max() {
        let mut config = configured_server_config();
        config.bootstrap.plugin_abi_min = PluginAbiVersion { major: 5, minor: 1 };
        config.bootstrap.plugin_abi_max = PluginAbiVersion { major: 5, minor: 0 };

        let error = config
            .validate_owned()
            .expect_err("plugin ABI range should reject min > max");
        assert_config_error_contains(error, "plugin_abi_min");
    }

    #[test]
    fn validate_rejects_plugin_abi_range_when_current_host_abi_is_excluded() {
        let mut config = configured_server_config();
        config.bootstrap.plugin_abi_min = PluginAbiVersion { major: 4, minor: 0 };
        config.bootstrap.plugin_abi_max = PluginAbiVersion { major: 4, minor: 9 };

        let error = config
            .validate_owned()
            .expect_err("plugin ABI range should include current host ABI");
        assert_config_error_contains(error, "does not include current host ABI");
    }

    #[test]
    fn validate_rejects_duplicate_enabled_adapters() {
        let mut config = configured_server_config();
        config.topology.enabled_adapters =
            Some(vec![AdapterId::new("je-47"), AdapterId::new("je-47")]);

        let error = config
            .validate_owned()
            .expect_err("enabled adapters should reject duplicates");
        assert_config_error_contains(error, "duplicate adapter");
    }

    #[test]
    fn validate_rejects_enabled_adapters_without_default() {
        let mut config = configured_server_config();
        config.topology.enabled_adapters = Some(vec![AdapterId::new("je-5")]);

        let error = config
            .validate_owned()
            .expect_err("enabled adapters should include the default adapter");
        assert_config_error_contains(error, "default-adapter");
    }

    #[test]
    fn full_reload_plan_rejects_restart_required_static_diff() {
        let active = configured_server_config();
        let mut candidate = active.clone();
        candidate.bootstrap.storage_profile = StorageProfileId::new("other-storage");

        let error = active
            .plan_full_reload(&candidate)
            .expect_err("full reload should reject storage profile diffs");
        assert_config_error_contains(error, "bootstrap config changes require a restart");
    }

    #[test]
    fn from_toml_rejects_missing_path_with_selected_path() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "revy-missing-server-config-{}-{nonce}.toml",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let error =
            ServerConfig::from_toml(&path).expect_err("missing server.toml should fail fast");
        let expected_path = path.display().to_string();

        match error {
            ServerConfigError::Config(message) => {
                assert!(message.contains("server config path"));
                assert!(message.contains(expected_path.as_str()));
            }
            other => panic!("unexpected config error: {other:?}"),
        }
    }

    #[test]
    fn from_toml_parses_admin_surfaces_and_disables_legacy_surface_slots()
    -> Result<(), ServerConfigError> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "revy-admin-surfaces-config-{}-{nonce}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("admin"))?;
        let surface_config = root.join("admin").join("grpc.toml");
        std::fs::write(&surface_config, "bind_addr = \"127.0.0.1:50051\"\n")?;
        let server_path = root.join("server.toml");
        std::fs::write(
            &server_path,
            r#"
[live.admin.surfaces.console]
profile = "console-v1"

[live.admin.surfaces.remote]
profile = "grpc-v1"
config = "admin/grpc.toml"
"#,
        )?;

        let parsed = ServerConfig::from_toml(&server_path)?;
        assert_eq!(
            parsed.admin.surfaces.get("console"),
            Some(&AdminSurfaceConfig {
                profile: AdminSurfaceProfileId::new("console-v1"),
                config: None,
            })
        );
        assert_eq!(
            parsed.admin.surfaces.get("remote"),
            Some(&AdminSurfaceConfig {
                profile: AdminSurfaceProfileId::new("grpc-v1"),
                config: Some(surface_config.clone()),
            })
        );

        assert_eq!(parsed.admin.surfaces.len(), 2);
        Ok(())
    }

    #[test]
    fn from_toml_rejects_legacy_admin_keys() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "revy-admin-surface-legacy-{}-{nonce}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root should be created");
        let server_path = root.join("server.toml");
        std::fs::write(
            &server_path,
            r#"
[static.admin.remote]
transport_profile = "grpc-v1"

[live.admin]
ui_profile = "console-v1"
local_console_permissions = ["status"]
"#,
        )
        .expect("server config should be written");

        let error =
            ServerConfig::from_toml(&server_path).expect_err("legacy admin keys should fail");
        assert_config_error_contains(error, "unknown field");
    }

    #[test]
    fn from_toml_rejects_missing_admin_surface_config_file() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "revy-admin-surface-config-missing-{}-{nonce}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root should be created");
        let server_path = root.join("server.toml");
        std::fs::write(
            &server_path,
            r#"
[live.admin.surfaces.remote]
profile = "grpc-v1"
config = "missing-grpc.toml"
"#,
        )
        .expect("server config should be written");

        let error = ServerConfig::from_toml(&server_path)
            .expect_err("missing admin surface config should be rejected");
        assert_config_error_contains(error, "live.admin.surfaces.remote.config");
    }
}
