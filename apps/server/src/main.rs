#![allow(clippy::multiple_crate_versions)]
use mc_plugin_host::host::plugin_host_from_config;
use server_runtime::RuntimeError;
use server_runtime::config::{ServerConfig, ServerConfigSource};
use server_runtime::runtime::{ServerBuilder, format_runtime_status_summary};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), RuntimeError> {
    let config = ServerConfig::from_toml(Path::new("runtime/server.toml"))?;
    let plugin_host =
        plugin_host_from_config(&config.plugin_host_bootstrap_config())?.ok_or_else(|| {
            RuntimeError::Config(format!(
                "no packaged plugins discovered under `{}`",
                config.bootstrap.plugins_dir.display()
            ))
        })?;
    let loaded_plugins =
        plugin_host.load_plugin_set(&config.plugin_host_runtime_selection_config())?;

    let server = ServerBuilder::new(
        ServerConfigSource::Toml(Path::new("runtime/server.toml").to_path_buf()),
        loaded_plugins,
    )
    .with_reload_host(plugin_host)
    .build()
    .await?;
    for binding in server.listener_bindings() {
        println!(
            "server listening on {} via {:?} for {:?}",
            binding.local_addr, binding.transport, binding.adapter_ids
        );
    }
    println!("{}", format_runtime_status_summary(&server.status().await));
    tokio::signal::ctrl_c()
        .await
        .map_err(|error| RuntimeError::Config(format!("failed to wait for ctrl-c: {error}")))?;
    server.shutdown().await
}
