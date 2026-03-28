use super::bootstrap::boot_server;
use super::{GenerationStatusState, format_runtime_status_summary};
use crate::RuntimeError;
use crate::config::{BEDROCK_OFFLINE_AUTH_PROFILE_ID, LevelType, ServerConfig, ServerConfigSource};
use crate::transport::{MinecraftStreamCipher, build_listener_plans, default_wire_codec};
use bytes::BytesMut;
use mc_plugin_auth_offline::{
    OFFLINE_AUTH_PROFILE_ID, in_process_plugin_entrypoints as offline_auth_entrypoints,
};
use mc_plugin_auth_online_stub::{
    ONLINE_STUB_AUTH_PLUGIN_ID, ONLINE_STUB_AUTH_PROFILE_ID,
    in_process_plugin_entrypoints as online_stub_auth_entrypoints,
};
use mc_plugin_gameplay_canonical::in_process_plugin_entrypoints as canonical_gameplay_entrypoints;
use mc_plugin_gameplay_readonly::in_process_plugin_entrypoints as readonly_gameplay_entrypoints;
use mc_plugin_host::registry::{LoadedPluginSet, ProtocolRegistry};
use mc_plugin_host_test_support::raw::{
    InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin, InProcessStoragePlugin,
};
use mc_plugin_host_test_support::{
    PluginAbiRange, PluginFailureAction, PluginFailureMatrix, TestPluginHost, TestPluginHostBuilder,
};
use mc_plugin_proto_be_924::in_process_plugin_entrypoints as be_26_3_entrypoints;
use mc_plugin_proto_be_placeholder::in_process_plugin_entrypoints as be_placeholder_entrypoints;
use mc_plugin_proto_je_5::in_process_plugin_entrypoints as je_1_7_10_entrypoints;
use mc_plugin_proto_je_47::in_process_plugin_entrypoints as je_1_8_x_entrypoints;
use mc_plugin_proto_je_340::in_process_plugin_entrypoints as je_1_12_2_entrypoints;
use mc_plugin_proto_je_404::in_process_plugin_entrypoints as je_1_13_2_entrypoints;
use mc_plugin_storage_je_anvil_1_7_10::in_process_plugin_entrypoints as storage_entrypoints;
use mc_plugin_storage_je_anvil_1_18_2::{
    JE_1_18_2_STORAGE_PLUGIN_ID, JE_1_18_2_STORAGE_PROFILE_ID,
};
use mc_plugin_test_support::PackagedPluginHarness;
use mc_proto_be_924::BE_924_ADAPTER_ID;
use mc_proto_be_placeholder::BE_PLACEHOLDER_ADAPTER_ID;
use mc_proto_common::{
    Edition, MinecraftWireCodec, PacketReader, PacketWriter, ProtocolError, TransportKind,
    WireCodec, WireFormatKind,
};
use mc_proto_je_5::{JE_1_7_10_STORAGE_PROFILE_ID, JE_5_ADAPTER_ID};
use mc_proto_je_47::JE_47_ADAPTER_ID;
use mc_proto_je_340::JE_340_ADAPTER_ID;
use mc_proto_je_404::JE_404_ADAPTER_ID;
use mc_proto_test_support::{TestJavaPacket, TestJavaProtocol, TestJavaProtocolError};
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
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
mod selection;

#[cfg(target_os = "linux")]
mod reload;

impl From<TestJavaProtocolError> for RuntimeError {
    fn from(error: TestJavaProtocolError) -> Self {
        Self::Config(error.to_string())
    }
}

fn tempdir() -> std::io::Result<tempfile::TempDir> {
    let base_dir = workspace_test_temp_root().join("revy-server-runtime");
    fs::create_dir_all(&base_dir)?;
    tempfile::Builder::new()
        .prefix("revy-server-runtime-")
        .tempdir_in(base_dir)
}

fn workspace_test_temp_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for ancestor in manifest_dir.ancestors() {
        let manifest = ancestor.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&manifest) else {
            continue;
        };
        if contents.contains("[workspace]") {
            return ancestor.join("target").join("test-tmp");
        }
    }
    panic!(
        "revy-server-runtime tests should run under the workspace root: {}",
        manifest_dir.display()
    );
}
