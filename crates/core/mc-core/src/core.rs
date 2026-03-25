mod canonical;
mod command;
mod inventory;
mod login;
mod mining;
mod mutation;
mod state_backend;
mod tick;
pub(crate) mod transaction;
mod world;

use crate::catalog::MiningToolSpec;
use crate::events::PlayerSummary;
use crate::inventory::{ItemStack, PlayerInventory};
use crate::player::PlayerSnapshot;
use crate::world::{
    BlockEntityState, BlockPos, ChunkColumn, ChunkPos, DimensionId, DroppedItemSnapshot, WorldMeta,
    required_chunks,
};
use crate::{DEFAULT_KEEPALIVE_INTERVAL_MS, DEFAULT_KEEPALIVE_TIMEOUT_MS, EntityId, PlayerId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) use self::inventory::OpenInventoryWindow;
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
pub struct WorldStore {
    pub(super) config: CoreConfig,
    pub(super) world_meta: WorldMeta,
    pub(super) chunks: BTreeMap<ChunkPos, ChunkColumn>,
    pub(super) block_entities: BTreeMap<BlockPos, BlockEntityState>,
    pub(super) chest_viewers: BTreeMap<BlockPos, BTreeMap<PlayerId, u8>>,
    pub(super) saved_players: BTreeMap<PlayerId, PlayerSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EntityKind {
    Player,
    DroppedItem,
}

#[derive(Clone, Debug)]
pub(super) struct PlayerIdentity {
    pub(super) player_id: PlayerId,
    pub(super) username: String,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PlayerTransform {
    pub(super) position: crate::Vec3,
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) on_ground: bool,
    pub(super) dimension: DimensionId,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PlayerVitals {
    pub(super) health: f32,
    pub(super) food: i16,
    pub(super) food_saturation: f32,
}

#[derive(Clone, Debug)]
pub(super) struct EntityStore {
    pub(super) entity_kinds: BTreeMap<EntityId, EntityKind>,
    pub(super) players_by_player_id: BTreeMap<PlayerId, EntityId>,
    pub(super) player_identity: BTreeMap<EntityId, PlayerIdentity>,
    pub(super) player_transform: BTreeMap<EntityId, PlayerTransform>,
    pub(super) player_vitals: BTreeMap<EntityId, PlayerVitals>,
    pub(super) player_inventory: BTreeMap<EntityId, PlayerInventory>,
    pub(super) player_selected_hotbar: BTreeMap<EntityId, u8>,
    pub(super) player_active_mining: BTreeMap<EntityId, ActiveMiningState>,
    pub(super) dropped_items: BTreeMap<EntityId, DroppedItemState>,
    pub(super) next_entity_id: i32,
}

#[derive(Clone, Debug)]
pub(crate) struct PlayerSessionState {
    pub(crate) entity_id: EntityId,
    pub(crate) cursor: Option<ItemStack>,
    pub(crate) active_container: Option<OpenInventoryWindow>,
    pub(crate) next_non_player_window_id: u8,
    pub(crate) view: ClientView,
    pub(crate) pending_keep_alive_id: Option<i32>,
    pub(crate) last_keep_alive_sent_at: Option<u64>,
    pub(crate) next_keep_alive_at: u64,
}

#[derive(Clone, Debug)]
pub(super) struct SessionStore {
    pub(super) player_sessions: BTreeMap<PlayerId, PlayerSessionState>,
    pub(super) next_keep_alive_id: i32,
    pub(super) keepalive_interval_ms: u64,
    pub(super) keepalive_timeout_ms: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemScheduler;

#[derive(Clone, Debug)]
pub struct ServerCore {
    pub(super) world: WorldStore,
    pub(super) entities: EntityStore,
    pub(super) sessions: SessionStore,
    pub(super) scheduler: SystemScheduler,
}

#[derive(Clone, Debug)]
pub(crate) struct DroppedItemState {
    pub(crate) snapshot: DroppedItemSnapshot,
    pub(crate) last_updated_at_ms: u64,
    pub(crate) pickup_allowed_at_ms: u64,
    pub(crate) despawn_at_ms: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveMiningState {
    pub(crate) position: BlockPos,
    pub(crate) started_at_ms: u64,
    pub(crate) duration_ms: u64,
    pub(crate) last_stage: Option<u8>,
    #[expect(dead_code, reason = "tool bonuses are scaffolded but not applied yet")]
    pub(crate) tool_context: Option<MiningToolSpec>,
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
            world: WorldStore {
                config,
                world_meta,
                chunks: BTreeMap::new(),
                block_entities: BTreeMap::new(),
                chest_viewers: BTreeMap::new(),
                saved_players: BTreeMap::new(),
            },
            entities: EntityStore {
                entity_kinds: BTreeMap::new(),
                players_by_player_id: BTreeMap::new(),
                player_identity: BTreeMap::new(),
                player_transform: BTreeMap::new(),
                player_vitals: BTreeMap::new(),
                player_inventory: BTreeMap::new(),
                player_selected_hotbar: BTreeMap::new(),
                player_active_mining: BTreeMap::new(),
                dropped_items: BTreeMap::new(),
                next_entity_id: 1,
            },
            sessions: SessionStore {
                player_sessions: BTreeMap::new(),
                next_keep_alive_id: 1,
                keepalive_interval_ms: DEFAULT_KEEPALIVE_INTERVAL_MS,
                keepalive_timeout_ms: DEFAULT_KEEPALIVE_TIMEOUT_MS,
            },
            scheduler: SystemScheduler,
        }
    }

    #[must_use]
    pub fn from_snapshot(config: CoreConfig, snapshot: crate::WorldSnapshot) -> Self {
        let mut core = Self::new(config);
        core.world.world_meta = snapshot.meta;
        core.world.chunks = snapshot.chunks;
        core.world.block_entities = snapshot.block_entities;
        core.world.saved_players = snapshot.players;
        core
    }

    #[must_use]
    pub fn snapshot(&self) -> crate::WorldSnapshot {
        let mut players = self.world.saved_players.clone();
        for player_id in self.sessions.player_sessions.keys().copied() {
            let view = self::state_backend::BaseStateRef::new(self);
            if let Some(snapshot) =
                self::inventory::persisted_online_player_snapshot_state(&view, player_id)
            {
                players.insert(player_id, snapshot);
            }
        }
        crate::WorldSnapshot {
            meta: self.world.world_meta.clone(),
            chunks: self.world.chunks.clone(),
            block_entities: self.world.block_entities.clone(),
            players,
        }
    }

    #[must_use]
    pub fn player_summary(&self) -> PlayerSummary {
        PlayerSummary {
            online_players: self.sessions.player_sessions.len(),
            max_players: self.world.config.max_players,
        }
    }

    pub fn set_max_players(&mut self, max_players: u8) {
        self.world.config.max_players = max_players;
        self.world.world_meta.max_players = max_players;
    }

    #[must_use]
    pub fn world_meta(&self) -> &WorldMeta {
        &self.world.world_meta
    }

    #[must_use]
    pub(super) fn player_entity_id(&self, player_id: PlayerId) -> Option<EntityId> {
        self.entities.players_by_player_id.get(&player_id).copied()
    }

    #[must_use]
    pub(super) fn player_session(&self, player_id: PlayerId) -> Option<&PlayerSessionState> {
        self.sessions.player_sessions.get(&player_id)
    }

    pub(super) fn player_session_mut(
        &mut self,
        player_id: PlayerId,
    ) -> Option<&mut PlayerSessionState> {
        self.sessions.player_sessions.get_mut(&player_id)
    }

    #[cfg(test)]
    #[must_use]
    pub(super) fn player_active_mining(&self, player_id: PlayerId) -> Option<&ActiveMiningState> {
        let entity_id = self.player_entity_id(player_id)?;
        self.entities.player_active_mining.get(&entity_id)
    }

    #[cfg(test)]
    pub(super) fn compose_player_snapshot_by_entity(
        &self,
        entity_id: EntityId,
    ) -> Option<PlayerSnapshot> {
        let identity = self.entities.player_identity.get(&entity_id)?;
        let transform = self.entities.player_transform.get(&entity_id)?;
        let vitals = self.entities.player_vitals.get(&entity_id)?;
        let inventory = self.entities.player_inventory.get(&entity_id)?;
        let selected_hotbar_slot = *self.entities.player_selected_hotbar.get(&entity_id)?;
        Some(PlayerSnapshot {
            id: identity.player_id,
            username: identity.username.clone(),
            position: transform.position,
            yaw: transform.yaw,
            pitch: transform.pitch,
            on_ground: transform.on_ground,
            dimension: transform.dimension,
            health: vitals.health,
            food: vitals.food,
            food_saturation: vitals.food_saturation,
            inventory: inventory.clone(),
            selected_hotbar_slot,
        })
    }

    #[must_use]
    #[cfg(test)]
    pub(super) fn compose_player_snapshot(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        let entity_id = self.player_entity_id(player_id)?;
        self.compose_player_snapshot_by_entity(entity_id)
    }

    pub(super) fn spawn_online_player(&mut self, player: PlayerSnapshot, now_ms: u64) -> EntityId {
        let player_id = player.id;
        let entity_id = EntityId(self.entities.next_entity_id);
        self.entities.next_entity_id = self.entities.next_entity_id.saturating_add(1);
        let view = ClientView::new(player.position.chunk_pos(), self.world.config.view_distance);
        self.entities
            .entity_kinds
            .insert(entity_id, EntityKind::Player);
        self.entities
            .players_by_player_id
            .insert(player_id, entity_id);
        self.entities.player_identity.insert(
            entity_id,
            PlayerIdentity {
                player_id,
                username: player.username.clone(),
            },
        );
        self.entities.player_transform.insert(
            entity_id,
            PlayerTransform {
                position: player.position,
                yaw: player.yaw,
                pitch: player.pitch,
                on_ground: player.on_ground,
                dimension: player.dimension,
            },
        );
        self.entities.player_vitals.insert(
            entity_id,
            PlayerVitals {
                health: player.health,
                food: player.food,
                food_saturation: player.food_saturation,
            },
        );
        self.entities
            .player_inventory
            .insert(entity_id, player.inventory);
        self.entities
            .player_selected_hotbar
            .insert(entity_id, player.selected_hotbar_slot);
        self.entities.player_active_mining.remove(&entity_id);
        self.sessions.player_sessions.insert(
            player_id,
            PlayerSessionState {
                entity_id,
                cursor: None,
                active_container: None,
                next_non_player_window_id: 1,
                view,
                pending_keep_alive_id: None,
                last_keep_alive_sent_at: None,
                next_keep_alive_at: now_ms.saturating_add(self.sessions.keepalive_interval_ms),
            },
        );
        entity_id
    }

    pub(super) fn remove_online_player(
        &mut self,
        player_id: PlayerId,
    ) -> Option<PlayerSessionState> {
        let session = self.sessions.player_sessions.remove(&player_id)?;
        let entity_id = session.entity_id;
        self.entities.players_by_player_id.remove(&player_id);
        self.entities.entity_kinds.remove(&entity_id);
        self.entities.player_identity.remove(&entity_id);
        self.entities.player_transform.remove(&entity_id);
        self.entities.player_vitals.remove(&entity_id);
        self.entities.player_inventory.remove(&entity_id);
        self.entities.player_selected_hotbar.remove(&entity_id);
        self.entities.player_active_mining.remove(&entity_id);
        Some(session)
    }
}
