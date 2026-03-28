use super::{
    current_artifact_key, with_current_gameplay_transaction, with_gameplay_transaction_and_limits,
};
use crate::PluginHostError as RuntimeError;
use crate::config::{BootstrapConfig, PluginBufferLimits, RuntimeSelectionConfig};
use crate::host::{PluginAbiRange, PluginFailureAction, plugin_host_from_config};
use crate::runtime::{ProtocolReloadSession, RuntimeReloadContext};
use crate::test_support::{
    InProcessAdminSurfacePlugin, InProcessAuthPlugin, InProcessGameplayPlugin,
    InProcessProtocolPlugin, InProcessStoragePlugin, PluginFailureMatrix, TestPluginHost,
    TestPluginHostBuilder,
};
use mc_core::{ConnectionId, CoreConfig, EntityId, PlayerId, ServerCore};
use mc_plugin_admin_ui_console::in_process_plugin_entrypoints as console_admin_surface_entrypoints;
use mc_plugin_api::abi::{
    CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, PluginAbiVersion, PluginKind, Utf8Slice,
};
use mc_plugin_api::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_api::manifest::PluginManifestV1;
use mc_plugin_auth_offline::in_process_plugin_entrypoints as offline_auth_entrypoints;
use mc_plugin_gameplay_canonical::in_process_plugin_entrypoints as canonical_gameplay_entrypoints;
use mc_plugin_gameplay_readonly::in_process_plugin_entrypoints as readonly_gameplay_entrypoints;
use mc_plugin_proto_be_924::in_process_plugin_entrypoints as be_26_3_entrypoints;
use mc_plugin_proto_be_placeholder::in_process_plugin_entrypoints as be_placeholder_entrypoints;
use mc_plugin_proto_je_5::in_process_plugin_entrypoints as in_process_protocol_entrypoints;
use mc_plugin_proto_je_47::in_process_plugin_entrypoints as je_1_8_x_entrypoints;
use mc_plugin_proto_je_340::in_process_plugin_entrypoints as je_1_12_2_entrypoints;
use mc_plugin_storage_je_anvil_1_7_10::in_process_plugin_entrypoints as storage_entrypoints;
use mc_plugin_test_support::PackagedPluginHarness;
use mc_proto_common::{ConnectionPhase, Edition, PacketWriter, TransportKind, WireFormatKind};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

mod admin_surface;
mod discovery;
mod failure_policy;
mod gameplay_query;
#[cfg(target_os = "linux")]
mod packaged_reload;
mod profiles;
mod support;
mod test_plugins;

use self::support::*;
use self::test_plugins::*;

fn runtime_selection_config() -> RuntimeSelectionConfig {
    RuntimeSelectionConfig {
        admin_surfaces: Vec::new(),
        ..RuntimeSelectionConfig::default()
    }
}

fn bootstrap_config_with_plugins_dir(plugins_dir: PathBuf) -> BootstrapConfig {
    BootstrapConfig {
        plugins_dir,
        ..BootstrapConfig::default()
    }
}

fn tempdir() -> std::io::Result<tempfile::TempDir> {
    let base_dir = workspace_test_temp_root().join("mc-plugin-host");
    fs::create_dir_all(&base_dir)?;
    tempfile::Builder::new()
        .prefix("mc-plugin-host-")
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
        "mc-plugin-host tests should run under the workspace root: {}",
        manifest_dir.display()
    );
}
