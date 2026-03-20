use super::ServerCore;
use crate::PlayerId;
use crate::gameplay::GameplayQuery;
use crate::player::PlayerSnapshot;
use crate::world::{BlockPos, BlockState, WorldMeta};

impl GameplayQuery for ServerCore {
    fn world_meta(&self) -> WorldMeta {
        self.world_meta.clone()
    }

    fn player_snapshot(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        self.online_players
            .get(&player_id)
            .map(|player| player.snapshot.clone())
    }

    fn block_state(&self, position: BlockPos) -> BlockState {
        self.block_at(position)
    }

    fn can_edit_block(&self, player_id: PlayerId, position: BlockPos) -> bool {
        self.player_snapshot(player_id)
            .is_some_and(|player| self.can_edit_block_for_snapshot(&player, position))
    }
}
