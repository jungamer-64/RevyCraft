use server_runtime::{RuntimeError, ServerConfig, VersionRegistry, spawn_server};
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), RuntimeError> {
    let config = ServerConfig::from_properties(Path::new("server.properties"))?;
    let server = spawn_server(config, VersionRegistry::with_je_1_7_10()).await?;
    println!("server listening on {}", server.local_addr());
    tokio::signal::ctrl_c()
        .await
        .map_err(|error| RuntimeError::Config(format!("failed to wait for ctrl-c: {error}")))?;
    server.shutdown().await
}
