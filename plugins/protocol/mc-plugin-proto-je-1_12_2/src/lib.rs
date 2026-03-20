#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use mc_plugin_sdk_rust::protocol::{delegate_protocol_adapter, export_protocol_plugin};
use mc_proto_je_1_12_2::Je1122Adapter;

#[derive(Default)]
pub struct Je1122ProtocolPlugin {
    adapter: Je1122Adapter,
}

delegate_protocol_adapter!(Je1122ProtocolPlugin, adapter, {
    let mut capabilities = mc_core::CapabilitySet::new();
    let _ = capabilities.insert("protocol.je");
    let _ = capabilities.insert("protocol.je.1_12_2");
    let _ = capabilities.insert("runtime.reload.protocol");
    if let Some(build_tag) = option_env!("REVY_PLUGIN_BUILD_TAG") {
        let _ = capabilities.insert(format!("build-tag:{build_tag}"));
    }
    capabilities
});

const MANIFEST: StaticPluginManifest = StaticPluginManifest::protocol_with_capabilities(
    "je-1_12_2",
    "JE 1.12.2 Protocol Plugin",
    &["runtime.reload.protocol"],
);

export_protocol_plugin!(Je1122ProtocolPlugin, MANIFEST);
