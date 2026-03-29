use revy_voxel_semantic::WorldSnapshot;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid data: {0}")]
    InvalidData(String),
    #[error("plugin error: {0}")]
    Plugin(String),
}

pub trait StorageAdapter: Send + Sync {
    /// # Errors
    ///
    /// Returns [`StorageError`] when the snapshot backend cannot be read or
    /// when persisted data is invalid.
    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError>;

    /// # Errors
    ///
    /// Returns [`StorageError`] when the snapshot cannot be serialized or
    /// written to the backing store.
    fn save_snapshot(&self, world_dir: &Path, snapshot: &WorldSnapshot)
    -> Result<(), StorageError>;
}
