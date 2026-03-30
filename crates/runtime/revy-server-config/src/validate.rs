use crate::error::ServerConfigError;
use crate::types::{AdminPrincipalConfig, AdminSurfaceConfig, ServerConfig};
use mc_plugin_api::AdapterId;
use mc_plugin_api::abi::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use std::collections::{HashMap, HashSet};
use std::ops::Deref;

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
        validate_server_config(&config)?;
        Ok(Self(config))
    }
}

pub(crate) fn validate_server_config(config: &ServerConfig) -> Result<(), ServerConfigError> {
    validate_plugin_abi_range(
        config.bootstrap.plugin_abi_min,
        config.bootstrap.plugin_abi_max,
    )?;
    validate_enabled_adapters(
        config.topology.enabled_adapters.as_deref(),
        &config.topology.default_adapter,
        "enabled-adapters",
        "default-adapter",
    )?;
    validate_enabled_adapters(
        config.topology.enabled_bedrock_adapters.as_deref(),
        &config.topology.default_bedrock_adapter,
        "enabled-bedrock-adapters",
        "default-bedrock-adapter",
    )?;
    validate_admin_surfaces(&config.admin.surfaces)?;
    validate_admin_principals(&config.admin.principals)
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
    let mut entries = principals.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    for (principal_id, principal) in entries {
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
