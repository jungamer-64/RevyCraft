mod command;
mod inventory;
mod login;
mod mutation;
mod projection;
mod tick;
mod world;

use crate::events::PlayerSummary;
use crate::inventory::ItemStack;
use crate::player::PlayerSnapshot;
use crate::world::{
    BlockEntityState, BlockPos, ChunkColumn, ChunkPos, DimensionId, WorldMeta, required_chunks,
};
use crate::{DEFAULT_KEEPALIVE_INTERVAL_MS, DEFAULT_KEEPALIVE_TIMEOUT_MS, EntityId, PlayerId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use self::inventory::OpenInventoryWindow;
#[cfg(test)]
pub(crate) use self::inventory::OpenInventoryWindowState;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientView {
    pub center: ChunkPos,
    pub view_distance: u8,
    pub loaded_chunks: BTreeSet<ChunkPos>,
}

impl ClientView {
    #[must_use]
    pub fn new(center: ChunkPos, view_distance: u8) -> Self {
        let loaded_chunks = required_chunks(center, view_distance);
        Self {
            center,
            view_distance,
            loaded_chunks,
        }
    }

    #[must_use]
    pub fn retarget(&mut self, center: ChunkPos, view_distance: u8) -> crate::ChunkDelta {
        let next_loaded = required_chunks(center, view_distance);
        let added = next_loaded
            .difference(&self.loaded_chunks)
            .copied()
            .collect::<Vec<_>>();
        let removed = self
            .loaded_chunks
            .difference(&next_loaded)
            .copied()
            .collect::<Vec<_>>();
        self.center = center;
        self.view_distance = view_distance;
        self.loaded_chunks = next_loaded;
        crate::ChunkDelta { added, removed }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreConfig {
    pub level_name: String,
    pub seed: u64,
    pub max_players: u8,
    pub view_distance: u8,
    pub game_mode: u8,
    pub difficulty: u8,
    pub spawn: BlockPos,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            level_name: "world".to_string(),
            seed: 0,
            max_players: 20,
            view_distance: 2,
            game_mode: 0,
            difficulty: 1,
            spawn: BlockPos::new(0, 4, 0),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServerCore {
    pub(super) config: CoreConfig,
    pub(super) world_meta: WorldMeta,
    pub(super) chunks: BTreeMap<ChunkPos, ChunkColumn>,
    pub(super) block_entities: BTreeMap<BlockPos, BlockEntityState>,
    pub(super) chest_viewers: BTreeMap<BlockPos, BTreeMap<PlayerId, u8>>,
    pub(super) saved_players: BTreeMap<PlayerId, PlayerSnapshot>,
    pub(super) online_players: BTreeMap<PlayerId, OnlinePlayer>,
    pub(super) next_entity_id: i32,
    pub(super) next_keep_alive_id: i32,
    pub(super) keepalive_interval_ms: u64,
    pub(super) keepalive_timeout_ms: u64,
}

#[derive(Clone, Debug)]
pub struct OnlinePlayer {
    pub(super) entity_id: EntityId,
    pub(super) snapshot: PlayerSnapshot,
    pub(super) cursor: Option<ItemStack>,
    pub(super) active_container: Option<OpenInventoryWindow>,
    pub(super) next_non_player_window_id: u8,
    pub(super) view: ClientView,
    pub(super) pending_keep_alive_id: Option<i32>,
    pub(super) last_keep_alive_sent_at: Option<u64>,
    pub(super) next_keep_alive_at: u64,
}

impl ServerCore {
    #[must_use]
    pub fn new(config: CoreConfig) -> Self {
        let world_meta = WorldMeta {
            level_name: config.level_name.clone(),
            seed: config.seed,
            spawn: config.spawn,
            dimension: DimensionId::Overworld,
            age: 0,
            time: 6000,
            level_type: "FLAT".to_string(),
            game_mode: config.game_mode,
            difficulty: config.difficulty,
            max_players: config.max_players,
        };
        Self {
            config,
            world_meta,
            chunks: BTreeMap::new(),
            block_entities: BTreeMap::new(),
            chest_viewers: BTreeMap::new(),
            saved_players: BTreeMap::new(),
            online_players: BTreeMap::new(),
            next_entity_id: 1,
            next_keep_alive_id: 1,
            keepalive_interval_ms: DEFAULT_KEEPALIVE_INTERVAL_MS,
            keepalive_timeout_ms: DEFAULT_KEEPALIVE_TIMEOUT_MS,
        }
    }

    #[must_use]
    pub fn from_snapshot(config: CoreConfig, snapshot: crate::WorldSnapshot) -> Self {
        let mut core = Self::new(config);
        core.world_meta = snapshot.meta;
        core.chunks = snapshot.chunks;
        core.block_entities = snapshot.block_entities;
        core.saved_players = snapshot.players;
        core
    }

    #[must_use]
    pub fn snapshot(&self) -> crate::WorldSnapshot {
        let mut players = self.saved_players.clone();
        for (player_id, player) in &self.online_players {
            players.insert(*player_id, Self::persisted_online_player_snapshot(player));
        }
        crate::WorldSnapshot {
            meta: self.world_meta.clone(),
            chunks: self.chunks.clone(),
            block_entities: self.block_entities.clone(),
            players,
        }
    }

    #[must_use]
    pub fn player_summary(&self) -> PlayerSummary {
        PlayerSummary {
            online_players: self.online_players.len(),
            max_players: self.config.max_players,
        }
    }

    pub const fn set_max_players(&mut self, max_players: u8) {
        self.config.max_players = max_players;
        self.world_meta.max_players = max_players;
    }

    #[must_use]
    pub const fn world_meta(&self) -> &WorldMeta {
        &self.world_meta
    }
}
