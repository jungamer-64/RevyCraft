use crate::PluginHostError;
use crate::config::ServerConfig;
use crate::plugin_host::PluginCatalog;
use crate::registry::{LoadedPluginSet, ProtocolRegistry};
use crate::runtime::{
    AuthProfileHandle, GameplayProfileHandle, RuntimeReloadContext, StorageProfileHandle,
};
use mc_core::PluginGenerationId;
use std::sync::Arc;

pub use crate::host::{PluginAbiRange, PluginFailureAction, PluginFailureMatrix, PluginHost};
pub use crate::plugin_host::{
    InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin, InProcessStoragePlugin,
};

#[derive(Default)]
pub struct TestPluginHostBuilder {
    protocol_plugins: Vec<InProcessProtocolPlugin>,
    gameplay_plugins: Vec<InProcessGameplayPlugin>,
    storage_plugins: Vec<InProcessStoragePlugin>,
    auth_plugins: Vec<InProcessAuthPlugin>,
    abi_range: PluginAbiRange,
    failure_matrix: PluginFailureMatrix,
}

impl TestPluginHostBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn protocol_raw(mut self, plugin: InProcessProtocolPlugin) -> Self {
        self.protocol_plugins.push(plugin);
        self
    }

    #[must_use]
    pub fn gameplay_raw(mut self, plugin: InProcessGameplayPlugin) -> Self {
        self.gameplay_plugins.push(plugin);
        self
    }

    #[must_use]
    pub fn storage_raw(mut self, plugin: InProcessStoragePlugin) -> Self {
        self.storage_plugins.push(plugin);
        self
    }

    #[must_use]
    pub fn auth_raw(mut self, plugin: InProcessAuthPlugin) -> Self {
        self.auth_plugins.push(plugin);
        self
    }

    #[must_use]
    pub const fn abi_range(mut self, abi_range: PluginAbiRange) -> Self {
        self.abi_range = abi_range;
        self
    }

    #[must_use]
    pub const fn failure_matrix(mut self, failure_matrix: PluginFailureMatrix) -> Self {
        self.failure_matrix = failure_matrix;
        self
    }

    #[must_use]
    pub fn build(self) -> Arc<PluginHost> {
        let mut catalog = PluginCatalog::default();
        for plugin in self.protocol_plugins {
            catalog.register_in_process_protocol_plugin(plugin);
        }
        for plugin in self.gameplay_plugins {
            catalog.register_in_process_gameplay_plugin(plugin);
        }
        for plugin in self.storage_plugins {
            catalog.register_in_process_storage_plugin(plugin);
        }
        for plugin in self.auth_plugins {
            catalog.register_in_process_auth_plugin(plugin);
        }
        Arc::new(PluginHost::new(
            catalog,
            self.abi_range,
            self.failure_matrix,
        ))
    }
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
