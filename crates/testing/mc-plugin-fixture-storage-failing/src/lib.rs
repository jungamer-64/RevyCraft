use mc_plugin_contract::codec::storage::StorageDescriptor;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use mc_plugin_sdk_rust::storage::RustStoragePlugin;
use mc_plugin_sdk_rust::{StorageCapability, StorageCapabilitySet, WorldSnapshot};
use mc_storage_common::StorageError;
use std::path::Path;

pub const PLUGIN_ID: &str = "storage-failing-runtime";
pub const PROFILE_ID: &str = "failing-storage";

#[derive(Default)]
pub struct FailingStoragePlugin;

impl RustStoragePlugin for FailingStoragePlugin {
    fn descriptor(&self) -> StorageDescriptor {
        StorageDescriptor {
            storage_profile: PROFILE_ID.into(),
        }
    }

    fn capability_set(&self) -> StorageCapabilitySet {
        let mut capabilities = StorageCapabilitySet::new();
        let _ = capabilities.insert(StorageCapability::RuntimeReload);
        capabilities
    }

    fn load_snapshot(&self, _world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        Ok(None)
    }

    fn save_snapshot(
        &self,
        _world_dir: &Path,
        _snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        Err(StorageError::Plugin("storage runtime failure".to_string()))
    }
}

const MANIFEST: StaticPluginManifest =
    StaticPluginManifest::storage(PLUGIN_ID, "Failing Storage Fixture", PROFILE_ID);

export_plugin!(storage, FailingStoragePlugin, MANIFEST);
