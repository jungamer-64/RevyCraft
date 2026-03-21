use super::{current_artifact_key, with_current_gameplay_query, with_gameplay_query};
use crate::PluginHostError as RuntimeError;
use crate::config::ServerConfig;
use crate::host::{PluginAbiRange, PluginFailureAction, plugin_host_from_config};
use crate::runtime::{ProtocolReloadSession, RuntimeReloadContext};
use crate::test_support::{
    InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin, InProcessStoragePlugin,
    PluginFailureMatrix, TestPluginHost, TestPluginHostBuilder,
};
use mc_core::{
    BlockPos, BlockState, ConnectionId, CoreConfig, DimensionId, EntityId, GameplayQuery, PlayerId,
    ServerCore, WorldMeta,
};
use mc_plugin_api::abi::{
    CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, PluginAbiVersion, PluginKind, Utf8Slice,
};
use mc_plugin_api::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_api::manifest::PluginManifestV1;
use mc_plugin_auth_offline::in_process_auth_entrypoints as offline_auth_entrypoints;
use mc_plugin_gameplay_canonical::in_process_gameplay_entrypoints as canonical_gameplay_entrypoints;
use mc_plugin_gameplay_readonly::in_process_gameplay_entrypoints as readonly_gameplay_entrypoints;
use mc_plugin_proto_be_26_3::in_process_protocol_entrypoints as be_26_3_entrypoints;
use mc_plugin_proto_be_placeholder::in_process_protocol_entrypoints as be_placeholder_entrypoints;
use mc_plugin_proto_je_1_7_10::in_process_protocol_entrypoints;
use mc_plugin_proto_je_1_8_x::in_process_protocol_entrypoints as je_1_8_x_entrypoints;
use mc_plugin_proto_je_1_12_2::in_process_protocol_entrypoints as je_1_12_2_entrypoints;
use mc_plugin_storage_je_anvil_1_7_10::in_process_storage_entrypoints as storage_entrypoints;
use mc_plugin_test_support::PackagedPluginHarness;
use mc_proto_common::{ConnectionPhase, Edition, PacketWriter, TransportKind, WireFormatKind};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::tempdir;
use uuid::Uuid;

#[path = "tests/discovery.rs"]
mod discovery;
#[path = "tests/failure_policy.rs"]
mod failure_policy;
#[path = "tests/gameplay_query.rs"]
mod gameplay_query;
#[cfg(target_os = "linux")]
#[path = "tests/packaged_reload.rs"]
mod packaged_reload;
#[path = "tests/profiles.rs"]
mod profiles;
#[path = "tests/support.rs"]
mod support;
#[path = "tests/test_plugins.rs"]
mod test_plugins;

use self::support::*;
use self::test_plugins::*;
