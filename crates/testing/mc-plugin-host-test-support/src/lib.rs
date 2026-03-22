//! Shared reusable in-process plugin-host fixtures for workspace tests.
//!
//! This crate is the sanctioned test/dev surface for building `mc-plugin-host`
//! fixtures outside the host crate itself. Packaged-plugin harness helpers live
//! in `mc-plugin-test-support`.

use mc_core::PluginGenerationId;
use mc_plugin_host::__test_hooks::{
    BuiltTestHost, InProcessHostBuildInput, activate_auth_profile, activate_gameplay_profiles,
    activate_storage_profile, build_in_process_host, discover, load_plugin_set,
    load_protocol_plugin_set, load_protocol_registry, reload_modified,
    reload_modified_with_context, replace_in_process_protocol_plugin, resolve_admin_ui_profile,
    resolve_auth_profile, resolve_gameplay_profile, resolve_storage_profile, runtime_host, status,
    take_pending_fatal_error,
};
use mc_plugin_host::__test_hooks::{
    InProcessAdminUiPlugin, InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin,
    InProcessStoragePlugin,
};
use mc_plugin_host::PluginHostError;
use mc_plugin_host::config::ServerConfig;
pub use mc_plugin_host::host::{PluginAbiRange, PluginFailureAction, PluginFailureMatrix};
use mc_plugin_host::registry::{LoadedPluginSet, ProtocolRegistry};
use mc_plugin_host::runtime::{
    AdminUiProfileHandle, AuthProfileHandle, GameplayProfileHandle, RuntimePluginHost,
    RuntimeReloadContext, StorageProfileHandle,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct TestPluginHost {
    inner: BuiltTestHost,
}

impl TestPluginHost {
    /// # Errors
    ///
    /// Returns [`PluginHostError`] when packaged plugin discovery fails.
    pub fn discover(config: &ServerConfig) -> Result<Option<Self>, PluginHostError> {
        discover(config).map(|host| host.map(|inner| Self { inner }))
    }

    #[must_use]
    pub fn runtime_host(&self) -> Arc<dyn RuntimePluginHost> {
        runtime_host(&self.inner)
    }

    #[must_use]
    pub fn status(&self) -> mc_plugin_host::host::PluginHostStatusSnapshot {
        status(&self.inner)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the host cannot materialize the requested runtime snapshot.
    pub fn load_plugin_set(
        &self,
        config: &ServerConfig,
    ) -> Result<LoadedPluginSet, PluginHostError> {
        load_plugin_set(&self.inner, config)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the host cannot materialize the protocol snapshot.
    pub fn load_protocol_registry(&self) -> Result<ProtocolRegistry, PluginHostError> {
        load_protocol_registry(&self.inner)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the host cannot materialize the protocol-only plugin set.
    pub fn load_protocol_plugin_set(&self) -> Result<LoadedPluginSet, PluginHostError> {
        load_protocol_plugin_set(&self.inner)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the gameplay profiles cannot be activated.
    pub fn activate_gameplay_profiles(&self, config: &ServerConfig) -> Result<(), PluginHostError> {
        activate_gameplay_profiles(&self.inner, config)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the storage profile cannot be activated.
    pub fn activate_storage_profile(&self, profile_id: &str) -> Result<(), PluginHostError> {
        activate_storage_profile(&self.inner, profile_id)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the auth profile cannot be activated.
    pub fn activate_auth_profile(&self, profile_id: &str) -> Result<(), PluginHostError> {
        activate_auth_profile(&self.inner, profile_id)
    }

    #[must_use]
    pub fn resolve_gameplay_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn GameplayProfileHandle>> {
        resolve_gameplay_profile(&self.inner, profile_id)
    }

    #[must_use]
    pub fn resolve_storage_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn StorageProfileHandle>> {
        resolve_storage_profile(&self.inner, profile_id)
    }

    #[must_use]
    pub fn resolve_auth_profile(&self, profile_id: &str) -> Option<Arc<dyn AuthProfileHandle>> {
        resolve_auth_profile(&self.inner, profile_id)
    }

    #[must_use]
    pub fn resolve_admin_ui_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn AdminUiProfileHandle>> {
        resolve_admin_ui_profile(&self.inner, profile_id)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the replacement generation cannot be loaded.
    pub fn replace_in_process_protocol_plugin(
        &self,
        plugin: InProcessProtocolPlugin,
    ) -> Result<PluginGenerationId, PluginHostError> {
        replace_in_process_protocol_plugin(&self.inner, plugin)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when a modified packaged plugin cannot be reloaded.
    pub fn reload_modified(&self) -> Result<Vec<String>, PluginHostError> {
        reload_modified(&self.inner)
    }

    /// # Errors
    ///
    /// Returns [`PluginHostError`] when a modified packaged plugin cannot be reloaded.
    pub fn reload_modified_with_context(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<String>, PluginHostError> {
        reload_modified_with_context(&self.inner, runtime)
    }

    #[must_use]
    pub fn take_pending_fatal_error(&self) -> Option<PluginHostError> {
        take_pending_fatal_error(&self.inner)
    }
}

#[derive(Default)]
pub struct TestPluginHostBuilder {
    protocol_plugins: Vec<InProcessProtocolPlugin>,
    gameplay_plugins: Vec<InProcessGameplayPlugin>,
    storage_plugins: Vec<InProcessStoragePlugin>,
    auth_plugins: Vec<InProcessAuthPlugin>,
    admin_ui_plugins: Vec<InProcessAdminUiPlugin>,
    bootstrap_config: mc_plugin_host::config::BootstrapConfig,
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

    #[must_use]
    pub fn bootstrap_config(
        mut self,
        bootstrap_config: mc_plugin_host::config::BootstrapConfig,
    ) -> Self {
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
            inner: build_in_process_host(InProcessHostBuildInput {
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

pub mod raw {
    pub use mc_plugin_host::__test_hooks::{
        InProcessAdminUiPlugin, InProcessAuthPlugin, InProcessGameplayPlugin,
        InProcessProtocolPlugin, InProcessStoragePlugin,
    };
}

#[cfg(test)]
mod tests {
    use super::{PluginAbiRange, TestPluginHost, TestPluginHostBuilder};
    use crate::raw::InProcessProtocolPlugin;
    use mc_core::{CoreConfig, ServerCore};
    use mc_plugin_host::config::ServerConfig;
    use mc_plugin_host::runtime::RuntimeReloadContext;
    use mc_plugin_proto_je_1_7_10::in_process_plugin_entrypoints as je_1_7_10_entrypoints;
    use mc_plugin_test_support::PackagedPluginHarness;
    use mc_proto_je_1_7_10::JE_1_7_10_ADAPTER_ID;
    use std::path::PathBuf;

    fn je_1_7_10_protocol_plugin() -> InProcessProtocolPlugin {
        InProcessProtocolPlugin {
            plugin_id: JE_1_7_10_ADAPTER_ID.to_string(),
            manifest: je_1_7_10_entrypoints().manifest,
            api: je_1_7_10_entrypoints().api,
        }
    }

    #[test]
    fn builder_runtime_host_and_protocol_snapshot_work() {
        let host = TestPluginHostBuilder::new()
            .protocol_raw(je_1_7_10_protocol_plugin())
            .abi_range(PluginAbiRange::default())
            .build();
        let runtime_host = host.runtime_host();
        let protocols = host
            .load_protocol_plugin_set()
            .expect("in-process protocol plugin set should load");
        assert!(
            runtime_host
                .managed_protocol_ids()
                .contains(&JE_1_7_10_ADAPTER_ID.to_string())
        );
        assert!(
            protocols
                .protocols()
                .resolve_adapter(JE_1_7_10_ADAPTER_ID)
                .is_some()
        );
    }

    #[test]
    fn discover_and_reload_wrappers_work_for_packaged_plugins() {
        let harness = PackagedPluginHarness::shared().expect("packaged harness should exist");
        let config = ServerConfig {
            plugins_dir: harness.dist_dir().to_path_buf(),
            plugin_allowlist: Some(vec![JE_1_7_10_ADAPTER_ID.to_string()]),
            ..ServerConfig::default()
        };
        let host = TestPluginHost::discover(&config)
            .expect("packaged discovery should succeed")
            .expect("expected packaged host");
        let protocols = host
            .load_protocol_plugin_set()
            .expect("packaged protocol plugin set should load");
        assert!(
            protocols
                .protocols()
                .resolve_adapter(JE_1_7_10_ADAPTER_ID)
                .is_some()
        );
        assert_eq!(
            host.reload_modified()
                .expect("reload with no modified artifacts should succeed"),
            Vec::<String>::new()
        );
        let runtime = RuntimeReloadContext {
            protocol_sessions: Vec::new(),
            gameplay_sessions: Vec::new(),
            snapshot: ServerCore::new(CoreConfig::default()).snapshot(),
            world_dir: PathBuf::from("."),
        };
        assert_eq!(
            host.reload_modified_with_context(&runtime)
                .expect("context reload with no modified artifacts should succeed"),
            Vec::<String>::new()
        );
    }
}
