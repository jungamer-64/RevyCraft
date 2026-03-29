#![allow(clippy::multiple_crate_versions)]

mod chunk_nbt;
mod level;
mod nbt;
mod playerdata;
mod region;

#[cfg(test)]
mod tests;

use mc_plugin_api::codec::storage::StorageDescriptor;
use mc_plugin_sdk_rust::capabilities::{build_tag_contains, storage_capabilities};
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use mc_plugin_sdk_rust::storage::RustStoragePlugin;
use mc_proto_common::StorageError;
use revy_voxel_core::{StorageCapability, StorageCapabilitySet, WorldSnapshot};
use revy_voxel_model::{ChunkColumn, ChunkPos};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub(crate) const JE_1_18_2_DATA_VERSION: i32 = 2975;
pub(crate) const JE_1_18_2_MIN_SECTION_Y: i32 = -4;
pub(crate) const JE_1_18_2_MAX_SECTION_Y: i32 = 19;
const LEVEL_DAT: &str = "level.dat";
const PLAYERDATA_DIR: &str = "playerdata";
const REGION_DIR: &str = "region";

pub const JE_1_18_2_STORAGE_PROFILE_ID: &str = "je-anvil-1_18_2";
pub const JE_1_18_2_STORAGE_PLUGIN_ID: &str = "storage-je-anvil-1_18_2";

#[derive(Default)]
pub struct Je1182StoragePlugin;

impl RustStoragePlugin for Je1182StoragePlugin {
    fn descriptor(&self) -> StorageDescriptor {
        StorageDescriptor {
            storage_profile: JE_1_18_2_STORAGE_PROFILE_ID.into(),
        }
    }

    fn capability_set(&self) -> StorageCapabilitySet {
        storage_capabilities(&[StorageCapability::RuntimeReload])
    }

    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        let level_path = world_dir.join(LEVEL_DAT);
        if !level_path.exists() {
            return Ok(None);
        }
        let meta = level::read_level_dat(&level_path)?;
        let (chunks, block_entities) = region::read_regions(&world_dir.join(REGION_DIR))?;
        let players = playerdata::read_playerdata(&world_dir.join(PLAYERDATA_DIR))?;
        Ok(Some(WorldSnapshot {
            meta,
            chunks,
            block_entities,
            players,
        }))
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        fs::create_dir_all(world_dir)?;
        level::write_level_dat(&world_dir.join(LEVEL_DAT), &snapshot.meta)?;
        region::write_regions(
            &world_dir.join(REGION_DIR),
            &chunks_for_save(snapshot),
            &snapshot.block_entities,
        )?;
        playerdata::write_playerdata(&world_dir.join(PLAYERDATA_DIR), &snapshot.players)?;
        Ok(())
    }

    fn import_runtime_state(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        if build_tag_contains("reload-fail") {
            return Err(StorageError::Plugin(
                "storage plugin refused runtime state import".to_string(),
            ));
        }
        self.save_snapshot(world_dir, snapshot)
    }
}

fn chunks_for_save(snapshot: &WorldSnapshot) -> BTreeMap<ChunkPos, ChunkColumn> {
    if !snapshot.chunks.is_empty() {
        return snapshot.chunks.clone();
    }
    let chunk_pos = snapshot.meta.spawn.chunk_pos();
    let mut chunks = BTreeMap::new();
    chunks.insert(chunk_pos, mc_content_canonical::default_chunk(chunk_pos));
    chunks
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::storage(
    JE_1_18_2_STORAGE_PLUGIN_ID,
    "JE 1.18.2 Anvil Storage Plugin",
    JE_1_18_2_STORAGE_PROFILE_ID,
);

export_plugin!(storage, Je1182StoragePlugin, MANIFEST);
