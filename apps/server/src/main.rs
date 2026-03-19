use server_runtime::RuntimeError;
use server_runtime::config::ServerConfig;
use server_runtime::host::plugin_host_from_config;
use server_runtime::registry::RuntimeRegistries;
use server_runtime::runtime::spawn_server;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), RuntimeError> {
    let config = ServerConfig::from_properties(Path::new("runtime/server.properties"))?;
    let mut registries = RuntimeRegistries::new();

    let plugin_host = plugin_host_from_config(&config)?.ok_or_else(|| {
        RuntimeError::Config(format!(
            "no packaged plugins discovered under `{}`",
            config.plugins_dir.display()
        ))
    })?;
    plugin_host.load_into_registries(&mut registries)?;

    let server = spawn_server(config, registries).await?;
    for binding in server.listener_bindings() {
        println!(
            "server listening on {} via {:?} for {:?}",
            binding.local_addr, binding.transport, binding.adapter_ids
        );
    }
    tokio::signal::ctrl_c()
        .await
        .map_err(|error| RuntimeError::Config(format!("failed to wait for ctrl-c: {error}")))?;
    server.shutdown().await
}
