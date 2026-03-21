#![allow(clippy::multiple_crate_versions)]
use mc_core::CapabilitySet;
use mc_plugin_api::codec::storage::StorageDescriptor;
use mc_plugin_sdk_rust::capabilities::{
    build_tag_contains,
    capability_set as build_capability_set,
};
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use mc_plugin_sdk_rust::storage::RustStoragePlugin;
use mc_proto_common::{StorageAdapter, StorageError};
use mc_proto_je_1_7_10::{JE_1_7_10_STORAGE_PROFILE_ID, Je1710StorageAdapter};
use std::path::Path;

pub const JE_1_7_10_STORAGE_PLUGIN_ID: &str = "storage-je-anvil-1_7_10";

#[derive(Default)]
pub struct Je1710StoragePlugin {
    adapter: Je1710StorageAdapter,
}

impl RustStoragePlugin for Je1710StoragePlugin {
    fn descriptor(&self) -> StorageDescriptor {
        StorageDescriptor {
            storage_profile: JE_1_7_10_STORAGE_PROFILE_ID.to_string(),
        }
    }

    fn capability_set(&self) -> CapabilitySet {
        build_capability_set(&[
            "storage.je-anvil",
            "storage.profile.je-anvil-1_7_10",
            "runtime.reload.storage",
        ])
    }

    fn load_snapshot(
        &self,
        world_dir: &Path,
    ) -> Result<Option<mc_core::WorldSnapshot>, StorageError> {
        self.adapter.load_snapshot(world_dir)
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &mc_core::WorldSnapshot,
    ) -> Result<(), StorageError> {
        self.adapter.save_snapshot(world_dir, snapshot)
    }

    fn import_runtime_state(
        &self,
        world_dir: &Path,
        snapshot: &mc_core::WorldSnapshot,
    ) -> Result<(), StorageError> {
        if build_tag_contains("reload-fail") {
            return Err(StorageError::Plugin(
                "storage plugin refused runtime state import".to_string(),
            ));
        }
        self.adapter.save_snapshot(world_dir, snapshot)
    }
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::storage(
    JE_1_7_10_STORAGE_PLUGIN_ID,
    "JE 1.7.10 Anvil Storage Plugin",
    &["storage.profile:je-anvil-1_7_10", "runtime.reload.storage"],
);

export_plugin!(storage, Je1710StoragePlugin, MANIFEST);
