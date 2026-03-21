#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_be_placeholder::BePlaceholderAdapter;

declare_protocol_plugin!(
    BePlaceholderProtocolPlugin,
    BePlaceholderAdapter,
    "be-placeholder",
    "Bedrock Placeholder Protocol Plugin",
    &["protocol.be", "protocol.be.placeholder", "runtime.reload.protocol"],
    &["runtime.reload.protocol"],
);
