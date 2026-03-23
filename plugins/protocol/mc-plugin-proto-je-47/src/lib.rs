#![allow(clippy::multiple_crate_versions)]
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_je_47::Je47Adapter;

declare_protocol_plugin!(
    Je47ProtocolPlugin,
    Je47Adapter,
    "je-47",
    "JE 1.8.x (Protocol 47) Plugin",
    &[
        "protocol.je",
        "protocol.je.47",
        "runtime.reload.protocol"
    ],
    &["runtime.reload.protocol"],
);
