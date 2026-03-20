use crate::events::{CoreCommand, CoreEvent, EventTarget, PlayerSummary, TargetedEvent};
use crate::gameplay::{
    CanonicalGameplayPolicy, GameplayEffect, GameplayJoinEffect, GameplayMutation,
    GameplayPolicyResolver, GameplayQuery, canonical_session_capabilities,
};
use crate::player::{
    InteractionHand, InventoryContainer, InventorySlot, ItemStack, PlayerInventory, PlayerSnapshot,
};
use crate::world::{
    BlockPos, BlockState, ChunkColumn, ChunkDelta, ChunkPos, DimensionId, Vec3, WorldMeta,
    generate_superflat_chunk, required_chunks,
};
use crate::{
    BLOCK_EDIT_REACH, ConnectionId, DEFAULT_KEEPALIVE_INTERVAL_MS, DEFAULT_KEEPALIVE_TIMEOUT_MS,
    EntityId, HOTBAR_SLOT_COUNT, PLAYER_HEIGHT, PLAYER_WIDTH, PlayerId, SessionCapabilitySet,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
    pub fn retarget(&mut self, center: ChunkPos, view_distance: u8) -> ChunkDelta {
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
        ChunkDelta { added, removed }
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
    config: CoreConfig,
    world_meta: WorldMeta,
    chunks: BTreeMap<ChunkPos, ChunkColumn>,
    saved_players: BTreeMap<PlayerId, PlayerSnapshot>,
    online_players: BTreeMap<PlayerId, OnlinePlayer>,
    next_entity_id: i32,
    next_keep_alive_id: i32,
    keepalive_interval_ms: u64,
    keepalive_timeout_ms: u64,
}

#[derive(Clone, Debug)]
struct OnlinePlayer {
    entity_id: EntityId,
    snapshot: PlayerSnapshot,
    view: ClientView,
    pending_keep_alive_id: Option<i32>,
    last_keep_alive_sent_at: Option<u64>,
    next_keep_alive_at: u64,
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
        core.saved_players = snapshot.players;
        core
    }

    #[must_use]
    pub fn snapshot(&self) -> crate::WorldSnapshot {
        let mut players = self.saved_players.clone();
        for (player_id, player) in &self.online_players {
            players.insert(*player_id, player.snapshot.clone());
        }
        crate::WorldSnapshot {
            meta: self.world_meta.clone(),
            chunks: self.chunks.clone(),
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

    pub fn set_max_players(&mut self, max_players: u8) {
        self.config.max_players = max_players;
        self.world_meta.max_players = max_players;
    }

    #[must_use]
    pub const fn world_meta(&self) -> &WorldMeta {
        &self.world_meta
    }

    /// Applies a command using the built-in canonical gameplay policy.
    ///
    /// # Panics
    ///
    /// Panics if the canonical gameplay policy returns an error while evaluating the command.
    pub fn apply_command(&mut self, command: CoreCommand, now_ms: u64) -> Vec<TargetedEvent> {
        let session = canonical_session_capabilities();
        self.apply_command_with_policy(command, now_ms, Some(&session), &CanonicalGameplayPolicy)
            .expect("canonical gameplay policy should not fail")
    }

    /// Applies a command using the provided gameplay policy resolver.
    ///
    /// # Errors
    ///
    /// Returns an error when the command requires session capabilities that are not present,
    /// or when the gameplay policy resolver rejects the command.
    pub fn apply_command_with_policy<R: GameplayPolicyResolver>(
        &mut self,
        command: CoreCommand,
        now_ms: u64,
        session: Option<&SessionCapabilitySet>,
        resolver: &R,
    ) -> Result<Vec<TargetedEvent>, String> {
        match command {
            CoreCommand::LoginStart {
                connection_id,
                username,
                player_id,
            } => self.login_player_with_policy(
                connection_id,
                username,
                player_id,
                now_ms,
                session.ok_or_else(|| "login requires session capabilities".to_string())?,
                resolver,
            ),
            CoreCommand::UpdateClientView {
                player_id,
                view_distance,
            } => Ok(self.update_client_settings(player_id, view_distance)),
            CoreCommand::ClientStatus {
                player_id: _,
                action_id: _,
            } => Ok(Vec::new()),
            CoreCommand::MoveIntent { .. }
            | CoreCommand::SetHeldSlot { .. }
            | CoreCommand::CreativeInventorySet { .. }
            | CoreCommand::DigBlock { .. }
            | CoreCommand::PlaceBlock { .. } => {
                let session = session.ok_or_else(|| {
                    "gameplay-owned command requires session capabilities".to_string()
                })?;
                let effect = resolver.handle_command(self, session, &command)?;
                Ok(self.apply_gameplay_effect(effect))
            }
            CoreCommand::KeepAliveResponse {
                player_id,
                keep_alive_id,
            } => {
                self.accept_keep_alive(player_id, keep_alive_id);
                Ok(Vec::new())
            }
            CoreCommand::Disconnect { player_id } => Ok(self.disconnect_player(player_id)),
        }
    }

    pub fn tick(&mut self, now_ms: u64) -> Vec<TargetedEvent> {
        let mut events = Vec::new();
        let player_ids = self.online_players.keys().copied().collect::<Vec<_>>();
        for player_id in player_ids {
            let Some(player) = self.online_players.get_mut(&player_id) else {
                continue;
            };
            if let Some(sent_at) = player.last_keep_alive_sent_at
                && now_ms.saturating_sub(sent_at) > self.keepalive_timeout_ms
            {
                events.extend(self.disconnect_player(player_id));
                continue;
            }
            if player.pending_keep_alive_id.is_none() && now_ms >= player.next_keep_alive_at {
                let keep_alive_id = self.next_keep_alive_id;
                self.next_keep_alive_id = self.next_keep_alive_id.saturating_add(1);
                player.pending_keep_alive_id = Some(keep_alive_id);
                player.last_keep_alive_sent_at = Some(now_ms);
                player.next_keep_alive_at = now_ms.saturating_add(self.keepalive_interval_ms);
                events.push(TargetedEvent {
                    target: EventTarget::Player(player_id),
                    event: CoreEvent::KeepAliveRequested { keep_alive_id },
                });
            }
        }
        events
    }

    /// Applies a tick for a single player using the provided gameplay policy resolver.
    ///
    /// # Errors
    ///
    /// Returns an error when the gameplay policy resolver rejects the tick.
    pub fn tick_player_with_policy<R: GameplayPolicyResolver>(
        &mut self,
        player_id: PlayerId,
        now_ms: u64,
        session: &SessionCapabilitySet,
        resolver: &R,
    ) -> Result<Vec<TargetedEvent>, String> {
        let effect = resolver.handle_tick(self, session, player_id, now_ms)?;
        Ok(self.apply_gameplay_effect(effect))
    }

    fn login_player_with_policy<R: GameplayPolicyResolver>(
        &mut self,
        connection_id: ConnectionId,
        username: String,
        player_id: PlayerId,
        now_ms: u64,
        session: &SessionCapabilitySet,
        resolver: &R,
    ) -> Result<Vec<TargetedEvent>, String> {
        if username.is_empty() || username.len() > 16 {
            return Ok(Self::reject_connection(connection_id, "Invalid username"));
        }
        if self.online_players.len() >= usize::from(self.config.max_players) {
            return Ok(Self::reject_connection(connection_id, "Server is full"));
        }
        if self.online_players.contains_key(&player_id) {
            return Ok(Self::reject_connection(
                connection_id,
                "Player is already online",
            ));
        }

        let mut player = self
            .saved_players
            .get(&player_id)
            .cloned()
            .unwrap_or_else(|| default_player(player_id, username.clone(), self.config.spawn));
        player.username = username;
        let join_effect = resolver.handle_player_join(self, session, &player)?;
        let join_events = Self::apply_gameplay_join_effect(&mut player, join_effect);

        let entity_id = EntityId(self.next_entity_id);
        self.next_entity_id = self.next_entity_id.saturating_add(1);

        let existing_players = self
            .online_players
            .values()
            .map(|online| (online.entity_id, online.snapshot.clone()))
            .collect::<Vec<_>>();

        let visible_chunks =
            self.initial_visible_chunks(player.position.chunk_pos(), self.config.view_distance);
        let view = ClientView::new(player.position.chunk_pos(), self.config.view_distance);

        self.online_players.insert(
            player_id,
            OnlinePlayer {
                entity_id,
                snapshot: player.clone(),
                view,
                pending_keep_alive_id: None,
                last_keep_alive_sent_at: None,
                next_keep_alive_at: now_ms.saturating_add(self.keepalive_interval_ms),
            },
        );

        let mut events =
            self.login_initial_events(connection_id, player_id, entity_id, &player, visible_chunks);
        events.extend(Self::existing_player_spawn_events(
            connection_id,
            existing_players,
        ));
        events.extend(join_events);

        events.push(TargetedEvent {
            target: EventTarget::EveryoneExcept(player_id),
            event: CoreEvent::EntitySpawned { entity_id, player },
        });
        Ok(events)
    }

    fn apply_gameplay_join_effect(
        player: &mut PlayerSnapshot,
        effect: GameplayJoinEffect,
    ) -> Vec<TargetedEvent> {
        if let Some(inventory) = effect.inventory {
            player.inventory = inventory;
        }
        if let Some(selected_hotbar_slot) = effect.selected_hotbar_slot {
            player.selected_hotbar_slot = selected_hotbar_slot;
        }
        effect.emitted_events
    }

    fn reject_connection(connection_id: ConnectionId, reason: &str) -> Vec<TargetedEvent> {
        vec![TargetedEvent {
            target: EventTarget::Connection(connection_id),
            event: CoreEvent::Disconnect {
                reason: reason.to_string(),
            },
        }]
    }

    fn login_initial_events(
        &self,
        connection_id: ConnectionId,
        player_id: PlayerId,
        entity_id: EntityId,
        player: &PlayerSnapshot,
        visible_chunks: Vec<ChunkColumn>,
    ) -> Vec<TargetedEvent> {
        vec![
            TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::LoginAccepted {
                    player_id,
                    entity_id,
                    player: player.clone(),
                },
            },
            TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::PlayBootstrap {
                    player: player.clone(),
                    entity_id,
                    world_meta: self.world_meta.clone(),
                    view_distance: self.config.view_distance,
                },
            },
            TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::ChunkBatch {
                    chunks: visible_chunks,
                },
            },
            TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::InventoryContents {
                    container: InventoryContainer::Player,
                    inventory: player.inventory.clone(),
                },
            },
            TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::SelectedHotbarSlotChanged {
                    slot: player.selected_hotbar_slot,
                },
            },
        ]
    }

    fn existing_player_spawn_events(
        connection_id: ConnectionId,
        existing_players: Vec<(EntityId, PlayerSnapshot)>,
    ) -> Vec<TargetedEvent> {
        existing_players
            .into_iter()
            .map(|(entity_id, player)| TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::EntitySpawned { entity_id, player },
            })
            .collect()
    }

    fn update_client_settings(
        &mut self,
        player_id: PlayerId,
        view_distance: u8,
    ) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return Vec::new();
        };
        let capped_view_distance = view_distance.min(self.config.view_distance).max(1);
        let delta = player
            .view
            .retarget(player.snapshot.position.chunk_pos(), capped_view_distance);
        delta
            .added
            .into_iter()
            .map(|chunk_pos| TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::ChunkBatch {
                    chunks: vec![self.ensure_chunk(chunk_pos).clone()],
                },
            })
            .collect()
    }

    pub fn apply_gameplay_effect(&mut self, effect: GameplayEffect) -> Vec<TargetedEvent> {
        let mut events = Vec::new();
        for mutation in effect.mutations {
            match mutation {
                GameplayMutation::PlayerPose {
                    player_id,
                    position,
                    yaw,
                    pitch,
                    on_ground,
                } => {
                    events.extend(
                        self.apply_player_pose_mutation(player_id, position, yaw, pitch, on_ground),
                    );
                }
                GameplayMutation::SelectedHotbarSlot { player_id, slot } => {
                    events.extend(self.apply_selected_hotbar_slot_mutation(player_id, slot));
                }
                GameplayMutation::InventorySlot {
                    player_id,
                    slot,
                    stack,
                } => {
                    events.extend(self.apply_inventory_slot_mutation(player_id, slot, stack));
                }
                GameplayMutation::Block { position, block } => {
                    events.extend(self.apply_block_mutation(position, block));
                }
            }
        }
        events.extend(effect.emitted_events);
        events
    }

    fn apply_player_pose_mutation(
        &mut self,
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    ) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return Vec::new();
        };

        if let Some(position) = position {
            player.snapshot.position = position;
        }
        if let Some(yaw) = yaw {
            player.snapshot.yaw = yaw;
        }
        if let Some(pitch) = pitch {
            player.snapshot.pitch = pitch;
        }
        player.snapshot.on_ground = on_ground;

        let delta = player.view.retarget(
            player.snapshot.position.chunk_pos(),
            player.view.view_distance,
        );
        let snapshot = player.snapshot.clone();
        let entity_id = player.entity_id;
        let added_chunks = delta.added;
        self.saved_players.insert(player_id, snapshot.clone());

        let mut events = Vec::new();
        for chunk_pos in added_chunks {
            events.push(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::ChunkBatch {
                    chunks: vec![self.ensure_chunk(chunk_pos).clone()],
                },
            });
        }

        events.push(TargetedEvent {
            target: EventTarget::EveryoneExcept(player_id),
            event: CoreEvent::EntityMoved {
                entity_id,
                player: snapshot,
            },
        });
        events
    }

    fn accept_keep_alive(&mut self, player_id: PlayerId, keep_alive_id: i32) {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return;
        };
        if player.pending_keep_alive_id == Some(keep_alive_id) {
            player.pending_keep_alive_id = None;
            player.last_keep_alive_sent_at = None;
        }
    }

    fn apply_selected_hotbar_slot_mutation(
        &mut self,
        player_id: PlayerId,
        slot: u8,
    ) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return Vec::new();
        };
        if slot >= HOTBAR_SLOT_COUNT {
            return Vec::new();
        }
        player.snapshot.selected_hotbar_slot = slot;
        self.saved_players
            .insert(player_id, player.snapshot.clone());
        vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::SelectedHotbarSlotChanged { slot },
        }]
    }

    fn apply_inventory_slot_mutation(
        &mut self,
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    ) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return Vec::new();
        };
        let _ = player.snapshot.inventory.set_slot(slot, stack.clone());
        self.saved_players
            .insert(player_id, player.snapshot.clone());
        vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::InventorySlotChanged {
                container: InventoryContainer::Player,
                slot,
                stack,
            },
        }]
    }

    fn apply_block_mutation(
        &mut self,
        position: BlockPos,
        block: BlockState,
    ) -> Vec<TargetedEvent> {
        self.set_block_at(position, block);
        self.emit_block_change(position)
    }

    pub(crate) fn place_inventory_correction(
        player_id: PlayerId,
        hand: InteractionHand,
        player: &PlayerSnapshot,
    ) -> Vec<TargetedEvent> {
        let selected_slot = match hand {
            InteractionHand::Main => InventorySlot::Hotbar(player.selected_hotbar_slot),
            InteractionHand::Offhand => InventorySlot::Offhand,
        };
        vec![
            TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventorySlotChanged {
                    container: InventoryContainer::Player,
                    slot: selected_slot,
                    stack: player.inventory.get_slot(selected_slot).cloned(),
                },
            },
            TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::SelectedHotbarSlotChanged {
                    slot: player.selected_hotbar_slot,
                },
            },
        ]
    }

    fn disconnect_player(&mut self, player_id: PlayerId) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.remove(&player_id) else {
            return Vec::new();
        };
        self.saved_players.insert(player_id, player.snapshot);
        vec![TargetedEvent {
            target: EventTarget::EveryoneExcept(player_id),
            event: CoreEvent::EntityDespawned {
                entity_ids: vec![player.entity_id],
            },
        }]
    }

    fn initial_visible_chunks(&mut self, center: ChunkPos, view_distance: u8) -> Vec<ChunkColumn> {
        required_chunks(center, view_distance)
            .into_iter()
            .map(|chunk_pos| self.ensure_chunk(chunk_pos).clone())
            .collect()
    }

    fn ensure_chunk(&mut self, chunk_pos: ChunkPos) -> &ChunkColumn {
        self.chunks
            .entry(chunk_pos)
            .or_insert_with(|| generate_superflat_chunk(chunk_pos))
    }

    fn block_at(&self, position: BlockPos) -> BlockState {
        let chunk_pos = position.chunk_pos();
        let local_x = u8::try_from(position.x.rem_euclid(crate::CHUNK_WIDTH))
            .expect("local x should fit into u8");
        let local_z = u8::try_from(position.z.rem_euclid(crate::CHUNK_WIDTH))
            .expect("local z should fit into u8");
        self.chunks
            .get(&chunk_pos)
            .cloned()
            .unwrap_or_else(|| generate_superflat_chunk(chunk_pos))
            .get_block(local_x, position.y, local_z)
    }

    fn set_block_at(&mut self, position: BlockPos, state: BlockState) {
        let chunk_pos = position.chunk_pos();
        let local_x = u8::try_from(position.x.rem_euclid(crate::CHUNK_WIDTH))
            .expect("local x should fit into u8");
        let local_z = u8::try_from(position.z.rem_euclid(crate::CHUNK_WIDTH))
            .expect("local z should fit into u8");
        let chunk = self
            .chunks
            .entry(chunk_pos)
            .or_insert_with(|| generate_superflat_chunk(chunk_pos));
        chunk.set_block(local_x, position.y, local_z, state);
    }

    fn emit_block_change(&self, position: BlockPos) -> Vec<TargetedEvent> {
        let block = self.block_at(position);
        self.online_players
            .iter()
            .filter(|(_, player)| player.view.loaded_chunks.contains(&position.chunk_pos()))
            .map(|(player_id, _)| TargetedEvent {
                target: EventTarget::Player(*player_id),
                event: CoreEvent::BlockChanged {
                    position,
                    block: block.clone(),
                },
            })
            .collect()
    }

    fn can_edit_block_for_snapshot(&self, actor: &PlayerSnapshot, position: BlockPos) -> bool {
        if !(0..=255).contains(&position.y) {
            return false;
        }
        if distance_squared_to_block_center(actor.position, position) > BLOCK_EDIT_REACH.powi(2) {
            return false;
        }
        !self
            .online_players
            .iter()
            .any(|(_, player)| block_intersects_player(position, &player.snapshot))
    }
}

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

fn default_player(player_id: PlayerId, username: String, spawn: BlockPos) -> PlayerSnapshot {
    PlayerSnapshot {
        id: player_id,
        username,
        position: Vec3::new(
            f64::from(spawn.x) + 0.5,
            f64::from(spawn.y),
            f64::from(spawn.z) + 0.5,
        ),
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        dimension: DimensionId::Overworld,
        health: 20.0,
        food: 20,
        food_saturation: 5.0,
        inventory: PlayerInventory::creative_starter(),
        selected_hotbar_slot: 0,
    }
}

fn distance_squared_to_block_center(position: Vec3, block: BlockPos) -> f64 {
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

fn block_intersects_player(block: BlockPos, player: &PlayerSnapshot) -> bool {
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
