use crate::PlayerId;
use crate::player::PlayerSnapshot;
use revy_voxel_model::{BlockPos, ChunkColumn, ChunkPos, WorldMeta};
use revy_voxel_rules::BlockEntityState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub meta: WorldMeta,
    pub chunks: BTreeMap<ChunkPos, ChunkColumn>,
    #[serde(default)]
    pub block_entities: BTreeMap<BlockPos, BlockEntityState>,
    pub players: BTreeMap<PlayerId, PlayerSnapshot>,
}
