use crate::__test_hooks as hooks;
use crate::PluginHostError;
use crate::config::ServerConfig;
use crate::host::PluginHostStatusSnapshot;
use crate::registry::LoadedPluginSet;
use crate::runtime::{
    AdminUiProfileHandle, AuthProfileHandle, GameplayProfileHandle, RuntimeReloadContext,
    StorageProfileHandle,
};
use mc_core::PluginGenerationId;
use std::sync::Arc;

pub use crate::host::{PluginAbiRange, PluginFailureMatrix};
pub use crate::plugin_host::{
    InProcessAdminUiPlugin, InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin,
    InProcessStoragePlugin,
};

#[derive(Clone)]
pub struct TestPluginHost {
    inner: hooks::BuiltTestHost,
}

impl TestPluginHost {
    /// # Errors
    ///
    /// Returns [`PluginHostError`] when packaged plugin discovery fails.
    pub fn discover(config: &ServerConfig) -> Result<Option<Self>, PluginHostError> {
        hooks::discover(config).map(|host| host.map(|inner| Self { inner }))
    }

    #[must_use]
    pub fn status(&self) -> PluginHostStatusSnapshot {
        hooks::status(&self.inner)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the host cannot materialize the requested runtime snapshot.
    pub fn load_plugin_set(
        &self,
        config: &ServerConfig,
    ) -> Result<LoadedPluginSet, PluginHostError> {
        hooks::load_plugin_set(&self.inner, config)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the host cannot materialize the protocol-only plugin set.
    pub fn load_protocol_plugin_set(&self) -> Result<LoadedPluginSet, PluginHostError> {
        hooks::load_protocol_plugin_set(&self.inner)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the gameplay profiles cannot be activated.
    pub fn activate_gameplay_profiles(&self, config: &ServerConfig) -> Result<(), PluginHostError> {
        hooks::activate_gameplay_profiles(&self.inner, config)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the storage profile cannot be activated.
    pub fn activate_storage_profile(&self, profile_id: &str) -> Result<(), PluginHostError> {
        hooks::activate_storage_profile(&self.inner, profile_id)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the auth profile cannot be activated.
    pub fn activate_auth_profile(&self, profile_id: &str) -> Result<(), PluginHostError> {
        hooks::activate_auth_profile(&self.inner, profile_id)
    }

    #[must_use]
    pub fn resolve_gameplay_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn GameplayProfileHandle>> {
        hooks::resolve_gameplay_profile(&self.inner, profile_id)
    }

    #[must_use]
    pub fn resolve_storage_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn StorageProfileHandle>> {
        hooks::resolve_storage_profile(&self.inner, profile_id)
    }

    #[must_use]
    pub fn resolve_auth_profile(&self, profile_id: &str) -> Option<Arc<dyn AuthProfileHandle>> {
        hooks::resolve_auth_profile(&self.inner, profile_id)
    }

    #[must_use]
    pub fn resolve_admin_ui_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn AdminUiProfileHandle>> {
        hooks::resolve_admin_ui_profile(&self.inner, profile_id)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the replacement generation cannot be loaded.
    pub fn replace_in_process_protocol_plugin(
        &self,
        plugin: InProcessProtocolPlugin,
    ) -> Result<PluginGenerationId, PluginHostError> {
        hooks::replace_in_process_protocol_plugin(&self.inner, plugin)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when a modified packaged plugin cannot be reloaded.
    pub fn reload_modified(&self) -> Result<Vec<String>, PluginHostError> {
        hooks::reload_modified(&self.inner)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when a modified packaged plugin cannot be reloaded.
    pub fn reload_modified_with_context(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<String>, PluginHostError> {
        hooks::reload_modified_with_context(&self.inner, runtime)
    }

    #[must_use]
    pub fn take_pending_fatal_error(&self) -> Option<PluginHostError> {
        hooks::take_pending_fatal_error(&self.inner)
    }
}

#[derive(Default)]
pub struct TestPluginHostBuilder {
    protocol_plugins: Vec<InProcessProtocolPlugin>,
    gameplay_plugins: Vec<InProcessGameplayPlugin>,
    storage_plugins: Vec<InProcessStoragePlugin>,
    auth_plugins: Vec<InProcessAuthPlugin>,
    admin_ui_plugins: Vec<InProcessAdminUiPlugin>,
    bootstrap_config: crate::config::BootstrapConfig,
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
    pub fn admin_ui_raw(mut self, plugin: InProcessAdminUiPlugin) -> Self {
        self.admin_ui_plugins.push(plugin);
        self
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn bootstrap_config(mut self, bootstrap_config: crate::config::BootstrapConfig) -> Self {
        self.bootstrap_config = bootstrap_config;
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
        TestPluginHost {
            inner: hooks::build_in_process_host(hooks::InProcessHostBuildInput {
                protocol_plugins: self.protocol_plugins,
                gameplay_plugins: self.gameplay_plugins,
                storage_plugins: self.storage_plugins,
                auth_plugins: self.auth_plugins,
                admin_ui_plugins: self.admin_ui_plugins,
                bootstrap_config: self.bootstrap_config,
                abi_range: self.abi_range,
                failure_matrix: self.failure_matrix,
            }),
        }
    }
}
