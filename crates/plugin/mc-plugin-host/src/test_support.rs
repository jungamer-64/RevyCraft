use crate::PluginHostError;
use crate::config::ServerConfig;
use crate::registry::{LoadedPluginSet, ProtocolRegistry};
use crate::runtime::{
    AuthProfileHandle, GameplayProfileHandle, RuntimeReloadContext, StorageProfileHandle,
};
use mc_core::PluginGenerationId;
use std::sync::Arc;

pub use crate::host::{PluginAbiRange, PluginFailureAction, PluginFailureMatrix, PluginHost};
pub use crate::plugin_host::{
    InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin, InProcessStoragePlugin,
    PluginCatalog,
};

#[must_use]
pub fn build_in_process_plugin_host(
    catalog: PluginCatalog,
    abi_range: PluginAbiRange,
    failure_matrix: PluginFailureMatrix,
) -> Arc<PluginHost> {
    Arc::new(PluginHost::new(catalog, abi_range, failure_matrix))
}

/// # Errors
///
/// Returns [`PluginHostError`] when the host cannot materialize the protocol snapshot.
pub fn load_protocol_registry(host: &Arc<PluginHost>) -> Result<ProtocolRegistry, PluginHostError> {
    host.load_protocol_registry()
}

/// # Errors
///
/// Returns [`PluginHostError`] when the host cannot materialize the protocol-only plugin set.
pub fn load_protocol_plugin_set(
    host: &Arc<PluginHost>,
) -> Result<LoadedPluginSet, PluginHostError> {
    let protocols = host.load_protocol_registry()?;
    let mut loaded_plugins = LoadedPluginSet::new();
    loaded_plugins.replace_protocols(protocols);
    Ok(loaded_plugins)
}

/// # Errors
///
/// Returns [`PluginHostError`] when the gameplay profiles cannot be activated.
pub fn activate_gameplay_profiles(
    host: &PluginHost,
    config: &ServerConfig,
) -> Result<(), PluginHostError> {
    host.activate_gameplay_profiles(config)
}

/// # Errors
///
/// Returns [`PluginHostError`] when the storage profile cannot be activated.
pub fn activate_storage_profile(
    host: &PluginHost,
    profile_id: &str,
) -> Result<(), PluginHostError> {
    host.activate_storage_profile(profile_id)
}

/// # Errors
///
/// Returns [`PluginHostError`] when the auth profile cannot be activated.
pub fn activate_auth_profile(host: &PluginHost, profile_id: &str) -> Result<(), PluginHostError> {
    host.activate_auth_profile(profile_id)
}

#[must_use]
pub fn resolve_gameplay_profile(
    host: &PluginHost,
    profile_id: &str,
) -> Option<Arc<dyn GameplayProfileHandle>> {
    host.resolve_gameplay_profile(profile_id)
        .map(|profile| profile as Arc<dyn GameplayProfileHandle>)
}

#[must_use]
pub fn resolve_storage_profile(
    host: &PluginHost,
    profile_id: &str,
) -> Option<Arc<dyn StorageProfileHandle>> {
    host.resolve_storage_profile(profile_id)
        .map(|profile| profile as Arc<dyn StorageProfileHandle>)
}

#[must_use]
pub fn resolve_auth_profile(
    host: &PluginHost,
    profile_id: &str,
) -> Option<Arc<dyn AuthProfileHandle>> {
    host.resolve_auth_profile(profile_id)
        .map(|profile| profile as Arc<dyn AuthProfileHandle>)
}

/// # Errors
///
/// Returns [`PluginHostError`] when the replacement generation cannot be loaded.
pub fn replace_in_process_protocol_plugin(
    host: &PluginHost,
    plugin: InProcessProtocolPlugin,
) -> Result<PluginGenerationId, PluginHostError> {
    host.replace_in_process_protocol_plugin(plugin)
}

/// # Errors
///
/// Returns [`PluginHostError`] when a modified packaged plugin cannot be reloaded.
pub fn reload_modified(host: &PluginHost) -> Result<Vec<String>, PluginHostError> {
    host.reload_modified()
}

/// # Errors
///
/// Returns [`PluginHostError`] when a modified packaged plugin cannot be reloaded.
pub fn reload_modified_with_context(
    host: &PluginHost,
    runtime: &RuntimeReloadContext,
) -> Result<Vec<String>, PluginHostError> {
    host.reload_modified_with_context(runtime)
}

#[must_use]
pub fn take_pending_fatal_error(host: &PluginHost) -> Option<PluginHostError> {
    host.take_pending_fatal_error()
}
