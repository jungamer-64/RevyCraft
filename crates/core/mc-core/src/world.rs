use crate::PlayerId;
use crate::player::PlayerSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[allow(unused_imports)]
pub(crate) use mc_content_api::{BlockEntityState, ContainerBlockEntityState};
#[allow(unused_imports)]
pub(crate) use mc_model::{
    BlockFace, BlockKey, BlockPos, BlockState, ChunkColumn, ChunkDelta, ChunkPos, ChunkSection,
    DimensionId, DroppedItemSnapshot, SectionBlockIndex, SectionPos, Vec3, WorldMeta,
    expand_block_index, flatten_block_index, required_chunks, section_local_y,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub meta: WorldMeta,
    pub chunks: BTreeMap<ChunkPos, ChunkColumn>,
    #[serde(default)]
    pub block_entities: BTreeMap<BlockPos, BlockEntityState>,
    pub players: BTreeMap<PlayerId, PlayerSnapshot>,
}
