use super::{PlayerTransform, ServerCore};
use crate::player::PlayerSnapshot;
use crate::world::{
    BlockEntityState, BlockPos, BlockState, ChunkColumn, ChunkPos, generate_superflat_chunk,
    required_chunks,
};
use crate::{BLOCK_EDIT_REACH, PLAYER_HEIGHT, PLAYER_WIDTH};

impl ServerCore {
    pub(super) fn initial_visible_chunks(
        &mut self,
        center: ChunkPos,
        view_distance: u8,
    ) -> Vec<ChunkColumn> {
        required_chunks(center, view_distance)
            .into_iter()
            .map(|chunk_pos| self.ensure_chunk(chunk_pos).clone())
            .collect()
    }

    pub(super) fn ensure_chunk(&mut self, chunk_pos: ChunkPos) -> &ChunkColumn {
        self.world
            .chunks
            .entry(chunk_pos)
            .or_insert_with(|| generate_superflat_chunk(chunk_pos))
    }

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

    pub(super) fn can_edit_block_for_snapshot(
        &self,
        actor: &PlayerSnapshot,
        position: BlockPos,
    ) -> bool {
        if !(0..=255).contains(&position.y) {
            return false;
        }
        if distance_squared_to_block_center(actor.position, position) > BLOCK_EDIT_REACH.powi(2) {
            return false;
        }
        !self
            .entities
            .player_transform
            .values()
            .any(|transform| block_intersects_player(position, transform))
    }
}

fn distance_squared_to_block_center(position: crate::Vec3, block: BlockPos) -> f64 {
    let eye_x = position.x;
    let eye_y = position.y + 1.62;
    let eye_z = position.z;
    let center_x = f64::from(block.x) + 0.5;
    let center_y = f64::from(block.y) + 0.5;
    let center_z = f64::from(block.z) + 0.5;
    let dx = eye_x - center_x;
    let dy = eye_y - center_y;
    let dz = eye_z - center_z;
    dx * dx + dy * dy + dz * dz
}

fn block_intersects_player(block: BlockPos, player: &PlayerTransform) -> bool {
    let half_width = PLAYER_WIDTH / 2.0;
    let player_min_x = player.position.x - half_width;
    let player_max_x = player.position.x + half_width;
    let player_min_y = player.position.y;
    let player_max_y = player.position.y + PLAYER_HEIGHT;
    let player_min_z = player.position.z - half_width;
    let player_max_z = player.position.z + half_width;

    let block_min_x = f64::from(block.x);
    let block_max_x = block_min_x + 1.0;
    let block_min_y = f64::from(block.y);
    let block_max_y = block_min_y + 1.0;
    let block_min_z = f64::from(block.z);
    let block_max_z = block_min_z + 1.0;

    player_min_x < block_max_x
        && player_max_x > block_min_x
        && player_min_y < block_max_y
        && player_max_y > block_min_y
        && player_min_z < block_max_z
        && player_max_z > block_min_z
}
