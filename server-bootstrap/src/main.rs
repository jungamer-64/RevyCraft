use mc_proto_je_1_7_10::{JE_1_7_10_STORAGE_PROFILE_ID, Je1710StorageAdapter};
use server_runtime::{
    RuntimeError, RuntimeRegistries, ServerConfig, plugin_host_from_config, spawn_server,
};
use std::path::Path;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), RuntimeError> {
    let config = ServerConfig::from_properties(Path::new("server.properties"))?;
    let mut registries = RuntimeRegistries::new();
    registries
        .register_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID, Arc::new(Je1710StorageAdapter));

    let plugin_host = plugin_host_from_config(&config)?.ok_or_else(|| {
        RuntimeError::Config(format!(
            "no protocol plugins discovered under `{}`",
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
