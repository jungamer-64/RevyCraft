use super::{
    ReloadableRunningServer, ServerBuilder, TopologyStatusState, format_runtime_status_summary,
};
use crate::RuntimeError;
use crate::config::{BEDROCK_OFFLINE_AUTH_PROFILE_ID, LevelType, ServerConfig, ServerConfigSource};
use crate::transport::{MinecraftStreamCipher, build_listener_plans, default_wire_codec};
use bytes::BytesMut;
use mc_plugin_auth_offline::{
    OFFLINE_AUTH_PROFILE_ID, in_process_auth_entrypoints as offline_auth_entrypoints,
};
use mc_plugin_auth_online_stub::{
    ONLINE_STUB_AUTH_PLUGIN_ID, ONLINE_STUB_AUTH_PROFILE_ID,
    in_process_auth_entrypoints as online_stub_auth_entrypoints,
};
use mc_plugin_gameplay_canonical::in_process_gameplay_entrypoints as canonical_gameplay_entrypoints;
use mc_plugin_gameplay_readonly::in_process_gameplay_entrypoints as readonly_gameplay_entrypoints;
use mc_plugin_host::registry::{LoadedPluginSet, ProtocolRegistry};
use mc_plugin_host_test_support::raw::{
    InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin, InProcessStoragePlugin,
};
use mc_plugin_host_test_support::{
    PluginAbiRange, PluginFailureAction, PluginFailureMatrix, TestPluginHost, TestPluginHostBuilder,
};
use mc_plugin_proto_be_26_3::in_process_protocol_entrypoints as be_26_3_entrypoints;
use mc_plugin_proto_be_placeholder::in_process_protocol_entrypoints as be_placeholder_entrypoints;
use mc_plugin_proto_je_1_7_10::in_process_protocol_entrypoints as je_1_7_10_entrypoints;
use mc_plugin_proto_je_1_8_x::in_process_protocol_entrypoints as je_1_8_x_entrypoints;
use mc_plugin_proto_je_1_12_2::in_process_protocol_entrypoints as je_1_12_2_entrypoints;
use mc_plugin_storage_je_anvil_1_7_10::in_process_storage_entrypoints as storage_entrypoints;
use mc_plugin_test_support::PackagedPluginHarness;
use mc_proto_be_26_3::BE_26_3_ADAPTER_ID;
use mc_proto_be_placeholder::BE_PLACEHOLDER_ADAPTER_ID;
use mc_proto_common::{
    Edition, MinecraftWireCodec, PacketReader, PacketWriter, ProtocolError, TransportKind,
    WireCodec, WireFormatKind,
};
use mc_proto_je_1_7_10::{JE_1_7_10_ADAPTER_ID, JE_1_7_10_STORAGE_PROFILE_ID};
use mc_proto_je_1_8_x::JE_1_8_X_ADAPTER_ID;
use mc_proto_je_1_12_2::JE_1_12_2_ADAPTER_ID;
use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;

mod builders;
mod registry;
mod support;

pub(crate) use self::support::*;

mod auth;
mod config_props;
mod connectivity;
mod gameplay;
mod guardrails;
mod multiversion;

#[cfg(target_os = "linux")]
mod reload;
