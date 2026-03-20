use super::*;

pub use crate::export_storage_plugin;

pub trait RustStoragePlugin: Send + Sync + 'static {
    fn descriptor(&self) -> StorageDescriptor;

    fn capability_set(&self) -> CapabilitySet {
        CapabilitySet::new()
    }

    /// Loads the current world snapshot for the provided world directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the snapshot cannot be read or decoded.
    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError>;

    /// Persists the provided world snapshot for the provided world directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the snapshot cannot be written.
    fn save_snapshot(&self, world_dir: &Path, snapshot: &WorldSnapshot)
    -> Result<(), StorageError>;

    /// Exports runtime-specific state for later re-import.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime state cannot be read or serialized.
    fn export_runtime_state(
        &self,
        world_dir: &Path,
    ) -> Result<Option<WorldSnapshot>, StorageError> {
        self.load_snapshot(world_dir)
    }

    /// Imports runtime-specific state that was previously exported.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime state cannot be applied.
    fn import_runtime_state(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        self.save_snapshot(world_dir, snapshot)
    }
}

#[doc(hidden)]
pub fn handle_storage_request<P: RustStoragePlugin>(
    plugin: &P,
    request: StorageRequest,
) -> Result<StorageResponse, String> {
    match request {
        StorageRequest::Describe => Ok(StorageResponse::Descriptor(plugin.descriptor())),
        StorageRequest::CapabilitySet => {
            Ok(StorageResponse::CapabilitySet(plugin.capability_set()))
        }
        StorageRequest::LoadSnapshot { world_dir } => plugin
            .load_snapshot(Path::new(&world_dir))
            .map(StorageResponse::Snapshot)
            .map_err(|error| error.to_string()),
        StorageRequest::SaveSnapshot {
            world_dir,
            snapshot,
        } => plugin
            .save_snapshot(Path::new(&world_dir), &snapshot)
            .map(|()| StorageResponse::Empty)
            .map_err(|error| error.to_string()),
        StorageRequest::ExportRuntimeState { world_dir } => plugin
            .export_runtime_state(Path::new(&world_dir))
            .map(StorageResponse::Snapshot)
            .map_err(|error| error.to_string()),
        StorageRequest::ImportRuntimeState {
            world_dir,
            snapshot,
        } => plugin
            .import_runtime_state(Path::new(&world_dir), &snapshot)
            .map(|()| StorageResponse::Empty)
            .map_err(|error| error.to_string()),
    }
}
