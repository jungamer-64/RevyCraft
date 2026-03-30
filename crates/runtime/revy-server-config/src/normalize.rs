use crate::error::ServerConfigError;
use crate::schema::{
    AdminPrincipalDocument, AdminSurfaceDocument, FailurePolicyDocument,
    PluginBufferLimitsDocument, ServerConfigDocument,
};
use crate::types::{
    AdminConfig, AdminPrincipalConfig, AdminSurfaceConfig, BEDROCK_BASELINE_ADAPTER_ID,
    BEDROCK_OFFLINE_AUTH_PROFILE_ID, BootstrapConfig, DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS, LevelType,
    NetworkConfig, PluginBufferLimits, PluginsConfig, ProfilesConfig, ServerConfig, TopologyConfig,
};
use crate::{AdminPermission, PluginFailureAction, PluginFailureMatrix};
use mc_plugin_api::abi::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use mc_plugin_api::{AdapterId, AuthProfileId, GameplayProfileId, StorageProfileId};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

pub(crate) fn normalize_server_config_document(
    document: ServerConfigDocument,
    parent: &Path,
) -> Result<ServerConfig, ServerConfigError> {
    let level_name = document
        .static_config
        .bootstrap
        .level_name
        .clone()
        .unwrap_or_else(|| "world".to_string());
    Ok(ServerConfig {
        bootstrap: BootstrapConfig {
            online_mode: document
                .static_config
                .bootstrap
                .online_mode
                .unwrap_or(false),
            level_name: level_name.clone(),
            level_type: LevelType::parse(
                document
                    .static_config
                    .bootstrap
                    .level_type
                    .as_deref()
                    .unwrap_or("flat"),
            )?,
            game_mode: document.static_config.bootstrap.game_mode.unwrap_or(0),
            difficulty: document.static_config.bootstrap.difficulty.unwrap_or(1),
            view_distance: document.static_config.bootstrap.view_distance.unwrap_or(2),
            world_dir: resolve_world_dir(
                parent,
                document.static_config.bootstrap.world_dir.as_deref(),
                Some(level_name.as_str()),
            ),
            storage_profile: document
                .static_config
                .bootstrap
                .storage_profile
                .unwrap_or_else(|| StorageProfileId::new("je-anvil-1_7_10")),
            plugins_dir: resolve_config_path(
                parent,
                document.static_config.plugins.plugins_dir.as_deref(),
                Path::new("plugins"),
            ),
            plugin_abi_min: parse_plugin_abi(
                document.static_config.plugins.plugin_abi_min.as_deref(),
                "static.plugins.plugin_abi_min",
            )?
            .unwrap_or(CURRENT_PLUGIN_ABI),
            plugin_abi_max: parse_plugin_abi(
                document.static_config.plugins.plugin_abi_max.as_deref(),
                "static.plugins.plugin_abi_max",
            )?
            .unwrap_or(CURRENT_PLUGIN_ABI),
        },
        network: NetworkConfig {
            server_ip: parse_server_ip(document.live.network.server_ip.as_deref())?,
            server_port: document.live.network.server_port.unwrap_or(25565),
            motd: document
                .live
                .network
                .motd
                .unwrap_or_else(|| "Multi-version Rust server".to_string()),
            max_players: document.live.network.max_players.unwrap_or(20),
        },
        topology: TopologyConfig {
            be_enabled: document.live.topology.be_enabled.unwrap_or(false),
            default_adapter: document
                .live
                .topology
                .default_adapter
                .unwrap_or_else(|| AdapterId::new("je-5")),
            enabled_adapters: normalize_optional_vec(document.live.topology.enabled_adapters),
            default_bedrock_adapter: document
                .live
                .topology
                .default_bedrock_adapter
                .unwrap_or_else(|| AdapterId::new(BEDROCK_BASELINE_ADAPTER_ID)),
            enabled_bedrock_adapters: normalize_optional_vec(
                document.live.topology.enabled_bedrock_adapters,
            ),
            reload_watch: document.live.topology.reload_watch.unwrap_or(false),
            drain_grace_secs: document
                .live
                .topology
                .drain_grace_secs
                .unwrap_or(DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS),
        },
        plugins: PluginsConfig {
            allowlist: normalize_optional_vec(document.live.plugins.allowlist),
            reload_watch: document.live.plugins.reload_watch.unwrap_or(false),
            buffer_limits: parse_plugin_buffer_limits(document.live.plugins.buffer_limits),
            failure_policy: parse_failure_policy_document(document.live.plugins.failure_policy)?,
        },
        profiles: ProfilesConfig {
            auth: document
                .live
                .profiles
                .auth
                .unwrap_or_else(|| AuthProfileId::new("offline-v1")),
            bedrock_auth: document
                .live
                .profiles
                .bedrock_auth
                .unwrap_or_else(|| AuthProfileId::new(BEDROCK_OFFLINE_AUTH_PROFILE_ID)),
            default_gameplay: document
                .live
                .profiles
                .default_gameplay
                .unwrap_or_else(|| GameplayProfileId::new("canonical")),
            gameplay_map: document.live.profiles.gameplay_map,
        },
        admin: AdminConfig {
            surfaces: parse_admin_surface_config(parent, document.live.admin.surfaces)?,
            principals: parse_admin_principal_config(document.static_config.admin.principals)?,
        },
    })
}

fn parse_failure_policy_document(
    document: FailurePolicyDocument,
) -> Result<PluginFailureMatrix, ServerConfigError> {
    let defaults = PluginFailureMatrix::default();
    Ok(PluginFailureMatrix {
        protocol: parse_failure_policy(
            document.protocol.as_deref(),
            parse_protocol_failure_policy,
            defaults.protocol,
        )?,
        gameplay: parse_failure_policy(
            document.gameplay.as_deref(),
            parse_gameplay_failure_policy,
            defaults.gameplay,
        )?,
        storage: parse_failure_policy(
            document.storage.as_deref(),
            parse_storage_failure_policy,
            defaults.storage,
        )?,
        auth: parse_failure_policy(
            document.auth.as_deref(),
            parse_auth_failure_policy,
            defaults.auth,
        )?,
        admin_surface: parse_failure_policy(
            document.admin_surface.as_deref(),
            parse_admin_surface_failure_policy,
            defaults.admin_surface,
        )?,
    })
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
    let mut entries = document.into_iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    for (principal_id, principal) in entries {
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
