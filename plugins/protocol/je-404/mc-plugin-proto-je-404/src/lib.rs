#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_je_404::Je404Adapter;
use mc_plugin_sdk_rust::ProtocolCapability;

declare_protocol_plugin!(
    Je404ProtocolPlugin,
    Je404Adapter,
    "je-404",
    "JE 1.13.2 (Protocol 404) Plugin",
    &[
        ProtocolCapability::RuntimeReload,
        ProtocolCapability::Je,
        ProtocolCapability::Je404,
    ],
);
