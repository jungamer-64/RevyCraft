#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_je_1_12_2::Je1122Adapter;

declare_protocol_plugin!(
    Je1122ProtocolPlugin,
    Je1122Adapter,
    "je-1_12_2",
    "JE 1.12.2 Protocol Plugin",
    &[
        "protocol.je",
        "protocol.je.1_12_2",
        "runtime.reload.protocol"
    ],
    &["runtime.reload.protocol"],
);
