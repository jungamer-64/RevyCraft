use super::*;
use crate::runtime::RunningServer;

#[derive(Clone)]
pub(crate) struct LoadedPluginTestEnvironment {
    pub(crate) loaded_plugins: LoadedPluginSet,
    pub(crate) plugin_host: Option<TestPluginHost>,
}

impl LoadedPluginTestEnvironment {
    pub(crate) fn protocols(&self) -> &ProtocolRegistry {
        self.loaded_plugins.protocols()
    }
}

pub(crate) async fn build_test_server(
    config: ServerConfig,
    loaded_plugins: LoadedPluginTestEnvironment,
) -> Result<RunningServer, RuntimeError> {
    build_test_server_from_source(ServerConfigSource::Inline(config), loaded_plugins).await
}

pub(crate) async fn build_test_server_from_source(
    source: ServerConfigSource,
    loaded_plugins: LoadedPluginTestEnvironment,
) -> Result<RunningServer, RuntimeError> {
    match loaded_plugins.plugin_host {
        Some(plugin_host) => {
            let config = source.load()?;
            let loaded_plugins = plugin_host.load_plugin_set(&config.plugin_host_config())?;
            ServerBuilder::new(source, loaded_plugins)
                .with_reload_host(plugin_host.runtime_host())
                .build()
                .await
                .map(ReloadableRunningServer::into_running_server)
        }
        None => {
            ServerBuilder::new(source, loaded_plugins.loaded_plugins)
                .build()
                .await
        }
    }
}

pub(crate) async fn build_reloadable_test_server(
    config: ServerConfig,
    loaded_plugins: LoadedPluginTestEnvironment,
) -> Result<ReloadableRunningServer, RuntimeError> {
    build_reloadable_test_server_from_source(ServerConfigSource::Inline(config), loaded_plugins)
        .await
}

pub(crate) async fn build_reloadable_test_server_from_source(
    source: ServerConfigSource,
    loaded_plugins: LoadedPluginTestEnvironment,
) -> Result<ReloadableRunningServer, RuntimeError> {
    let LoadedPluginTestEnvironment {
        loaded_plugins: _loaded_plugins,
        plugin_host,
    } = loaded_plugins;
    let plugin_host = plugin_host.ok_or_else(|| {
        RuntimeError::Config("reloadable test server requires a reload host".to_string())
    })?;
    let config = source.load()?;
    let loaded_plugins = plugin_host.load_plugin_set(&config.plugin_host_config())?;
    ServerBuilder::new(source, loaded_plugins)
        .with_reload_host(plugin_host.runtime_host())
        .build()
        .await
}

pub(crate) fn active_protocol_registry(server: &RunningServer) -> ProtocolRegistry {
    server.runtime.active_topology().protocol_registry.clone()
}
