#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_be_26_3::Bedrock263Adapter;

declare_protocol_plugin!(
    Bedrock263ProtocolPlugin,
    Bedrock263Adapter,
    "be-26_3",
    "Bedrock 26.3 Protocol Plugin",
    &[
        "protocol.bedrock",
        "protocol.bedrock.26_3",
        "runtime.reload.protocol",
    ],
    &["runtime.reload.protocol"],
);
