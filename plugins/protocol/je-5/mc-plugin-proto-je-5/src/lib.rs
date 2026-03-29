#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_je_5::Je5Adapter;
use mc_plugin_sdk_rust::ProtocolCapability;

declare_protocol_plugin!(
    Je5ProtocolPlugin,
    Je5Adapter,
    "je-5",
    "JE 1.7.10 (Protocol 5) Plugin",
    &[
        ProtocolCapability::RuntimeReload,
        ProtocolCapability::Je,
        ProtocolCapability::Je5,
    ],
);
