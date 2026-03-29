#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::ProtocolCapability;
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_je_775::Je775Adapter;

declare_protocol_plugin!(
    Je775ProtocolPlugin,
    Je775Adapter,
    "je-775",
    "JE 26.1 (Protocol 775) Plugin",
    &[
        ProtocolCapability::RuntimeReload,
        ProtocolCapability::Je,
        ProtocolCapability::Je775,
    ],
);
