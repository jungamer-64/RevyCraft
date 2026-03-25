use super::ServerCore;
use crate::world::{BlockEntityState, BlockPos, BlockState, generate_superflat_chunk};

impl ServerCore {
    pub(super) fn block_at(&self, position: BlockPos) -> BlockState {
        let chunk_pos = position.chunk_pos();
        let local_x = u8::try_from(position.x.rem_euclid(crate::CHUNK_WIDTH))
            .expect("local x should fit into u8");
        let local_z = u8::try_from(position.z.rem_euclid(crate::CHUNK_WIDTH))
            .expect("local z should fit into u8");
        self.world
            .chunks
            .get(&chunk_pos)
            .cloned()
            .unwrap_or_else(|| generate_superflat_chunk(chunk_pos))
            .get_block(local_x, position.y, local_z)
    }

    pub(super) fn set_block_at(&mut self, position: BlockPos, state: BlockState) {
        let chunk_pos = position.chunk_pos();
        let local_x = u8::try_from(position.x.rem_euclid(crate::CHUNK_WIDTH))
            .expect("local x should fit into u8");
        let local_z = u8::try_from(position.z.rem_euclid(crate::CHUNK_WIDTH))
            .expect("local z should fit into u8");
        let chunk = self
            .world
            .chunks
            .entry(chunk_pos)
            .or_insert_with(|| generate_superflat_chunk(chunk_pos));
        chunk.set_block(local_x, position.y, local_z, state);
    }

    pub(super) fn block_entity_at(&self, position: BlockPos) -> Option<BlockEntityState> {
        self.world.block_entities.get(&position).cloned()
    }
}
