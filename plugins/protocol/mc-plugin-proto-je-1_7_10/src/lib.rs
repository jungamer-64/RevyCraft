#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_je_1_7_10::Je1710Adapter;

declare_protocol_plugin!(
    Je1710ProtocolPlugin,
    Je1710Adapter,
    "je-1_7_10",
    "JE 1.7.10 Protocol Plugin",
    &[
        "protocol.je",
        "protocol.je.1_7_10",
        "runtime.reload.protocol"
    ],
    &["runtime.reload.protocol"],
);
