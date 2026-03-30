#![allow(clippy::multiple_crate_versions)]

pub use revy_voxel_semantic::{
    BlockEntityState, BlockPos, BlockState, GameplayCanEditBlockKey, GameplayEffect,
    GameplayEffectBatch, GameplayReadSet, PlayerId, PlayerSnapshot, WorldMeta,
};

pub trait GameplayReadView: Send {
    fn world_meta(&mut self) -> WorldMeta;

    fn player_snapshot(&mut self, player_id: PlayerId) -> Option<PlayerSnapshot>;

    fn block_state(&mut self, position: BlockPos) -> Option<BlockState>;

    fn block_entity(&mut self, position: BlockPos) -> Option<BlockEntityState>;

    fn can_edit_block(&mut self, player_id: PlayerId, position: BlockPos) -> bool;
}
