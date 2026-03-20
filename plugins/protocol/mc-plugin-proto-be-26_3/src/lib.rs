#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::{StaticPluginManifest, delegate_protocol_adapter, export_protocol_plugin};
use mc_proto_be_26_3::Bedrock263Adapter;

#[derive(Default)]
pub struct Bedrock263ProtocolPlugin {
    adapter: Bedrock263Adapter,
}

delegate_protocol_adapter!(Bedrock263ProtocolPlugin, adapter, {
    let mut capabilities = mc_core::CapabilitySet::new();
    let _ = capabilities.insert("protocol.bedrock");
    let _ = capabilities.insert("protocol.bedrock.26_3");
    let _ = capabilities.insert("runtime.reload.protocol");
    if let Some(build_tag) = option_env!("REVY_PLUGIN_BUILD_TAG") {
        let _ = capabilities.insert(format!("build-tag:{build_tag}"));
    }
    capabilities
});

const MANIFEST: StaticPluginManifest = StaticPluginManifest::protocol_with_capabilities(
    "be-26_3",
    "Bedrock 26.3 Protocol Plugin",
    &["runtime.reload.protocol"],
);

export_protocol_plugin!(Bedrock263ProtocolPlugin, MANIFEST);
