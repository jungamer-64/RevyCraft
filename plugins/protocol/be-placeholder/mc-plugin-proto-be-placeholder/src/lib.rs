#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_be_placeholder::BePlaceholderAdapter;
use mc_plugin_sdk_rust::ProtocolCapability;

declare_protocol_plugin!(
    BePlaceholderProtocolPlugin,
    BePlaceholderAdapter,
    "be-placeholder",
    "Bedrock Placeholder Protocol Plugin",
    &[
        ProtocolCapability::RuntimeReload,
        ProtocolCapability::Bedrock,
    ],
);
