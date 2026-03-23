use super::*;
use mc_core::StorageCapabilitySet;

pub trait RustStoragePlugin: Send + Sync + 'static {
    fn descriptor(&self) -> StorageDescriptor;

    fn capability_set(&self) -> StorageCapabilitySet {
        StorageCapabilitySet::default()
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
