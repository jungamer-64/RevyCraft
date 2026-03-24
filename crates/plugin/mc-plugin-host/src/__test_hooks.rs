use crate::PluginHostError;
use crate::config::{BootstrapConfig, RuntimeSelectionConfig};
use crate::host::{
    PluginAbiRange, PluginFailureMatrix, PluginHost, PluginHostStatusSnapshot,
    plugin_host_from_config,
};
use crate::plugin_host::PluginCatalog;
pub use crate::plugin_host::{
    InProcessAdminUiPlugin, InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin,
    InProcessStoragePlugin,
};
use crate::registry::{LoadedPluginSet, ProtocolRegistry};
use crate::runtime::{
    AdminUiProfileHandle, AuthProfileHandle, GameplayProfileHandle, RuntimePluginHost,
    RuntimeReloadContext, StorageProfileHandle,
};
use mc_core::{PluginGenerationId, StorageProfileId};
use std::sync::Arc;

#[derive(Clone)]
pub struct BuiltTestHost {
    inner: Arc<PluginHost>,
}

#[derive(Default)]
pub struct InProcessHostBuildInput {
    pub protocol_plugins: Vec<InProcessProtocolPlugin>,
    pub gameplay_plugins: Vec<InProcessGameplayPlugin>,
    pub storage_plugins: Vec<InProcessStoragePlugin>,
    pub auth_plugins: Vec<InProcessAuthPlugin>,
    pub admin_ui_plugins: Vec<InProcessAdminUiPlugin>,
    pub bootstrap_config: crate::config::BootstrapConfig,
    pub abi_range: PluginAbiRange,
    pub failure_matrix: PluginFailureMatrix,
}

/// # Errors
///
/// Returns [`PluginHostError`] when packaged plugin discovery fails.
pub fn discover(config: &BootstrapConfig) -> Result<Option<BuiltTestHost>, PluginHostError> {
    plugin_host_from_config(config).map(|host| host.map(|inner| BuiltTestHost { inner }))
}

#[must_use]
pub fn build_in_process_host(input: InProcessHostBuildInput) -> BuiltTestHost {
    let mut catalog = PluginCatalog::default();
    for plugin in input.protocol_plugins {
        catalog.register_in_process_protocol_plugin(plugin);
    }
    for plugin in input.gameplay_plugins {
        catalog.register_in_process_gameplay_plugin(plugin);
    }
    for plugin in input.storage_plugins {
        catalog.register_in_process_storage_plugin(plugin);
    }
    for plugin in input.auth_plugins {
        catalog.register_in_process_auth_plugin(plugin);
    }
    for plugin in input.admin_ui_plugins {
        catalog.register_in_process_admin_ui_plugin(plugin);
    }
    BuiltTestHost {
        inner: Arc::new(PluginHost::new(
            catalog,
            input.bootstrap_config,
            input.abi_range,
            input.failure_matrix,
        )),
    }
}

#[must_use]
pub fn runtime_host(host: &BuiltTestHost) -> Arc<dyn RuntimePluginHost> {
    Arc::clone(&host.inner) as Arc<dyn RuntimePluginHost>
}

#[must_use]
pub fn status(host: &BuiltTestHost) -> PluginHostStatusSnapshot {
    host.inner.status()
}

/// # Errors
///
/// Returns [`PluginHostError`] when the host cannot materialize the requested runtime snapshot.
pub fn load_plugin_set(
    host: &BuiltTestHost,
    config: &RuntimeSelectionConfig,
) -> Result<LoadedPluginSet, PluginHostError> {
    host.inner.load_plugin_set(config)
}

/// # Errors
///
/// Returns [`PluginHostError`] when the host cannot materialize the protocol snapshot.
pub fn load_protocol_registry(host: &BuiltTestHost) -> Result<ProtocolRegistry, PluginHostError> {
    host.inner
        .load_protocol_registry(&crate::config::RuntimeSelectionConfig::default())
}

/// # Errors
///
/// Returns [`PluginHostError`] when the host cannot materialize the protocol-only plugin set.
pub fn load_protocol_plugin_set(host: &BuiltTestHost) -> Result<LoadedPluginSet, PluginHostError> {
    let protocols = host
        .inner
        .load_protocol_registry(&crate::config::RuntimeSelectionConfig::default())?;
    let mut loaded_plugins = LoadedPluginSet::new();
    loaded_plugins.replace_protocols(protocols);
    Ok(loaded_plugins)
}

/// # Errors
///
/// Returns [`PluginHostError`] when the gameplay profiles cannot be activated.
pub fn activate_gameplay_profiles(
    host: &BuiltTestHost,
    config: &RuntimeSelectionConfig,
) -> Result<(), PluginHostError> {
    host.inner.activate_gameplay_profiles(config)
}

/// # Errors
///
/// Returns [`PluginHostError`] when the storage profile cannot be activated.
pub fn activate_storage_profile(
    host: &BuiltTestHost,
    profile_id: &str,
) -> Result<(), PluginHostError> {
    host.inner
        .activate_storage_profile(&StorageProfileId::new(profile_id))
}

/// # Errors
///
/// Returns [`PluginHostError`] when the auth profile cannot be activated.
pub fn activate_auth_profile(
    host: &BuiltTestHost,
    profile_id: &str,
) -> Result<(), PluginHostError> {
    host.inner.activate_auth_profile(profile_id)
}

#[must_use]
pub fn resolve_gameplay_profile(
    host: &BuiltTestHost,
    profile_id: &str,
) -> Option<Arc<dyn GameplayProfileHandle>> {
    host.inner
        .resolve_gameplay_profile(profile_id)
        .map(|profile| profile as Arc<dyn GameplayProfileHandle>)
}

#[must_use]
pub fn resolve_storage_profile(
    host: &BuiltTestHost,
    profile_id: &str,
) -> Option<Arc<dyn StorageProfileHandle>> {
    host.inner
        .resolve_storage_profile(profile_id)
        .map(|profile| profile as Arc<dyn StorageProfileHandle>)
}

#[must_use]
pub fn resolve_auth_profile(
    host: &BuiltTestHost,
    profile_id: &str,
) -> Option<Arc<dyn AuthProfileHandle>> {
    host.inner
        .resolve_auth_profile(profile_id)
        .map(|profile| profile as Arc<dyn AuthProfileHandle>)
}

#[must_use]
pub fn resolve_admin_ui_profile(
    host: &BuiltTestHost,
    profile_id: &str,
) -> Option<Arc<dyn AdminUiProfileHandle>> {
    host.inner
        .resolve_admin_ui_profile(profile_id)
        .map(|profile| profile as Arc<dyn AdminUiProfileHandle>)
}

/// # Errors
///
/// Returns [`PluginHostError`] when the replacement generation cannot be loaded.
pub fn replace_in_process_protocol_plugin(
    host: &BuiltTestHost,
    plugin: InProcessProtocolPlugin,
) -> Result<PluginGenerationId, PluginHostError> {
    host.inner.replace_in_process_protocol_plugin(plugin)
}

/// # Errors
///
/// Returns [`PluginHostError`] when a modified packaged plugin cannot be reloaded.
pub fn reload_modified(host: &BuiltTestHost) -> Result<Vec<String>, PluginHostError> {
    host.inner.reload_modified()
}

/// # Errors
///
/// Returns [`PluginHostError`] when a modified packaged plugin cannot be reloaded.
pub fn reload_modified_with_context(
    host: &BuiltTestHost,
    runtime: &RuntimeReloadContext,
) -> Result<Vec<String>, PluginHostError> {
    host.inner.reload_modified_with_context(runtime)
}

#[must_use]
pub fn artifact_quarantine_reason(host: &BuiltTestHost, plugin_id: &str) -> Option<String> {
    host.inner.artifact_quarantine_reason(plugin_id)
}

#[must_use]
pub fn take_pending_fatal_error(host: &BuiltTestHost) -> Option<PluginHostError> {
    host.inner.take_pending_fatal_error()
}
