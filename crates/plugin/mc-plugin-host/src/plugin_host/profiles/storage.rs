use super::{
    Arc, CapabilitySet, Path, PluginGenerationId, RwLock, StorageAdapter, StorageError,
    StorageProfileHandle, StorageRequest, StorageResponse, WorldSnapshot,
};

pub(crate) struct HotSwappableStorageProfile {
    plugin_id: String,
    pub(crate) generation: RwLock<Arc<super::StorageGeneration>>,
    pub(crate) reload_gate: RwLock<()>,
}

impl HotSwappableStorageProfile {
    pub(crate) const fn new(plugin_id: String, generation: Arc<super::StorageGeneration>) -> Self {
        Self {
            plugin_id,
            generation: RwLock::new(generation),
            reload_gate: RwLock::new(()),
        }
    }

    pub(crate) fn current_generation(&self) -> Result<Arc<super::StorageGeneration>, StorageError> {
        Ok(self
            .generation
            .read()
            .expect("storage generation lock should not be poisoned")
            .clone())
    }

    pub(crate) fn swap_generation(&self, generation: Arc<super::StorageGeneration>) {
        *self
            .generation
            .write()
            .expect("storage generation lock should not be poisoned") = generation;
    }

    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    fn capability_set(&self) -> CapabilitySet {
        self.current_generation()
            .map(|generation| generation.capabilities.clone())
            .unwrap_or_default()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.current_generation()
            .ok()
            .map(|generation| generation.generation_id)
    }

    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("storage reload gate should not be poisoned");
        let generation = self.current_generation()?;
        match generation.invoke(&StorageRequest::LoadSnapshot {
            world_dir: world_dir.display().to_string(),
        })? {
            StorageResponse::Snapshot(snapshot) => Ok(snapshot),
            other => Err(StorageError::Plugin(format!(
                "unexpected storage load_snapshot payload: {other:?}"
            ))),
        }
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("storage reload gate should not be poisoned");
        let generation = self.current_generation()?;
        match generation.invoke(&StorageRequest::SaveSnapshot {
            world_dir: world_dir.display().to_string(),
            snapshot: snapshot.clone(),
        })? {
            StorageResponse::Empty => Ok(()),
            other => Err(StorageError::Plugin(format!(
                "unexpected storage save_snapshot payload: {other:?}"
            ))),
        }
    }
}

impl StorageAdapter for HotSwappableStorageProfile {
    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        Self::load_snapshot(self, world_dir)
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        Self::save_snapshot(self, world_dir, snapshot)
    }
}

impl StorageProfileHandle for HotSwappableStorageProfile {
    fn plugin_id(&self) -> &str {
        Self::plugin_id(self)
    }

    fn capability_set(&self) -> CapabilitySet {
        Self::capability_set(self)
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Self::plugin_generation_id(self)
    }

    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        Self::load_snapshot(self, world_dir)
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        Self::save_snapshot(self, world_dir, snapshot)
    }
}
