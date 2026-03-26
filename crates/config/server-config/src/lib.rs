use mc_core::{AdapterId, AdminUiProfileId, AuthProfileId, GameplayProfileId, StorageProfileId};
use mc_plugin_api::abi::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use mc_plugin_host::config::{
    BootstrapConfig as PluginHostBootstrapConfig, PluginBufferLimits as PluginHostBufferLimits,
    RuntimeSelectionConfig as PluginHostRuntimeSelectionConfig,
};
use mc_plugin_host::host::{PluginAbiRange, PluginFailureAction, PluginFailureMatrix};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const BEDROCK_BASELINE_ADAPTER_ID: &str = "be-924";
pub const BEDROCK_OFFLINE_AUTH_PROFILE_ID: &str = "bedrock-offline-v1";
pub const DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS: u64 = 30;
pub const DEFAULT_ADMIN_GRPC_BIND_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 50_051);

#[derive(Debug, Error)]
pub enum ServerConfigError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("plugin-host config error: {0}")]
    PluginHost(#[from] mc_plugin_host::PluginHostError),
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
            Self::Inline(config) => {
                let config = config.clone();
                config.validate()?;
                Ok(config)
            }
            Self::Toml(path) => ServerConfig::from_toml(path),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdminPermission {
    Status,
    Sessions,
    ReloadRuntime,
    Shutdown,
}

impl AdminPermission {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Sessions => "sessions",
            Self::ReloadRuntime => "reload-runtime",
            Self::Shutdown => "shutdown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginsConfig {
    pub allowlist: Option<Vec<String>>,
    pub reload_watch: bool,
    pub buffer_limits: PluginHostBufferLimits,
    pub failure_policy: PluginFailureMatrix,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            allowlist: None,
            reload_watch: false,
            buffer_limits: PluginHostBufferLimits::default(),
            failure_policy: PluginFailureMatrix::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminConfig {
    pub ui_profile: AdminUiProfileId,
    pub local_console_permissions: Vec<AdminPermission>,
    pub grpc: AdminGrpcConfig,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            ui_profile: AdminUiProfileId::new("console-v1"),
            local_console_permissions: all_admin_permissions().to_vec(),
            grpc: AdminGrpcConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminGrpcConfig {
    pub enabled: bool,
    pub bind_addr: SocketAddr,
    pub allow_non_loopback: bool,
    pub principals: HashMap<String, AdminGrpcPrincipalConfig>,
}

impl Default for AdminGrpcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_addr: DEFAULT_ADMIN_GRPC_BIND_ADDR,
            allow_non_loopback: false,
            principals: HashMap::new(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AdminGrpcPrincipalConfig {
    pub token_file: PathBuf,
    pub token: String,
    pub permissions: Vec<AdminPermission>,
}

impl Debug for AdminGrpcPrincipalConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminGrpcPrincipalConfig")
            .field("token_file", &self.token_file)
            .field("token", &"***redacted***")
            .field("permissions", &self.permissions)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminGrpcTransportConfig {
    pub enabled: bool,
    pub bind_addr: SocketAddr,
    pub allow_non_loopback: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticConfig {
    pub bootstrap: BootstrapConfig,
    pub admin_grpc: AdminGrpcTransportConfig,
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
        if candidate.admin_grpc != self.admin_grpc {
            return Err(ServerConfigError::Config(
                "admin.grpc transport changes require a restart".to_string(),
            ));
        }
        Ok(())
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

pub type ValidatedServerConfig = ServerConfig;

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
    fn into_validated(self) -> Result<ServerConfig, ServerConfigError> {
        self.server.validate()?;
        Ok(self.server)
    }

    fn static_config(&self) -> StaticConfig {
        StaticConfig {
            bootstrap: self.server.bootstrap.clone(),
            admin_grpc: AdminGrpcTransportConfig {
                enabled: self.server.admin.grpc.enabled,
                bind_addr: self.server.admin.grpc.bind_addr,
                allow_non_loopback: self.server.admin.grpc.allow_non_loopback,
            },
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

    fn plugin_host_bootstrap_config(&self) -> PluginHostBootstrapConfig {
        PluginHostBootstrapConfig {
            storage_profile: self.server.bootstrap.storage_profile.clone(),
            plugins_dir: self.server.bootstrap.plugins_dir.clone(),
            plugin_abi_min: self.server.bootstrap.plugin_abi_min,
            plugin_abi_max: self.server.bootstrap.plugin_abi_max,
        }
    }

    fn plugin_host_runtime_selection_config(&self) -> PluginHostRuntimeSelectionConfig {
        PluginHostRuntimeSelectionConfig {
            be_enabled: self.server.topology.be_enabled,
            auth_profile: self.server.profiles.auth.clone(),
            bedrock_auth_profile: self.server.profiles.bedrock_auth.clone(),
            default_gameplay_profile: self.server.profiles.default_gameplay.clone(),
            gameplay_profile_map: self.server.profiles.gameplay_map.clone(),
            admin_ui_profile: self.server.admin.ui_profile.clone(),
            plugin_allowlist: self.server.plugins.allowlist.clone(),
            buffer_limits: self.server.plugins.buffer_limits,
            plugin_failure_policy_protocol: self.server.plugins.failure_policy.protocol,
            plugin_failure_policy_gameplay: self.server.plugins.failure_policy.gameplay,
            plugin_failure_policy_storage: self.server.plugins.failure_policy.storage,
            plugin_failure_policy_auth: self.server.plugins.failure_policy.auth,
            plugin_failure_policy_admin_ui: self.server.plugins.failure_policy.admin_ui,
        }
    }
}

impl ServerConfig {
    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when `server.toml` cannot be read or parsed.
    pub fn from_toml(path: &Path) -> Result<Self, ServerConfigError> {
        if !path.exists() {
            return NormalizedServerConfig::from(Self::default()).into_validated();
        }
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
    pub fn admin_grpc_enabled(&self) -> bool {
        self.admin.grpc.enabled
    }

    #[must_use]
    pub fn admin_grpc_bind_addr(&self) -> SocketAddr {
        self.admin.grpc.bind_addr
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

    #[must_use]
    pub fn plugin_host_bootstrap_config(&self) -> PluginHostBootstrapConfig {
        NormalizedServerConfig::from(self.clone()).plugin_host_bootstrap_config()
    }

    #[must_use]
    pub fn plugin_host_runtime_selection_config(&self) -> PluginHostRuntimeSelectionConfig {
        NormalizedServerConfig::from(self.clone()).plugin_host_runtime_selection_config()
    }

    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when validated fields are inconsistent.
    pub fn validate(&self) -> Result<(), ServerConfigError> {
        validate_admin_grpc_config(&self.admin.grpc)
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
    grpc: AdminGrpcDocument,
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
    admin_ui_response_bytes: Option<usize>,
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
    admin_ui: Option<String>,
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
    ui_profile: Option<AdminUiProfileId>,
    local_console_permissions: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AdminGrpcDocument {
    enabled: Option<bool>,
    bind_addr: Option<String>,
    allow_non_loopback: Option<bool>,
    principals: HashMap<String, AdminGrpcPrincipalDocument>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AdminGrpcPrincipalDocument {
    token_file: Option<PathBuf>,
    permissions: Option<Vec<String>>,
}

impl ServerConfigDocument {
    fn from_path(path: &Path) -> Result<Self, ServerConfigError> {
        toml::from_str(&fs::read_to_string(path)?).map_err(|error| {
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
                        PluginFailureMatrix::parse_protocol,
                        PluginFailureMatrix::default().protocol,
                    )?,
                    gameplay: parse_failure_policy(
                        self.live.plugins.failure_policy.gameplay.as_deref(),
                        PluginFailureMatrix::parse_gameplay,
                        PluginFailureMatrix::default().gameplay,
                    )?,
                    storage: parse_failure_policy(
                        self.live.plugins.failure_policy.storage.as_deref(),
                        PluginFailureMatrix::parse_storage,
                        PluginFailureMatrix::default().storage,
                    )?,
                    auth: parse_failure_policy(
                        self.live.plugins.failure_policy.auth.as_deref(),
                        PluginFailureMatrix::parse_auth,
                        PluginFailureMatrix::default().auth,
                    )?,
                    admin_ui: parse_failure_policy(
                        self.live.plugins.failure_policy.admin_ui.as_deref(),
                        PluginFailureMatrix::parse_admin_ui,
                        PluginFailureMatrix::default().admin_ui,
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
                ui_profile: self
                    .live
                    .admin
                    .ui_profile
                    .unwrap_or_else(|| AdminUiProfileId::new("console-v1")),
                local_console_permissions: parse_admin_permissions(
                    self.live.admin.local_console_permissions,
                    "live.admin.local_console_permissions",
                    Some(all_admin_permissions().to_vec()),
                    false,
                )?,
                grpc: parse_admin_grpc_config(parent, self.static_config.admin.grpc)?,
            },
        }))
    }
}

fn parse_admin_grpc_config(
    parent: &Path,
    document: AdminGrpcDocument,
) -> Result<AdminGrpcConfig, ServerConfigError> {
    let mut principals = HashMap::new();
    let mut seen_tokens = HashMap::new();
    let mut principal_entries = document.principals.into_iter().collect::<Vec<_>>();
    principal_entries.sort_by(|left, right| left.0.cmp(&right.0));
    for (principal_id, principal) in principal_entries {
        let token_file = principal.token_file.ok_or_else(|| {
            ServerConfigError::Config(format!(
                "static.admin.grpc.principals.{principal_id}.token_file is required"
            ))
        })?;
        let token_file = resolve_config_path(parent, Some(token_file.as_path()), Path::new(""));
        let token = fs::read_to_string(&token_file)?.trim().to_string();
        if token.is_empty() {
            return Err(ServerConfigError::Config(format!(
                "static.admin.grpc.principals.{principal_id}.token_file resolved to an empty token"
            )));
        }
        if let Some(previous_principal) = seen_tokens.insert(token.clone(), principal_id.clone()) {
            return Err(ServerConfigError::Config(format!(
                "admin.grpc principals `{previous_principal}` and `{principal_id}` resolved to the same token"
            )));
        }
        let permissions = parse_admin_permissions(
            principal.permissions,
            &format!("static.admin.grpc.principals.{principal_id}.permissions"),
            None,
            true,
        )?;
        principals.insert(
            principal_id,
            AdminGrpcPrincipalConfig {
                token_file,
                token,
                permissions,
            },
        );
    }
    let enabled = document.enabled.unwrap_or(false);
    if enabled && principals.is_empty() {
        return Err(ServerConfigError::Config(
            "static.admin.grpc.enabled=true requires at least one static.admin.grpc.principals entry"
                .to_string(),
        ));
    }
    let config = AdminGrpcConfig {
        enabled,
        bind_addr: parse_socket_addr(
            document.bind_addr.as_deref(),
            "static.admin.grpc.bind_addr",
            DEFAULT_ADMIN_GRPC_BIND_ADDR,
        )?,
        allow_non_loopback: document.allow_non_loopback.unwrap_or(false),
        principals,
    };
    validate_admin_grpc_config(&config)?;
    Ok(config)
}

fn normalize_optional_vec<T>(values: Option<Vec<T>>) -> Option<Vec<T>> {
    match values {
        Some(values) if values.is_empty() => None,
        other => other,
    }
}

fn parse_plugin_buffer_limits(document: PluginBufferLimitsDocument) -> PluginHostBufferLimits {
    let defaults = PluginHostBufferLimits::default();
    PluginHostBufferLimits {
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
        admin_ui_response_bytes: document
            .admin_ui_response_bytes
            .unwrap_or(defaults.admin_ui_response_bytes),
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

fn parse_socket_addr(
    value: Option<&str>,
    key: &str,
    default: SocketAddr,
) -> Result<SocketAddr, ServerConfigError> {
    match value {
        Some(value) => value
            .parse()
            .map_err(|_| ServerConfigError::Config(format!("invalid {key} `{value}`"))),
        None => Ok(default),
    }
}

fn validate_admin_grpc_config(config: &AdminGrpcConfig) -> Result<(), ServerConfigError> {
    if config.enabled && config.principals.is_empty() {
        return Err(ServerConfigError::Config(
            "static.admin.grpc.enabled=true requires at least one static.admin.grpc.principals entry"
                .to_string(),
        ));
    }
    let mut seen_tokens = HashMap::new();
    let mut principal_entries = config.principals.iter().collect::<Vec<_>>();
    principal_entries.sort_by(|left, right| left.0.cmp(right.0));
    for (principal_id, principal) in principal_entries {
        if principal.token.trim().is_empty() {
            return Err(ServerConfigError::Config(format!(
                "static.admin.grpc.principals.{principal_id}.token_file resolved to an empty token"
            )));
        }
        if principal.permissions.is_empty() {
            return Err(ServerConfigError::Config(format!(
                "static.admin.grpc.principals.{principal_id}.permissions must not be empty"
            )));
        }
        let normalized_token = principal.token.trim().to_string();
        if let Some(previous_principal) = seen_tokens.insert(normalized_token, principal_id.clone())
        {
            return Err(ServerConfigError::Config(format!(
                "admin.grpc principals `{previous_principal}` and `{principal_id}` resolved to the same token"
            )));
        }
    }
    validate_admin_grpc_transport(config)
}

fn validate_admin_grpc_transport(config: &AdminGrpcConfig) -> Result<(), ServerConfigError> {
    if config.enabled && !config.allow_non_loopback && !config.bind_addr.ip().is_loopback() {
        return Err(ServerConfigError::Config(format!(
            "static.admin.grpc.bind_addr `{}` is non-loopback; set static.admin.grpc.allow_non_loopback=true to expose the built-in plaintext gRPC server",
            config.bind_addr
        )));
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

const fn all_admin_permissions() -> [AdminPermission; 4] {
    [
        AdminPermission::Status,
        AdminPermission::Sessions,
        AdminPermission::ReloadRuntime,
        AdminPermission::Shutdown,
    ]
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
        .map(|value| {
            PluginAbiRange::parse_version(value)
                .map_err(|_| ServerConfigError::Config(format!("invalid {key} `{value}`")))
        })
        .transpose()
}
