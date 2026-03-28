use super::{
    Arc, Path, PluginGenerationId, ReloadableGenerationSlot, StorageAdapter, StorageCapabilitySet,
    StorageError, StorageProfileHandle, StorageRequest, StorageResponse, WorldSnapshot,
};

pub(crate) struct HotSwappableStorageProfile {
    plugin_id: String,
    generation: ReloadableGenerationSlot<super::StorageGeneration>,
}

impl HotSwappableStorageProfile {
    pub(crate) const fn new(plugin_id: String, generation: Arc<super::StorageGeneration>) -> Self {
        Self {
            plugin_id,
            generation: ReloadableGenerationSlot::new(
                generation,
                "storage generation lock should not be poisoned",
                "storage reload gate should not be poisoned",
            ),
        }
    }

    pub(crate) fn current_generation(&self) -> Arc<super::StorageGeneration> {
        self.generation.current()
    }

    pub(crate) fn swap_generation_while_reloading(
        &self,
        generation: Arc<super::StorageGeneration>,
    ) {
        self.generation.swap_while_reloading(generation);
    }

    pub(crate) fn with_reload_write<T>(
        &self,
        f: impl FnOnce(Arc<super::StorageGeneration>) -> T,
    ) -> T {
        self.generation.with_reload_write(f)
    }

    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    fn capability_set(&self) -> StorageCapabilitySet {
        self.generation.capability_set()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Some(self.generation.generation_id())
    }

    fn with_generation<T>(
        &self,
        f: impl FnOnce(&super::StorageGeneration) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        self.generation
            .with_reload_read(|generation| f(&generation))
    }

    fn invoke<T>(
        &self,
        request: StorageRequest,
        map_response: impl FnOnce(StorageResponse) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        self.with_generation(|generation| {
            let response = generation.invoke(&request)?;
            map_response(response)
        })
    }

    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        self.invoke(
            StorageRequest::LoadSnapshot {
                world_dir: world_dir.display().to_string(),
            },
            |response| match response {
                StorageResponse::Snapshot(snapshot) => Ok(snapshot),
                other => Err(StorageError::Plugin(format!(
                    "unexpected storage load_snapshot payload: {other:?}"
                ))),
            },
        )
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        self.invoke(
            StorageRequest::SaveSnapshot {
                world_dir: world_dir.display().to_string(),
                snapshot: snapshot.clone(),
            },
            |response| match response {
                StorageResponse::Empty => Ok(()),
                other => Err(StorageError::Plugin(format!(
                    "unexpected storage save_snapshot payload: {other:?}"
                ))),
            },
        )
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

    fn capability_set(&self) -> StorageCapabilitySet {
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
