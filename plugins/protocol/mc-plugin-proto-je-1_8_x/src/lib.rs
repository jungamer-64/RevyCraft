#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use mc_plugin_sdk_rust::protocol::{delegate_protocol_adapter, export_protocol_plugin};
use mc_proto_je_1_8_x::Je18xAdapter;

#[derive(Default)]
pub struct Je18xProtocolPlugin {
    adapter: Je18xAdapter,
}

delegate_protocol_adapter!(Je18xProtocolPlugin, adapter, {
    let mut capabilities = mc_core::CapabilitySet::new();
    let _ = capabilities.insert("protocol.je");
    let _ = capabilities.insert("protocol.je.1_8_x");
    let _ = capabilities.insert("runtime.reload.protocol");
    if let Some(build_tag) = option_env!("REVY_PLUGIN_BUILD_TAG") {
        let _ = capabilities.insert(format!("build-tag:{build_tag}"));
    }
    capabilities
});

const MANIFEST: StaticPluginManifest = StaticPluginManifest::protocol_with_capabilities(
    "je-1_8_x",
    "JE 1.8.x Protocol Plugin",
    &["runtime.reload.protocol"],
);

export_protocol_plugin!(Je18xProtocolPlugin, MANIFEST);
