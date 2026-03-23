#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_je_340::Je340Adapter;

declare_protocol_plugin!(
    Je340ProtocolPlugin,
    Je340Adapter,
    "je-340",
    "JE 1.12.2 (Protocol 340) Plugin",
    &["protocol.je", "protocol.je.340", "runtime.reload.protocol"],
    &["runtime.reload.protocol"],
);
