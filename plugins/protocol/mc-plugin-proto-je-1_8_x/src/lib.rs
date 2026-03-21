#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_je_1_8_x::Je18xAdapter;

declare_protocol_plugin!(
    Je18xProtocolPlugin,
    Je18xAdapter,
    "je-1_8_x",
    "JE 1.8.x Protocol Plugin",
    &[
        "protocol.je",
        "protocol.je.1_8_x",
        "runtime.reload.protocol"
    ],
    &["runtime.reload.protocol"],
);
