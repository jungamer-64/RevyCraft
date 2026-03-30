use crate::PlayerId;
use crate::player::PlayerSnapshot;
use crate::{BlockEntityState, BlockPos, ChunkColumn, ChunkPos, WorldMeta};
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
