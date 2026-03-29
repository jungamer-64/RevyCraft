#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_be_924::Bedrock924Adapter;
use mc_plugin_sdk_rust::ProtocolCapability;

declare_protocol_plugin!(
    Bedrock924ProtocolPlugin,
    Bedrock924Adapter,
    "be-924",
    "Bedrock 26.3 (Protocol 924) Plugin",
    &[
        ProtocolCapability::RuntimeReload,
        ProtocolCapability::Bedrock,
        ProtocolCapability::Bedrock924,
    ],
);
