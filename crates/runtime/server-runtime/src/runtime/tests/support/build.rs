use super::*;
use crate::runtime::RunningServer;
use mc_core::PlayerId;

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
            let runtime_selection = plugin_host_runtime_selection_test_config(&config);
            let loaded_plugins = plugin_host.load_plugin_set(&runtime_selection)?;
            boot_server(
                source,
                config,
                loaded_plugins,
                Some(plugin_host.runtime_host()),
            )
            .await
        }
        None => {
            boot_server(
                source.clone(),
                source.load()?,
                loaded_plugins.loaded_plugins,
                None,
            )
            .await
        }
    }
}

pub(crate) async fn build_reloadable_test_server(
    config: ServerConfig,
    loaded_plugins: LoadedPluginTestEnvironment,
) -> Result<RunningServer, RuntimeError> {
    build_reloadable_test_server_from_source(ServerConfigSource::Inline(config), loaded_plugins)
        .await
}

pub(crate) async fn build_reloadable_test_server_from_source(
    source: ServerConfigSource,
    loaded_plugins: LoadedPluginTestEnvironment,
) -> Result<RunningServer, RuntimeError> {
    let LoadedPluginTestEnvironment {
        loaded_plugins: _loaded_plugins,
        plugin_host,
    } = loaded_plugins;
    let plugin_host = plugin_host.ok_or_else(|| {
        RuntimeError::Config("reloadable test server requires a reload host".to_string())
    })?;
    let config = source.load()?;
    let runtime_selection = plugin_host_runtime_selection_test_config(&config);
    let loaded_plugins = plugin_host.load_plugin_set(&runtime_selection)?;
    boot_server(
        source,
        config,
        loaded_plugins,
        Some(plugin_host.runtime_host()),
    )
    .await
}

pub(crate) fn active_protocol_registry(server: &RunningServer) -> ProtocolRegistry {
    server.runtime.active_generation().protocol_registry.clone()
}

pub(crate) async fn open_test_crafting_table(
    server: &RunningServer,
    player_id: PlayerId,
    window_id: u8,
    title: &str,
) -> Result<(), RuntimeError> {
    server
        .runtime
        .open_test_crafting_table(player_id, window_id, title)
        .await
}

pub(crate) async fn close_test_container(
    server: &RunningServer,
    player_id: PlayerId,
    window_id: u8,
) -> Result<(), RuntimeError> {
    server
        .runtime
        .close_test_container(player_id, window_id)
        .await
}

pub(crate) async fn open_test_furnace(
    server: &RunningServer,
    player_id: PlayerId,
    window_id: u8,
    title: &str,
) -> Result<(), RuntimeError> {
    server
        .runtime
        .open_test_furnace(player_id, window_id, title)
        .await
}

pub(crate) async fn open_test_chest(
    server: &RunningServer,
    player_id: PlayerId,
    window_id: u8,
    title: &str,
) -> Result<(), RuntimeError> {
    server
        .runtime
        .open_test_chest(player_id, window_id, title)
        .await
}
