use mc_proto_be_placeholder::BePlaceholderAdapter;
use mc_proto_je_1_12_2::Je1122Adapter;
use mc_proto_je_1_7_10::{
    JE_1_7_10_STORAGE_PROFILE_ID, Je1710Adapter, Je1710StorageAdapter,
};
use mc_proto_je_1_8_x::Je18xAdapter;
use server_runtime::{
    RuntimeError, RuntimeRegistries, ServerConfig, plugin_host_from_config, spawn_server,
};
use std::path::Path;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), RuntimeError> {
    let config = ServerConfig::from_properties(Path::new("server.properties"))?;
    let mut registries = RuntimeRegistries::new();
    registries.register_storage_profile(JE_1_7_10_STORAGE_PROFILE_ID, Arc::new(Je1710StorageAdapter));

    let mut loaded_protocol_plugin = false;
    if let Some(plugin_host) = plugin_host_from_config(&config)? {
        plugin_host.load_into_registries(&mut registries)?;
        loaded_protocol_plugin = true;
    }

    if !loaded_protocol_plugin {
        let adapter = Arc::new(Je1710Adapter::new());
        registries.register_adapter(adapter.clone());
        registries.register_probe(adapter);
    }

    let adapter = Arc::new(Je18xAdapter::new());
    registries.register_adapter(adapter.clone());
    registries.register_probe(adapter);

    let adapter = Arc::new(Je1122Adapter::new());
    registries.register_adapter(adapter.clone());
    registries.register_probe(adapter);

    let adapter = Arc::new(BePlaceholderAdapter::new());
    registries.register_adapter(adapter.clone());
    registries.register_probe(adapter);

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
