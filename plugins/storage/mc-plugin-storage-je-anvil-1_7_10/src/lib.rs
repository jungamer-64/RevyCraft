#![allow(clippy::multiple_crate_versions)]
use mc_core::CapabilitySet;
use mc_plugin_api::StorageDescriptor;
use mc_plugin_sdk_rust::{RustStoragePlugin, StaticPluginManifest, export_storage_plugin};
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
        let mut capabilities = CapabilitySet::new();
        let _ = capabilities.insert("storage.je-anvil");
        let _ = capabilities.insert("storage.profile.je-anvil-1_7_10");
        let _ = capabilities.insert("runtime.reload.storage");
        if let Some(build_tag) = option_env!("REVY_PLUGIN_BUILD_TAG") {
            let _ = capabilities.insert(format!("build-tag:{build_tag}"));
        }
        capabilities
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
        if option_env!("REVY_PLUGIN_BUILD_TAG").is_some_and(|tag| tag.contains("reload-fail")) {
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

export_storage_plugin!(Je1710StoragePlugin, MANIFEST);
