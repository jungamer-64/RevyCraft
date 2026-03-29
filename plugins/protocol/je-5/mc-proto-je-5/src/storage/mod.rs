mod chunk_nbt;
mod level;
mod nbt;
mod playerdata;
mod region;

#[cfg(test)]
mod tests;

use mc_proto_common::{StorageAdapter, StorageError};
use revy_voxel_core::WorldSnapshot;
use std::fs;
use std::path::Path;

const LEVEL_DAT: &str = "level.dat";
const PLAYERDATA_DIR: &str = "playerdata";
const REGION_DIR: &str = "region";

#[derive(Default)]
pub struct Je1710StorageAdapter;

impl StorageAdapter for Je1710StorageAdapter {
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
            &snapshot.chunks,
            &snapshot.block_entities,
        )?;
        playerdata::write_playerdata(&world_dir.join(PLAYERDATA_DIR), &snapshot.players)?;
        Ok(())
    }
}
