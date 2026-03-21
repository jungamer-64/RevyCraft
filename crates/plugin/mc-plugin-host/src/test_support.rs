use crate::PluginHostError;
use crate::config::ServerConfig;
use crate::host::{PluginHost, PluginHostStatusSnapshot, plugin_host_from_config};
use crate::plugin_host::PluginCatalog;
use crate::registry::{LoadedPluginSet, ProtocolRegistry};
use crate::runtime::{
    AuthProfileHandle, GameplayProfileHandle, RuntimePluginHost, RuntimeReloadContext,
    StorageProfileHandle,
};
use mc_core::PluginGenerationId;
use std::sync::Arc;

pub use crate::host::{PluginAbiRange, PluginFailureAction, PluginFailureMatrix};
pub use crate::plugin_host::{
    InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin, InProcessStoragePlugin,
};

#[derive(Clone)]
pub struct TestPluginHost {
    inner: Arc<PluginHost>,
}

impl TestPluginHost {
    /// # Errors
    ///
    /// Returns [`PluginHostError`] when packaged plugin discovery fails.
    pub fn discover(config: &ServerConfig) -> Result<Option<Self>, PluginHostError> {
        plugin_host_from_config(config).map(|host| host.map(|inner| Self { inner }))
    }

    #[must_use]
    pub fn runtime_host(&self) -> Arc<dyn RuntimePluginHost> {
        Arc::clone(&self.inner) as Arc<dyn RuntimePluginHost>
    }

    #[must_use]
    pub fn status(&self) -> PluginHostStatusSnapshot {
        self.inner.status()
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the host cannot materialize the requested runtime snapshot.
    pub fn load_plugin_set(
        &self,
        config: &ServerConfig,
    ) -> Result<LoadedPluginSet, PluginHostError> {
        self.inner.load_plugin_set(config)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the host cannot materialize the protocol snapshot.
    pub fn load_protocol_registry(&self) -> Result<ProtocolRegistry, PluginHostError> {
        self.inner.load_protocol_registry()
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the host cannot materialize the protocol-only plugin set.
    pub fn load_protocol_plugin_set(&self) -> Result<LoadedPluginSet, PluginHostError> {
        let protocols = self.load_protocol_registry()?;
        let mut loaded_plugins = LoadedPluginSet::new();
        loaded_plugins.replace_protocols(protocols);
        Ok(loaded_plugins)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the gameplay profiles cannot be activated.
    pub fn activate_gameplay_profiles(&self, config: &ServerConfig) -> Result<(), PluginHostError> {
        self.inner.activate_gameplay_profiles(config)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the storage profile cannot be activated.
    pub fn activate_storage_profile(&self, profile_id: &str) -> Result<(), PluginHostError> {
        self.inner.activate_storage_profile(profile_id)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the auth profile cannot be activated.
    pub fn activate_auth_profile(&self, profile_id: &str) -> Result<(), PluginHostError> {
        self.inner.activate_auth_profile(profile_id)
    }

    #[must_use]
    pub fn resolve_gameplay_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn GameplayProfileHandle>> {
        self.inner
            .resolve_gameplay_profile(profile_id)
            .map(|profile| profile as Arc<dyn GameplayProfileHandle>)
    }

    #[must_use]
    pub fn resolve_storage_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn StorageProfileHandle>> {
        self.inner
            .resolve_storage_profile(profile_id)
            .map(|profile| profile as Arc<dyn StorageProfileHandle>)
    }

    #[must_use]
    pub fn resolve_auth_profile(&self, profile_id: &str) -> Option<Arc<dyn AuthProfileHandle>> {
        self.inner
            .resolve_auth_profile(profile_id)
            .map(|profile| profile as Arc<dyn AuthProfileHandle>)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the replacement generation cannot be loaded.
    pub fn replace_in_process_protocol_plugin(
        &self,
        plugin: InProcessProtocolPlugin,
    ) -> Result<PluginGenerationId, PluginHostError> {
        self.inner.replace_in_process_protocol_plugin(plugin)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when a modified packaged plugin cannot be reloaded.
    pub fn reload_modified(&self) -> Result<Vec<String>, PluginHostError> {
        self.inner.reload_modified()
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when a modified packaged plugin cannot be reloaded.
    pub fn reload_modified_with_context(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<String>, PluginHostError> {
        self.inner.reload_modified_with_context(runtime)
    }

    #[must_use]
    pub fn take_pending_fatal_error(&self) -> Option<PluginHostError> {
        self.inner.take_pending_fatal_error()
    }
}

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
    pub fn build(self) -> TestPluginHost {
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
        TestPluginHost {
            inner: Arc::new(PluginHost::new(
                catalog,
                self.abi_range,
                self.failure_matrix,
            )),
        }
    }
}
