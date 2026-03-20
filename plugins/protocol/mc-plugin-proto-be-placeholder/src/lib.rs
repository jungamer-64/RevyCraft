#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use mc_plugin_sdk_rust::protocol::{delegate_protocol_adapter, export_protocol_plugin};
use mc_proto_be_placeholder::BePlaceholderAdapter;

#[derive(Default)]
pub struct BePlaceholderProtocolPlugin {
    adapter: BePlaceholderAdapter,
}

delegate_protocol_adapter!(BePlaceholderProtocolPlugin, adapter, {
    let mut capabilities = mc_core::CapabilitySet::new();
    let _ = capabilities.insert("protocol.be");
    let _ = capabilities.insert("protocol.be.placeholder");
    let _ = capabilities.insert("runtime.reload.protocol");
    if let Some(build_tag) = option_env!("REVY_PLUGIN_BUILD_TAG") {
        let _ = capabilities.insert(format!("build-tag:{build_tag}"));
    }
    capabilities
});

const MANIFEST: StaticPluginManifest = StaticPluginManifest::protocol_with_capabilities(
    "be-placeholder",
    "Bedrock Placeholder Protocol Plugin",
    &["runtime.reload.protocol"],
);

export_protocol_plugin!(BePlaceholderProtocolPlugin, MANIFEST);
