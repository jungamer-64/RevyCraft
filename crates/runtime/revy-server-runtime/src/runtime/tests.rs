use super::bootstrap::boot_server;
use super::{GenerationStatusState, format_runtime_status_summary};
use crate::PluginFailureAction;
use crate::RuntimeError;
use crate::config::{BEDROCK_OFFLINE_AUTH_PROFILE_ID, LevelType, ServerConfig, ServerConfigSource};
use crate::transport::{MinecraftStreamCipher, build_listener_plans, default_wire_codec};
use bytes::BytesMut;
use mc_plugin_host::host::PluginHost;
use mc_plugin_host::registry::{LoadedPluginSet, ProtocolRegistry};
use mc_plugin_test_support::PackagedPluginHarness;
use mc_proto_be_924::BE_924_ADAPTER_ID;
use mc_proto_be_placeholder::BE_PLACEHOLDER_ADAPTER_ID;
use mc_proto_common::{
    Edition, MinecraftWireCodec, PacketReader, PacketWriter, ProtocolError, TransportKind,
    WireCodec, WireFormatKind,
};
use mc_proto_je_5::JE_5_ADAPTER_ID;
use mc_proto_je_47::JE_47_ADAPTER_ID;
use mc_proto_je_340::JE_340_ADAPTER_ID;
use mc_proto_je_404::JE_404_ADAPTER_ID;
use mc_proto_je_775::JE_775_ADAPTER_ID;
use mc_proto_test_support::{TestJavaPacket, TestJavaProtocol, TestJavaProtocolError};
use mc_storage_je_anvil_1_7_10::JE_1_7_10_STORAGE_PROFILE_ID;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
mod executable;
mod gameplay;
mod guardrails;
mod multiversion;
mod selection;

mod reload;

const OFFLINE_AUTH_PROFILE_ID: &str = "offline-v1";
const ONLINE_STUB_AUTH_PLUGIN_ID: &str = "auth-online-stub";
const ONLINE_STUB_AUTH_PROFILE_ID: &str = "mojang-online-v1";
const JE_1_18_2_STORAGE_PLUGIN_ID: &str = "storage-je-anvil-1_18_2";
const JE_1_18_2_STORAGE_PROFILE_ID: &str = "je-anvil-1_18_2";
const JE_26_1_STORAGE_PLUGIN_ID: &str = "storage-je-anvil-26_1";
const JE_26_1_STORAGE_PROFILE_ID: &str = "je-anvil-26_1";

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
            return ancestor
                .join(std::env::var_os("CARGO_TARGET_DIR").unwrap_or_else(|| "target".into()))
                .join("test-tmp");
        }
    }
    panic!(
        "revy-server-runtime tests should run under the workspace root: {}",
        manifest_dir.display()
    );
}
