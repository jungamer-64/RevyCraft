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
use crate::events::{CoreEvent, EventTarget, TargetedEvent};
use crate::inventory::{InventoryContainer, InventoryWindowContents};
use crate::inventory::{ItemStack, PlayerInventory};
use crate::player::PlayerSnapshot;
use crate::world::{
    BlockEntityState, BlockPos, ChunkColumn, ChunkPos, DimensionId, DroppedItemSnapshot, WorldMeta,
    required_chunks,
};
use crate::{DEFAULT_KEEPALIVE_INTERVAL_MS, DEFAULT_KEEPALIVE_TIMEOUT_MS, EntityId, PlayerId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub use self::inventory::{
    ChestWindowBinding, ChestWindowState, ContainerDescriptor, FurnaceWindowBinding,
    FurnaceWindowState, OpenInventoryWindow, OpenInventoryWindowState,
};
use self::state_backend::CoreStateMut;

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerSessionState {
    pub entity_id: EntityId,
    pub cursor: Option<ItemStack>,
    pub active_container: Option<OpenInventoryWindow>,
    pub next_non_player_window_id: u8,
    pub view: ClientView,
    pub pending_keep_alive_id: Option<i32>,
    pub last_keep_alive_sent_at: Option<u64>,
    pub next_keep_alive_at: u64,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DroppedItemState {
    pub snapshot: DroppedItemSnapshot,
    pub last_updated_at_ms: u64,
    pub pickup_allowed_at_ms: u64,
    pub despawn_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActiveMiningState {
    pub position: BlockPos,
    pub started_at_ms: u64,
    pub duration_ms: u64,
    pub last_stage: Option<u8>,
    pub tool_context: Option<MiningToolSpec>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OnlinePlayerRuntimeState {
    pub player: PlayerSnapshot,
    pub session: PlayerSessionState,
    pub active_mining: Option<ActiveMiningState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoreRuntimeStateBlob {
    pub snapshot: crate::WorldSnapshot,
    pub online_players: BTreeMap<PlayerId, OnlinePlayerRuntimeState>,
    pub dropped_items: BTreeMap<EntityId, DroppedItemState>,
    pub chest_viewers: BTreeMap<BlockPos, BTreeMap<PlayerId, u8>>,
    pub next_entity_id: i32,
    pub next_keep_alive_id: i32,
    pub keepalive_interval_ms: u64,
    pub keepalive_timeout_ms: u64,
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
    pub fn export_runtime_state(&self) -> CoreRuntimeStateBlob {
        let online_players = self
            .sessions
            .player_sessions
            .iter()
            .filter_map(|(player_id, session)| {
                Some((
                    *player_id,
                    OnlinePlayerRuntimeState {
                        player: self.compose_player_snapshot_by_entity(session.entity_id)?,
                        session: session.clone(),
                        active_mining: self
                            .entities
                            .player_active_mining
                            .get(&session.entity_id)
                            .cloned(),
                    },
                ))
            })
            .collect();

        CoreRuntimeStateBlob {
            snapshot: self.snapshot(),
            online_players,
            dropped_items: self.entities.dropped_items.clone(),
            chest_viewers: self.world.chest_viewers.clone(),
            next_entity_id: self.entities.next_entity_id,
            next_keep_alive_id: self.sessions.next_keep_alive_id,
            keepalive_interval_ms: self.sessions.keepalive_interval_ms,
            keepalive_timeout_ms: self.sessions.keepalive_timeout_ms,
        }
    }

    #[must_use]
    pub fn from_runtime_state(config: CoreConfig, blob: CoreRuntimeStateBlob) -> Self {
        let mut core = Self::from_snapshot(config, blob.snapshot);
        core.world.chest_viewers = blob.chest_viewers;
        core.entities.dropped_items = blob.dropped_items;
        core.entities.next_entity_id = blob.next_entity_id;
        core.sessions.next_keep_alive_id = blob.next_keep_alive_id;
        core.sessions.keepalive_interval_ms = blob.keepalive_interval_ms;
        core.sessions.keepalive_timeout_ms = blob.keepalive_timeout_ms;
        core.world.world_meta.level_name = core.world.config.level_name.clone();
        core.world.world_meta.game_mode = core.world.config.game_mode;
        core.world.world_meta.difficulty = core.world.config.difficulty;
        core.world.world_meta.max_players = core.world.config.max_players;
        core.entities.entity_kinds = core
            .entities
            .dropped_items
            .keys()
            .copied()
            .map(|entity_id| (entity_id, EntityKind::DroppedItem))
            .collect();

        for player_id in blob.online_players.keys().copied().collect::<Vec<_>>() {
            core.world.saved_players.remove(&player_id);
        }

        for (player_id, online) in blob.online_players {
            let entity_id = {
                let mut state = self::state_backend::BaseState::new(&mut core);
                state.spawn_online_player(online.player, 0, Some(online.session.entity_id))
            };
            debug_assert_eq!(entity_id, online.session.entity_id);
            if let Some(session) = core.sessions.player_sessions.get_mut(&player_id) {
                *session = online.session;
            }
            if let Some(active_mining) = online.active_mining {
                core.entities
                    .player_active_mining
                    .insert(entity_id, active_mining);
            } else {
                core.entities.player_active_mining.remove(&entity_id);
            }
        }

        core
    }

    #[must_use]
    pub fn session_resync_events(&self, player_id: PlayerId) -> Vec<TargetedEvent> {
        let Some(session) = self.player_session(player_id) else {
            return Vec::new();
        };
        let Some(player) = self.compose_player_snapshot_by_entity(session.entity_id) else {
            return Vec::new();
        };
        let Some(inventory) = self.entities.player_inventory.get(&session.entity_id) else {
            return Vec::new();
        };
        let Some(selected_hotbar_slot) =
            self.entities.player_selected_hotbar.get(&session.entity_id)
        else {
            return Vec::new();
        };

        let mut events = vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::PlayBootstrap {
                player,
                entity_id: session.entity_id,
                world_meta: self.world.world_meta.clone(),
                view_distance: self.world.config.view_distance,
            },
        }];
        let visible_chunks = session
            .view
            .loaded_chunks
            .iter()
            .filter_map(|chunk_pos| self.world.chunks.get(chunk_pos).cloned())
            .collect::<Vec<_>>();
        events.push(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::ChunkBatch {
                chunks: visible_chunks,
            },
        });
        events.extend(self.entities.players_by_player_id.iter().filter_map(
            |(other_player_id, entity_id)| {
                if *other_player_id == player_id || *entity_id == session.entity_id {
                    return None;
                }
                let player = self.compose_player_snapshot_by_entity(*entity_id)?;
                session
                    .view
                    .loaded_chunks
                    .contains(&player.position.chunk_pos())
                    .then_some(TargetedEvent {
                        target: EventTarget::Player(player_id),
                        event: CoreEvent::EntitySpawned {
                            entity_id: *entity_id,
                            player,
                        },
                    })
            },
        ));
        events.extend(
            self.entities
                .dropped_items
                .iter()
                .filter_map(|(entity_id, item)| {
                    session
                        .view
                        .loaded_chunks
                        .contains(&item.snapshot.position.chunk_pos())
                        .then_some(TargetedEvent {
                            target: EventTarget::Player(player_id),
                            event: CoreEvent::DroppedItemSpawned {
                                entity_id: *entity_id,
                                item: item.snapshot.clone(),
                            },
                        })
                }),
        );
        events.extend(self.entities.player_active_mining.iter().filter_map(
            |(breaker_entity_id, mining)| {
                session
                    .view
                    .loaded_chunks
                    .contains(&mining.position.chunk_pos())
                    .then_some(TargetedEvent {
                        target: EventTarget::Player(player_id),
                        event: CoreEvent::BlockBreakingProgress {
                            breaker_entity_id: *breaker_entity_id,
                            position: mining.position,
                            stage: mining.last_stage,
                            duration_ms: mining.duration_ms,
                        },
                    })
            },
        ));

        events.extend(if let Some(window) = session.active_container.as_ref() {
            let mut events = vec![TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventoryContents {
                    window_id: window.window_id,
                    container: window.container,
                    contents: window.contents(inventory),
                },
            }];
            events.extend(
                window
                    .property_entries()
                    .into_iter()
                    .map(|(property_id, value)| TargetedEvent {
                        target: EventTarget::Player(player_id),
                        event: CoreEvent::ContainerPropertyChanged {
                            window_id: window.window_id,
                            property_id,
                            value,
                        },
                    }),
            );
            events
        } else {
            vec![TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventoryContents {
                    window_id: 0,
                    container: InventoryContainer::Player,
                    contents: InventoryWindowContents::player(inventory.clone()),
                },
            }]
        });
        events.push(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::SelectedHotbarSlotChanged {
                slot: *selected_hotbar_slot,
            },
        });
        events.push(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::CursorChanged {
                stack: session.cursor.clone(),
            },
        });
        events
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
