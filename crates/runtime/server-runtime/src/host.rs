use crate::RuntimeError;
use crate::config::ServerConfig;
use crate::registry::RuntimeRegistries;
use std::sync::Arc;

pub use mc_plugin_host::host::{
    AuthGeneration, AuthPluginStatusSnapshot, GameplayGeneration, GameplayPluginStatusSnapshot,
    HotSwappableAuthProfile, HotSwappableGameplayProfile, HotSwappableStorageProfile,
    InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin, InProcessStoragePlugin,
    PluginAbiRange, PluginArtifactStatusSnapshot, PluginCatalog, PluginFailureAction,
    PluginFailureMatrix, PluginHost, PluginHostStatusSnapshot, ProtocolPluginStatusSnapshot,
    StoragePluginStatusSnapshot, plugin_reload_poll_interval_ms,
};

fn plugin_host_config(config: &ServerConfig) -> mc_plugin_host::config::ServerConfig {
    mc_plugin_host::config::ServerConfig {
        be_enabled: config.be_enabled,
        storage_profile: config.storage_profile.clone(),
        auth_profile: config.auth_profile.clone(),
        bedrock_auth_profile: config.bedrock_auth_profile.clone(),
        default_gameplay_profile: config.default_gameplay_profile.clone(),
        gameplay_profile_map: config.gameplay_profile_map.clone(),
        plugins_dir: config.plugins_dir.clone(),
        plugin_allowlist: config.plugin_allowlist.clone(),
        plugin_failure_policy_protocol: config.plugin_failure_policy_protocol,
        plugin_failure_policy_gameplay: config.plugin_failure_policy_gameplay,
        plugin_failure_policy_storage: config.plugin_failure_policy_storage,
        plugin_failure_policy_auth: config.plugin_failure_policy_auth,
        plugin_abi_min: config.plugin_abi_min,
        plugin_abi_max: config.plugin_abi_max,
    }
}

/// # Errors
///
/// Returns [`RuntimeError`] when the plugin catalog cannot be discovered or a
/// configured plugin policy is invalid.
pub fn plugin_host_from_config(config: &ServerConfig) -> Result<Option<Arc<PluginHost>>, RuntimeError> {
    mc_plugin_host::host::plugin_host_from_config(&plugin_host_config(config)).map_err(RuntimeError::from)
}

/// # Errors
///
/// Returns [`RuntimeError`] when protocol adapters or runtime-selected plugin
/// profiles cannot be loaded from the packaged plugin catalog.
pub fn initialize_runtime_registries_from_config(
    plugin_host: &Arc<PluginHost>,
    config: &ServerConfig,
    registries: &mut RuntimeRegistries,
) -> Result<(), RuntimeError> {
    plugin_host
        .initialize_runtime_registries(&plugin_host_config(config), registries)
        .map_err(RuntimeError::from)
}

/// # Errors
///
/// Returns [`RuntimeError`] when gameplay, storage, or auth runtime profiles
/// cannot be activated from the packaged plugin catalog.
pub fn activate_runtime_profiles_from_config(
    plugin_host: &Arc<PluginHost>,
    config: &ServerConfig,
) -> Result<(), RuntimeError> {
    plugin_host
        .activate_runtime_profiles(&plugin_host_config(config))
        .map_err(RuntimeError::from)
}
