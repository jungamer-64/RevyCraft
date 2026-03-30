mod document;
mod error;
mod normalize;
mod reload_plan;
mod schema;
mod types;
mod validate;

#[cfg(test)]
mod tests;

pub use self::error::{ServerConfigError, ServerConfigSource};
pub use self::reload_plan::{
    CoreReloadPlan, FullReloadPlan, LiveConfig, StaticConfig, TopologyReloadPlan,
};
pub use self::types::{
    AdminConfig, AdminPrincipalConfig, AdminSurfaceConfig, BEDROCK_BASELINE_ADAPTER_ID,
    BEDROCK_OFFLINE_AUTH_PROFILE_ID, BootstrapConfig, DEFAULT_TOPOLOGY_DRAIN_GRACE_SECS, LevelType,
    NetworkConfig, PluginBufferLimits, PluginsConfig, ProfilesConfig, ServerConfig, TopologyConfig,
};
pub use self::validate::ValidatedServerConfig;
pub use revy_server_types::{
    AdminPermission, PluginAbiVersionView, PluginFailureAction, PluginFailureMatrix,
    PluginHostAdminSurfaceSelectionView, PluginHostBootstrapSelectionView,
    PluginHostBufferLimitsView, PluginHostRuntimeSelectionView,
};

use std::path::Path;

impl ServerConfig {
    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when `server.toml` cannot be read or parsed.
    pub fn from_toml(path: &Path) -> Result<ValidatedServerConfig, ServerConfigError> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let document = document::load_server_config_document(path)?;
        normalize::normalize_server_config_document(document, parent)?.validate_owned()
    }

    #[must_use]
    pub fn live_config(&self) -> LiveConfig {
        reload_plan::live_config(self)
    }

    #[must_use]
    pub fn static_config(&self) -> StaticConfig {
        reload_plan::static_config(self)
    }

    #[must_use]
    pub fn plugin_host_bootstrap_view(&self) -> PluginHostBootstrapSelectionView {
        PluginHostBootstrapSelectionView {
            storage_profile: self.bootstrap.storage_profile.clone(),
            plugins_dir: self.bootstrap.plugins_dir.clone(),
            plugin_abi_min: PluginAbiVersionView {
                major: self.bootstrap.plugin_abi_min.major,
                minor: self.bootstrap.plugin_abi_min.minor,
            },
            plugin_abi_max: PluginAbiVersionView {
                major: self.bootstrap.plugin_abi_max.major,
                minor: self.bootstrap.plugin_abi_max.minor,
            },
        }
    }

    #[must_use]
    pub fn plugin_host_runtime_selection_view(&self) -> PluginHostRuntimeSelectionView {
        PluginHostRuntimeSelectionView {
            be_enabled: self.topology.be_enabled,
            auth_profile: self.profiles.auth.clone(),
            bedrock_auth_profile: self.profiles.bedrock_auth.clone(),
            default_gameplay_profile: self.profiles.default_gameplay.clone(),
            gameplay_profile_map: self.profiles.gameplay_map.clone(),
            admin_surfaces: self
                .admin
                .surfaces
                .iter()
                .map(
                    |(instance_id, surface)| PluginHostAdminSurfaceSelectionView {
                        instance_id: instance_id.clone(),
                        profile: surface.profile.clone(),
                        config_path: surface.config.clone(),
                    },
                )
                .collect(),
            plugin_allowlist: self.plugins.allowlist.clone(),
            buffer_limits: PluginHostBufferLimitsView {
                protocol_response_bytes: self.plugins.buffer_limits.protocol_response_bytes,
                gameplay_response_bytes: self.plugins.buffer_limits.gameplay_response_bytes,
                storage_response_bytes: self.plugins.buffer_limits.storage_response_bytes,
                auth_response_bytes: self.plugins.buffer_limits.auth_response_bytes,
                admin_surface_response_bytes: self
                    .plugins
                    .buffer_limits
                    .admin_surface_response_bytes,
                callback_payload_bytes: self.plugins.buffer_limits.callback_payload_bytes,
                metadata_bytes: self.plugins.buffer_limits.metadata_bytes,
            },
            failure_matrix: self.plugins.failure_policy,
        }
    }

    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when the candidate changes restart-only state.
    pub fn plan_topology_reload(
        &self,
        candidate: &Self,
    ) -> Result<TopologyReloadPlan, ServerConfigError> {
        reload_plan::plan_topology_reload(self, candidate)
    }

    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when the candidate changes restart-only state.
    pub fn plan_core_reload(&self, candidate: &Self) -> Result<CoreReloadPlan, ServerConfigError> {
        reload_plan::plan_core_reload(self, candidate)
    }

    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when the candidate changes restart-only state.
    pub fn plan_full_reload(&self, candidate: &Self) -> Result<FullReloadPlan, ServerConfigError> {
        reload_plan::plan_full_reload(self, candidate)
    }

    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when validated fields are inconsistent.
    pub fn validate(&self) -> Result<(), ServerConfigError> {
        validate::validate_server_config(self)
    }

    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when config-only invariants are violated.
    pub fn validate_owned(self) -> Result<ValidatedServerConfig, ServerConfigError> {
        self.try_into()
    }
}
