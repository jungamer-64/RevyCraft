use server_runtime::{RuntimeError, RuntimeRegistries, ServerConfig, spawn_server};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), RuntimeError> {
    let config = ServerConfig::from_properties(Path::new("server.properties"))?;
    let server = spawn_server(config, RuntimeRegistries::with_je_and_be_placeholder()).await?;
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
