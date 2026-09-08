mod canonical;
mod command;
mod inventory;
mod login;
mod mining;
mod mutation;
mod state_backend;
mod tick;
pub(crate) mod transaction;
mod version;
mod world;

use crate::events::PlayerSummary;
use crate::events::{CoreEvent, EventTarget, TargetedEvent};
use crate::inventory::{InventoryWindowContents, ItemStack, PlayerInventory};
use crate::player::PlayerSnapshot;
use crate::world::{
    BlockEntityState, BlockPos, BlockState, ChunkColumn, ChunkPos, DimensionId,
    DroppedItemSnapshot, WorldMeta, required_chunks,
};
use crate::{DEFAULT_KEEPALIVE_INTERVAL_MS, DEFAULT_KEEPALIVE_TIMEOUT_MS, EntityId, PlayerId};
use revy_voxel_semantic::{ContainerKindId, ContentBehavior, MiningToolSpec};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

pub use self::inventory::OpenInventoryWindow;
use self::state_backend::CoreStateRead;
pub use self::transaction::{
    GameplayEffectApplyResult, GameplayLoginPreview, GameplayLoginPreviewError,
};
pub use self::version::{
    CoreHandoff, CoreMutation, CoreRevision, CoreTransferCommit, CoreTransferDelta,
    CoreTransferDeltaDescriptor, CoreTransferError, CoreTransferMutation, CoreTransferSnapshot,
    CoreVersion, EncodedCoreTransferCommit, PreparedCoreCommit,
};
pub use revy_voxel_semantic::CoreConfig;

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

#[derive(Clone, Debug)]
pub struct WorldStore {
    pub(super) config: CoreConfig,
    pub(super) world_meta: WorldMeta,
    pub(super) chunks: VersionComponent<BTreeMap<ChunkPos, VersionComponent<ChunkColumn>>>,
    pub(super) block_entities: VersionComponent<BTreeMap<BlockPos, BlockEntityState>>,
    pub(super) container_viewers: VersionComponent<BTreeMap<BlockPos, WorldContainerViewers>>,
    pub(super) saved_players: VersionComponent<BTreeMap<PlayerId, PlayerSnapshot>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldContainerViewers {
    pub kind: ContainerKindId,
    pub viewers: BTreeMap<PlayerId, u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum EntityKind {
    Player,
    DroppedItem,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct PlayerIdentity {
    pub(super) player_id: PlayerId,
    pub(super) username: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) struct PlayerTransform {
    pub(super) position: crate::Vec3,
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) on_ground: bool,
    pub(super) dimension: DimensionId,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) struct PlayerVitals {
    pub(super) health: f32,
    pub(super) food: i16,
    pub(super) food_saturation: f32,
}

#[derive(Clone, Debug)]
pub(super) struct EntityStore {
    pub(super) entity_kinds: VersionComponent<BTreeMap<EntityId, EntityKind>>,
    pub(super) players_by_player_id: VersionComponent<BTreeMap<PlayerId, EntityId>>,
    pub(super) player_identity: VersionComponent<BTreeMap<EntityId, PlayerIdentity>>,
    pub(super) player_transform: VersionComponent<BTreeMap<EntityId, PlayerTransform>>,
    pub(super) player_vitals: VersionComponent<BTreeMap<EntityId, PlayerVitals>>,
    pub(super) player_inventory: VersionComponent<BTreeMap<EntityId, PlayerInventory>>,
    pub(super) player_selected_hotbar: VersionComponent<BTreeMap<EntityId, u8>>,
    pub(super) player_active_mining: VersionComponent<BTreeMap<EntityId, ActiveMiningState>>,
    pub(super) dropped_items: VersionComponent<BTreeMap<EntityId, DroppedItemState>>,
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
    pub(super) player_sessions: VersionComponent<BTreeMap<PlayerId, PlayerSessionState>>,
    pub(super) next_keep_alive_id: i32,
    pub(super) keepalive_interval_ms: u64,
    pub(super) keepalive_timeout_ms: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemScheduler;

#[derive(Debug)]
pub(super) struct VersionComponent<T>(Arc<T>);

impl<T> VersionComponent<T> {
    fn new(value: T) -> Self {
        Self(Arc::new(value))
    }
}

impl<T> Clone for VersionComponent<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Deref for VersionComponent<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl<T: Clone> DerefMut for VersionComponent<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Arc::make_mut(&mut self.0)
    }
}

#[derive(Debug)]
pub struct ServerCore {
    pub(super) content_behavior: Arc<dyn ContentBehavior>,
    pub(super) world: VersionComponent<WorldStore>,
    pub(super) entities: VersionComponent<EntityStore>,
    pub(super) sessions: VersionComponent<SessionStore>,
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

impl ServerCore {
    #[must_use]
    pub fn new(config: CoreConfig, content_behavior: Arc<dyn ContentBehavior>) -> Self {
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
            content_behavior,
            world: VersionComponent::new(WorldStore {
                config,
                world_meta,
                chunks: VersionComponent::new(BTreeMap::new()),
                block_entities: VersionComponent::new(BTreeMap::new()),
                container_viewers: VersionComponent::new(BTreeMap::new()),
                saved_players: VersionComponent::new(BTreeMap::new()),
            }),
            entities: VersionComponent::new(EntityStore {
                entity_kinds: VersionComponent::new(BTreeMap::new()),
                players_by_player_id: VersionComponent::new(BTreeMap::new()),
                player_identity: VersionComponent::new(BTreeMap::new()),
                player_transform: VersionComponent::new(BTreeMap::new()),
                player_vitals: VersionComponent::new(BTreeMap::new()),
                player_inventory: VersionComponent::new(BTreeMap::new()),
                player_selected_hotbar: VersionComponent::new(BTreeMap::new()),
                player_active_mining: VersionComponent::new(BTreeMap::new()),
                dropped_items: VersionComponent::new(BTreeMap::new()),
                next_entity_id: 1,
            }),
            sessions: VersionComponent::new(SessionStore {
                player_sessions: VersionComponent::new(BTreeMap::new()),
                next_keep_alive_id: 1,
                keepalive_interval_ms: DEFAULT_KEEPALIVE_INTERVAL_MS,
                keepalive_timeout_ms: DEFAULT_KEEPALIVE_TIMEOUT_MS,
            }),
            scheduler: SystemScheduler,
        }
    }

    pub(crate) fn fork_components(&self) -> Self {
        Self {
            content_behavior: Arc::clone(&self.content_behavior),
            world: self.world.clone(),
            entities: self.entities.clone(),
            sessions: self.sessions.clone(),
            scheduler: self.scheduler,
        }
    }

    fn player_session_state(&self, player_id: PlayerId) -> Option<PlayerSessionState> {
        self.sessions.player_sessions.get(&player_id).cloned()
    }

    pub(super) fn world_mut(&mut self) -> &mut WorldStore {
        &mut self.world
    }

    pub(super) fn entities_mut(&mut self) -> &mut EntityStore {
        &mut self.entities
    }

    pub(super) fn sessions_mut(&mut self) -> &mut SessionStore {
        &mut self.sessions
    }

    #[must_use]
    pub fn from_snapshot(
        config: CoreConfig,
        snapshot: crate::WorldSnapshot,
        content_behavior: Arc<dyn ContentBehavior>,
    ) -> Self {
        let mut core = Self::new(config, content_behavior);
        let world = core.world_mut();
        world.world_meta = snapshot.meta;
        world.chunks = VersionComponent::new(snapshot.chunks);
        world.block_entities = VersionComponent::new(snapshot.block_entities);
        world.saved_players = VersionComponent::new(snapshot.players);
        core
    }

    #[must_use]
    pub fn snapshot(&self) -> crate::WorldSnapshot {
        let mut players = (*self.world.saved_players).clone();
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
            chunks: (*self.world.chunks).clone(),
            block_entities: (*self.world.block_entities).clone(),
            players,
        }
    }

    #[must_use]
    pub fn content_behavior(&self) -> &Arc<dyn ContentBehavior> {
        &self.content_behavior
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
                    container: window.container.kind.clone(),
                    contents: window.contents(inventory),
                },
            }];
            events.extend(
                window
                    .property_entries()
                    .into_iter()
                    .map(|(property, value)| TargetedEvent {
                        target: EventTarget::Player(player_id),
                        event: CoreEvent::ContainerPropertyChanged {
                            window_id: window.window_id,
                            property,
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
                    container: self.content_behavior.player_container_kind(),
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

    pub fn set_max_players(&mut self, max_players: u32) {
        let world = self.world_mut();
        world.config.max_players = max_players;
        world.world_meta.max_players = max_players;
    }

    fn reconfigure(&mut self, config: CoreConfig) {
        let world = self.world_mut();
        world.world_meta.level_name = config.level_name.clone();
        world.world_meta.game_mode = config.game_mode;
        world.world_meta.difficulty = config.difficulty;
        world.world_meta.max_players = config.max_players;
        world.config = config;
    }

    #[must_use]
    pub fn world_meta(&self) -> &WorldMeta {
        &self.world.world_meta
    }

    #[must_use]
    pub fn player_snapshot(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        self::state_backend::BaseStateRef::new(self).compose_player_snapshot(player_id)
    }

    #[must_use]
    pub fn block_state(&self, position: BlockPos) -> Option<BlockState> {
        self::state_backend::BaseStateRef::new(self).block_state(position)
    }

    #[must_use]
    pub fn block_entity(&self, position: BlockPos) -> Option<BlockEntityState> {
        self::state_backend::BaseStateRef::new(self).block_entity(position)
    }

    #[must_use]
    pub fn can_edit_block(&self, player_id: PlayerId, position: BlockPos) -> bool {
        self::state_backend::BaseStateRef::new(self)
            .compose_player_snapshot(player_id)
            .is_some_and(|player| {
                self::state_backend::BaseStateRef::new(self)
                    .can_edit_block_for_snapshot(&player, position)
            })
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
        self.sessions_mut().player_sessions.get_mut(&player_id)
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
        let session = self.sessions_mut().player_sessions.remove(&player_id)?;
        let entity_id = session.entity_id;
        let entities = self.entities_mut();
        entities.players_by_player_id.remove(&player_id);
        entities.entity_kinds.remove(&entity_id);
        entities.player_identity.remove(&entity_id);
        entities.player_transform.remove(&entity_id);
        entities.player_vitals.remove(&entity_id);
        entities.player_inventory.remove(&entity_id);
        entities.player_selected_hotbar.remove(&entity_id);
        entities.player_active_mining.remove(&entity_id);
        Some(session)
    }
}
