use super::{
    ActiveMiningState, ClientView, DroppedItemState, EntityKind, PlayerIdentity,
    PlayerSessionState, PlayerTransform, PlayerVitals, ServerCore,
    canonical::{ApplyCoreOpsOptions, CoreOp, apply_core_ops},
};
use crate::catalog;
use crate::events::{CoreEvent, EventTarget, GameplayCommand, TargetedEvent};
use crate::inventory::{InventoryContainer, InventorySlot, ItemStack, PlayerInventory};
use crate::player::{InteractionHand, PlayerSnapshot};
use crate::world::{
    BlockEntityState, BlockFace, BlockPos, BlockState, ChunkColumn, ChunkPos, DroppedItemSnapshot,
    Vec3, WorldMeta, generate_superflat_chunk,
};
use crate::{
    BLOCK_EDIT_REACH, ConnectionId, EntityId, HOTBAR_SLOT_COUNT, PLAYER_HEIGHT, PLAYER_WIDTH,
    PlayerId,
};
use std::collections::{BTreeMap, BTreeSet};

const DROPPED_ITEM_PICKUP_DELAY_MS: u64 = 500;
const DROPPED_ITEM_DESPAWN_MS: u64 = 5 * 60 * 1000;

#[derive(Default)]
struct TxOverlay {
    chunks: BTreeMap<ChunkPos, ChunkColumn>,
    block_entities: BTreeMap<BlockPos, Option<BlockEntityState>>,
    players_by_player_id: BTreeMap<PlayerId, Option<EntityId>>,
    entity_kinds: BTreeMap<EntityId, Option<EntityKind>>,
    player_identity: BTreeMap<EntityId, Option<PlayerIdentity>>,
    player_transform: BTreeMap<EntityId, Option<PlayerTransform>>,
    player_vitals: BTreeMap<EntityId, Option<PlayerVitals>>,
    player_inventory: BTreeMap<EntityId, Option<PlayerInventory>>,
    player_selected_hotbar: BTreeMap<EntityId, Option<u8>>,
    player_active_mining: BTreeMap<EntityId, Option<ActiveMiningState>>,
    dropped_items: BTreeMap<EntityId, Option<DroppedItemState>>,
    player_sessions: BTreeMap<PlayerId, Option<PlayerSessionState>>,
    next_entity_id: Option<i32>,
}

pub struct GameplayTransaction<'a> {
    base: &'a mut ServerCore,
    overlay: TxOverlay,
    now_ms: u64,
    ops: Vec<CoreOp>,
}

impl<'a> GameplayTransaction<'a> {
    pub fn new(base: &'a mut ServerCore, now_ms: u64) -> Self {
        Self {
            base,
            overlay: TxOverlay::default(),
            now_ms,
            ops: Vec::new(),
        }
    }

    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    #[must_use]
    pub fn world_meta(&self) -> WorldMeta {
        self.base.world.world_meta.clone()
    }

    #[must_use]
    pub fn player_snapshot(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        self.compose_player_snapshot(player_id)
    }

    #[must_use]
    pub fn block_state(&self, position: BlockPos) -> BlockState {
        let chunk_pos = position.chunk_pos();
        if let Some(chunk) = self.overlay.chunks.get(&chunk_pos) {
            return chunk.get_block(local_block_x(position), position.y, local_block_z(position));
        }
        self.base.block_at(position)
    }

    #[must_use]
    pub fn block_entity(&self, position: BlockPos) -> Option<BlockEntityState> {
        if let Some(entry) = self.overlay.block_entities.get(&position) {
            return entry.clone();
        }
        self.base.block_entity_at(position)
    }

    #[must_use]
    pub fn can_edit_block(&self, player_id: PlayerId, position: BlockPos) -> bool {
        self.player_snapshot(player_id)
            .is_some_and(|player| self.can_edit_block_for_snapshot(&player, position))
    }

    pub fn set_player_pose(
        &mut self,
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    ) {
        let Some(entity_id) = self.player_entity_id(player_id) else {
            return;
        };
        let Some(transform) = self.ensure_player_transform_mut(entity_id) else {
            return;
        };
        if let Some(position) = position {
            transform.position = position;
        }
        if let Some(yaw) = yaw {
            transform.yaw = yaw;
        }
        if let Some(pitch) = pitch {
            transform.pitch = pitch;
        }
        transform.on_ground = on_ground;

        let center = transform.position.chunk_pos();
        if let Some(session) = self.ensure_player_session_mut(player_id) {
            let view_distance = session.view.view_distance;
            let _ = session.view.retarget(center, view_distance);
        }

        self.ops.push(CoreOp::SetPlayerPose {
            player_id,
            position,
            yaw,
            pitch,
            on_ground,
        });
    }

    pub fn set_selected_hotbar_slot(&mut self, player_id: PlayerId, slot: u8) {
        let Some(entity_id) = self.player_entity_id(player_id) else {
            return;
        };
        if slot >= HOTBAR_SLOT_COUNT {
            return;
        }
        let Some(selected_hotbar_slot) = self.ensure_player_selected_hotbar_mut(entity_id) else {
            return;
        };
        *selected_hotbar_slot = slot;
        self.ops
            .push(CoreOp::SetSelectedHotbarSlot { player_id, slot });
    }

    pub fn set_inventory_slot(
        &mut self,
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    ) {
        let Some(entity_id) = self.player_entity_id(player_id) else {
            return;
        };
        let Some(inventory) = self.ensure_player_inventory_mut(entity_id) else {
            return;
        };
        let _ = inventory.set_slot(slot, stack.clone());
        if slot.is_crafting_result() || slot.crafting_input_index().is_some() {
            ServerCore::recompute_crafting_result_for_inventory(inventory);
        }
        self.ops.push(CoreOp::SetInventorySlot {
            player_id,
            slot,
            stack,
        });
    }

    pub fn clear_mining(&mut self, player_id: PlayerId) {
        let Some(entity_id) = self.player_entity_id(player_id) else {
            return;
        };
        if self.current_active_mining(entity_id).is_none() {
            return;
        }
        self.overlay.player_active_mining.insert(entity_id, None);
        self.ops.push(CoreOp::ClearMining { player_id });
    }

    pub fn begin_mining(&mut self, player_id: PlayerId, position: BlockPos, duration_ms: u64) {
        let Some(entity_id) = self.player_entity_id(player_id) else {
            return;
        };
        if self
            .current_active_mining(entity_id)
            .is_some_and(|state| state.position == position)
        {
            return;
        }

        let Some(inventory) = self.current_player_inventory(entity_id) else {
            return;
        };
        let Some(selected_hotbar_slot) = self.current_player_selected_hotbar(entity_id) else {
            return;
        };
        self.overlay.player_active_mining.insert(
            entity_id,
            Some(ActiveMiningState {
                position,
                started_at_ms: self.now_ms,
                duration_ms,
                last_stage: Some(0),
                tool_context: catalog::tool_spec_for_item(
                    inventory.selected_hotbar_stack(selected_hotbar_slot),
                ),
            }),
        );
        self.ops.push(CoreOp::BeginMining {
            player_id,
            position,
            duration_ms,
        });
    }

    pub fn open_chest(&mut self, player_id: PlayerId, position: BlockPos) {
        if self.block_state(position).key.as_str() != catalog::CHEST {
            return;
        }
        if self.player_session(player_id).is_none() {
            return;
        }
        self.overlay
            .block_entities
            .entry(position)
            .or_insert_with(|| Some(BlockEntityState::chest(27)));
        self.ops.push(CoreOp::OpenChest {
            player_id,
            position,
        });
    }

    pub fn open_furnace(&mut self, player_id: PlayerId, position: BlockPos) {
        if self.block_state(position).key.as_str() != catalog::FURNACE {
            return;
        }
        if self.player_session(player_id).is_none() {
            return;
        }
        self.overlay
            .block_entities
            .entry(position)
            .or_insert_with(|| Some(BlockEntityState::furnace()));
        self.ops.push(CoreOp::OpenFurnace {
            player_id,
            position,
        });
    }

    pub fn set_block(&mut self, position: BlockPos, block: BlockState) {
        let chunk = self.ensure_chunk_mut(position.chunk_pos());
        chunk.set_block(
            local_block_x(position),
            position.y,
            local_block_z(position),
            block.clone(),
        );

        if block.key.as_str() == catalog::CHEST {
            self.overlay
                .block_entities
                .insert(position, Some(BlockEntityState::chest(27)));
        } else if block.key.as_str() == catalog::FURNACE {
            self.overlay
                .block_entities
                .insert(position, Some(BlockEntityState::furnace()));
        } else {
            self.overlay.block_entities.insert(position, None);
        }

        self.ops.push(CoreOp::SetBlock { position, block });
    }

    pub fn spawn_dropped_item(&mut self, position: Vec3, item: ItemStack) {
        let entity_id = self.allocate_entity_id();
        let snapshot = DroppedItemSnapshot {
            item: item.clone(),
            position,
            velocity: Vec3::new(0.0, 0.0, 0.0),
        };
        self.overlay
            .entity_kinds
            .insert(entity_id, Some(EntityKind::DroppedItem));
        self.overlay.dropped_items.insert(
            entity_id,
            Some(DroppedItemState {
                snapshot,
                last_updated_at_ms: self.now_ms,
                pickup_allowed_at_ms: self.now_ms.saturating_add(DROPPED_ITEM_PICKUP_DELAY_MS),
                despawn_at_ms: self.now_ms.saturating_add(DROPPED_ITEM_DESPAWN_MS),
            }),
        );
        self.ops.push(CoreOp::SpawnDroppedItem {
            expected_entity_id: Some(entity_id),
            position,
            item,
        });
    }

    pub fn emit_event(&mut self, target: EventTarget, event: CoreEvent) {
        self.ops.push(CoreOp::EmitEvent { target, event });
    }

    pub fn begin_login(
        &mut self,
        connection_id: ConnectionId,
        username: String,
        player_id: PlayerId,
    ) -> Result<Option<Vec<TargetedEvent>>, String> {
        if username.is_empty() || username.len() > 16 {
            return Ok(Some(ServerCore::reject_connection(
                connection_id,
                "Invalid username",
            )));
        }
        if self.online_player_count() >= usize::from(self.base.world.config.max_players) {
            return Ok(Some(ServerCore::reject_connection(
                connection_id,
                "Server is full",
            )));
        }
        if self.player_entity_id(player_id).is_some() {
            return Ok(Some(ServerCore::reject_connection(
                connection_id,
                "Player is already online",
            )));
        }

        let mut player = self
            .base
            .world
            .saved_players
            .get(&player_id)
            .cloned()
            .unwrap_or_else(|| {
                super::login::default_player(
                    player_id,
                    username.clone(),
                    self.base.world.config.spawn,
                )
            });
        player.username = username;
        ServerCore::recompute_crafting_result_for_inventory(&mut player.inventory);

        let entity_id = self.allocate_entity_id();
        self.insert_overlay_player(entity_id, player.clone(), self.now_ms);
        self.ops.push(CoreOp::PrepareLogin {
            player_id,
            player,
            expected_entity_id: entity_id,
        });
        Ok(None)
    }

    pub fn finalize_login(
        &mut self,
        connection_id: ConnectionId,
        player_id: PlayerId,
    ) -> Result<(), String> {
        if self.compose_player_snapshot(player_id).is_none() {
            return Err("cannot finalize login for missing player".to_string());
        }
        if self.player_session(player_id).is_none() {
            return Err("cannot finalize login for missing player session".to_string());
        }
        self.ops.push(CoreOp::FinalizeLogin {
            connection_id,
            player_id,
        });
        Ok(())
    }

    pub fn commit(self) -> Vec<TargetedEvent> {
        let GameplayTransaction {
            base,
            overlay: _,
            now_ms,
            ops,
        } = self;
        apply_core_ops(base, ops, now_ms, ApplyCoreOpsOptions::default())
    }

    fn online_player_count(&self) -> usize {
        let mut count = self.base.sessions.player_sessions.len();
        for (player_id, entry) in &self.overlay.player_sessions {
            match (
                self.base.sessions.player_sessions.contains_key(player_id),
                entry.is_some(),
            ) {
                (true, false) => count = count.saturating_sub(1),
                (false, true) => count = count.saturating_add(1),
                _ => {}
            }
        }
        count
    }

    fn player_entity_id(&self, player_id: PlayerId) -> Option<EntityId> {
        if let Some(entry) = self.overlay.players_by_player_id.get(&player_id) {
            return *entry;
        }
        self.base.player_entity_id(player_id)
    }

    fn player_session(&self, player_id: PlayerId) -> Option<PlayerSessionState> {
        if let Some(entry) = self.overlay.player_sessions.get(&player_id) {
            return entry.clone();
        }
        self.base.player_session(player_id).cloned()
    }

    fn ensure_player_session_mut(
        &mut self,
        player_id: PlayerId,
    ) -> Option<&mut PlayerSessionState> {
        if !self.overlay.player_sessions.contains_key(&player_id) {
            let session = self.base.player_session(player_id)?.clone();
            self.overlay
                .player_sessions
                .insert(player_id, Some(session));
        }
        self.overlay
            .player_sessions
            .get_mut(&player_id)
            .and_then(Option::as_mut)
    }

    fn compose_player_snapshot(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        let entity_id = self.player_entity_id(player_id)?;
        let identity = self.current_player_identity(entity_id)?;
        let transform = self.current_player_transform(entity_id)?;
        let vitals = self.current_player_vitals(entity_id)?;
        let inventory = self.current_player_inventory(entity_id)?;
        let selected_hotbar_slot = self.current_player_selected_hotbar(entity_id)?;
        Some(PlayerSnapshot {
            id: identity.player_id,
            username: identity.username,
            position: transform.position,
            yaw: transform.yaw,
            pitch: transform.pitch,
            on_ground: transform.on_ground,
            dimension: transform.dimension,
            health: vitals.health,
            food: vitals.food,
            food_saturation: vitals.food_saturation,
            inventory,
            selected_hotbar_slot,
        })
    }

    fn current_player_identity(&self, entity_id: EntityId) -> Option<PlayerIdentity> {
        if let Some(entry) = self.overlay.player_identity.get(&entity_id) {
            return entry.clone();
        }
        self.base.entities.player_identity.get(&entity_id).cloned()
    }

    fn current_player_transform(&self, entity_id: EntityId) -> Option<PlayerTransform> {
        if let Some(entry) = self.overlay.player_transform.get(&entity_id) {
            return *entry;
        }
        self.base.entities.player_transform.get(&entity_id).copied()
    }

    fn current_player_vitals(&self, entity_id: EntityId) -> Option<PlayerVitals> {
        if let Some(entry) = self.overlay.player_vitals.get(&entity_id) {
            return *entry;
        }
        self.base.entities.player_vitals.get(&entity_id).copied()
    }

    fn current_player_inventory(&self, entity_id: EntityId) -> Option<PlayerInventory> {
        if let Some(entry) = self.overlay.player_inventory.get(&entity_id) {
            return entry.clone();
        }
        self.base.entities.player_inventory.get(&entity_id).cloned()
    }

    fn current_player_selected_hotbar(&self, entity_id: EntityId) -> Option<u8> {
        if let Some(entry) = self.overlay.player_selected_hotbar.get(&entity_id) {
            return *entry;
        }
        self.base
            .entities
            .player_selected_hotbar
            .get(&entity_id)
            .copied()
    }

    fn current_active_mining(&self, entity_id: EntityId) -> Option<ActiveMiningState> {
        if let Some(entry) = self.overlay.player_active_mining.get(&entity_id) {
            return entry.clone();
        }
        self.base
            .entities
            .player_active_mining
            .get(&entity_id)
            .cloned()
    }

    fn ensure_player_transform_mut(&mut self, entity_id: EntityId) -> Option<&mut PlayerTransform> {
        if !self.overlay.player_transform.contains_key(&entity_id) {
            let transform = self
                .base
                .entities
                .player_transform
                .get(&entity_id)
                .copied()?;
            self.overlay
                .player_transform
                .insert(entity_id, Some(transform));
        }
        self.overlay
            .player_transform
            .get_mut(&entity_id)
            .and_then(Option::as_mut)
    }

    fn ensure_player_inventory_mut(&mut self, entity_id: EntityId) -> Option<&mut PlayerInventory> {
        if !self.overlay.player_inventory.contains_key(&entity_id) {
            let inventory = self.base.entities.player_inventory.get(&entity_id)?.clone();
            self.overlay
                .player_inventory
                .insert(entity_id, Some(inventory));
        }
        self.overlay
            .player_inventory
            .get_mut(&entity_id)
            .and_then(Option::as_mut)
    }

    fn ensure_player_selected_hotbar_mut(&mut self, entity_id: EntityId) -> Option<&mut u8> {
        if !self.overlay.player_selected_hotbar.contains_key(&entity_id) {
            let slot = *self.base.entities.player_selected_hotbar.get(&entity_id)?;
            self.overlay
                .player_selected_hotbar
                .insert(entity_id, Some(slot));
        }
        self.overlay
            .player_selected_hotbar
            .get_mut(&entity_id)
            .and_then(Option::as_mut)
    }

    fn ensure_chunk_mut(&mut self, chunk_pos: ChunkPos) -> &mut ChunkColumn {
        self.overlay.chunks.entry(chunk_pos).or_insert_with(|| {
            self.base
                .world
                .chunks
                .get(&chunk_pos)
                .cloned()
                .unwrap_or_else(|| generate_superflat_chunk(chunk_pos))
        })
    }

    fn allocate_entity_id(&mut self) -> EntityId {
        let next_entity_id = self
            .overlay
            .next_entity_id
            .get_or_insert(self.base.entities.next_entity_id);
        let entity_id = EntityId(*next_entity_id);
        *next_entity_id = next_entity_id.saturating_add(1);
        entity_id
    }

    fn insert_overlay_player(&mut self, entity_id: EntityId, player: PlayerSnapshot, now_ms: u64) {
        let player_id = player.id;
        let view = ClientView::new(
            player.position.chunk_pos(),
            self.base.world.config.view_distance,
        );
        self.overlay
            .entity_kinds
            .insert(entity_id, Some(EntityKind::Player));
        self.overlay
            .players_by_player_id
            .insert(player_id, Some(entity_id));
        self.overlay.player_identity.insert(
            entity_id,
            Some(PlayerIdentity {
                player_id,
                username: player.username.clone(),
            }),
        );
        self.overlay.player_transform.insert(
            entity_id,
            Some(PlayerTransform {
                position: player.position,
                yaw: player.yaw,
                pitch: player.pitch,
                on_ground: player.on_ground,
                dimension: player.dimension,
            }),
        );
        self.overlay.player_vitals.insert(
            entity_id,
            Some(PlayerVitals {
                health: player.health,
                food: player.food,
                food_saturation: player.food_saturation,
            }),
        );
        self.overlay
            .player_inventory
            .insert(entity_id, Some(player.inventory));
        self.overlay
            .player_selected_hotbar
            .insert(entity_id, Some(player.selected_hotbar_slot));
        self.overlay.player_active_mining.insert(entity_id, None);
        self.overlay.player_sessions.insert(
            player_id,
            Some(PlayerSessionState {
                entity_id,
                cursor: None,
                active_container: None,
                next_non_player_window_id: 1,
                view,
                pending_keep_alive_id: None,
                last_keep_alive_sent_at: None,
                next_keep_alive_at: now_ms.saturating_add(self.base.sessions.keepalive_interval_ms),
            }),
        );
    }

    fn can_edit_block_for_snapshot(&self, actor: &PlayerSnapshot, position: BlockPos) -> bool {
        if !(0..=255).contains(&position.y) {
            return false;
        }
        if distance_squared_to_block_center(actor.position, position) > BLOCK_EDIT_REACH.powi(2) {
            return false;
        }
        !self.player_entity_ids().into_iter().any(|entity_id| {
            self.current_player_transform(entity_id)
                .is_some_and(|transform| block_intersects_player(position, &transform))
        })
    }

    fn player_entity_ids(&self) -> BTreeSet<EntityId> {
        let mut entity_ids = self
            .base
            .entities
            .players_by_player_id
            .values()
            .copied()
            .collect::<BTreeSet<_>>();
        for entry in self.overlay.players_by_player_id.values() {
            match entry {
                Some(entity_id) => {
                    entity_ids.insert(*entity_id);
                }
                None => {}
            }
        }
        for (player_id, entry) in &self.overlay.players_by_player_id {
            if entry.is_none()
                && let Some(entity_id) = self.base.entities.players_by_player_id.get(player_id)
            {
                entity_ids.remove(entity_id);
            }
        }
        entity_ids
    }
}

impl ServerCore {
    pub fn begin_gameplay_transaction(&mut self, now_ms: u64) -> GameplayTransaction<'_> {
        GameplayTransaction::new(self, now_ms)
    }

    pub fn apply_builtin_gameplay_command(
        &mut self,
        command: GameplayCommand,
        now_ms: u64,
    ) -> Vec<TargetedEvent> {
        let mut tx = self.begin_gameplay_transaction(now_ms);
        apply_builtin_gameplay_command(&mut tx, &command);
        tx.commit()
    }
}

fn apply_builtin_gameplay_command(tx: &mut GameplayTransaction<'_>, command: &GameplayCommand) {
    match command {
        GameplayCommand::MoveIntent {
            player_id,
            position,
            yaw,
            pitch,
            on_ground,
        } => tx.set_player_pose(*player_id, *position, *yaw, *pitch, *on_ground),
        GameplayCommand::SetHeldSlot { player_id, slot } => set_held_slot(tx, *player_id, *slot),
        GameplayCommand::CreativeInventorySet {
            player_id,
            slot,
            stack,
        } => creative_inventory_set(tx, *player_id, *slot, stack.as_ref()),
        GameplayCommand::DigBlock {
            player_id,
            position,
            status,
            ..
        } => dig_block(tx, *player_id, *position, *status),
        GameplayCommand::PlaceBlock {
            player_id,
            hand,
            position,
            face,
            held_item,
        } => place_block(tx, *player_id, *hand, *position, *face, held_item.as_ref()),
        GameplayCommand::UseBlock {
            player_id,
            hand,
            position,
            face,
            held_item,
        } => use_block(tx, *player_id, *hand, *position, *face, held_item.as_ref()),
    }
}

fn set_held_slot(tx: &mut GameplayTransaction<'_>, player_id: PlayerId, slot: i16) {
    let Some(player) = tx.player_snapshot(player_id) else {
        return;
    };
    let Ok(slot) = u8::try_from(slot) else {
        tx.emit_event(
            EventTarget::Player(player_id),
            CoreEvent::SelectedHotbarSlotChanged {
                slot: player.selected_hotbar_slot,
            },
        );
        return;
    };
    if slot >= HOTBAR_SLOT_COUNT {
        tx.emit_event(
            EventTarget::Player(player_id),
            CoreEvent::SelectedHotbarSlotChanged {
                slot: player.selected_hotbar_slot,
            },
        );
        return;
    }
    tx.clear_mining(player_id);
    tx.set_selected_hotbar_slot(player_id, slot);
}

fn creative_inventory_set(
    tx: &mut GameplayTransaction<'_>,
    player_id: PlayerId,
    slot: InventorySlot,
    stack: Option<&ItemStack>,
) {
    let Some(player) = tx.player_snapshot(player_id) else {
        return;
    };
    if tx.world_meta().game_mode != 1
        || !slot.is_storage_slot()
        || stack.is_some_and(|stack| {
            !stack.is_supported_inventory_item() || stack.count == 0 || stack.count > 64
        })
    {
        reject_inventory_slot_events(tx, player_id, slot, &player);
        return;
    }
    tx.set_inventory_slot(player_id, slot, stack.cloned());
}

fn dig_block(
    tx: &mut GameplayTransaction<'_>,
    player_id: PlayerId,
    position: BlockPos,
    status: u8,
) {
    if !matches!(status, 0..=2) {
        return;
    }
    let Some(player) = tx.player_snapshot(player_id) else {
        return;
    };
    if status == 1 {
        tx.clear_mining(player_id);
        return;
    }
    let current = tx.block_state(position);
    let protected_container = matches!(current.key.as_str(), catalog::CHEST | catalog::FURNACE)
        && tx
            .block_entity(position)
            .is_some_and(|entity| entity.has_inventory_contents());
    if !tx.can_edit_block(player_id, position)
        || current.is_air()
        || current.key.as_str() == catalog::BEDROCK
        || protected_container
    {
        tx.clear_mining(player_id);
        tx.emit_event(
            EventTarget::Player(player_id),
            CoreEvent::BlockChanged {
                position,
                block: current,
            },
        );
        return;
    }
    if tx.world_meta().game_mode == 1 {
        tx.clear_mining(player_id);
        tx.set_block(position, BlockState::air());
        return;
    }
    let duration_ms = catalog::survival_mining_duration_ms(
        &current,
        catalog::tool_spec_for_item(
            player
                .inventory
                .selected_hotbar_stack(player.selected_hotbar_slot),
        ),
    )
    .unwrap_or(50);
    tx.begin_mining(player_id, position, duration_ms);
}

fn place_block(
    tx: &mut GameplayTransaction<'_>,
    player_id: PlayerId,
    hand: InteractionHand,
    position: BlockPos,
    face: Option<BlockFace>,
    held_item: Option<&ItemStack>,
) {
    let Some(face) = face else {
        return;
    };
    let Some(player) = tx.player_snapshot(player_id) else {
        return;
    };
    let place_pos = position.offset(face);
    let Some(selected_stack) = player
        .inventory
        .selected_stack(player.selected_hotbar_slot, hand)
        .cloned()
    else {
        place_rejection(tx, player_id, hand, place_pos, &player);
        return;
    };
    if held_item.is_some_and(|held_item| held_item != &selected_stack) {
        place_rejection(tx, player_id, hand, place_pos, &player);
        return;
    }
    let Some(block) = catalog::placeable_block_state_from_item_key(selected_stack.key.as_str())
    else {
        place_rejection(tx, player_id, hand, place_pos, &player);
        return;
    };
    if !tx.can_edit_block(player_id, place_pos)
        || tx.block_state(position).is_air()
        || !tx.block_state(place_pos).is_air()
    {
        place_rejection(tx, player_id, hand, place_pos, &player);
        return;
    }
    tx.clear_mining(player_id);
    tx.set_block(place_pos, block);
    if tx.world_meta().game_mode != 1 {
        tx.set_inventory_slot(
            player_id,
            held_inventory_slot(player.selected_hotbar_slot, hand),
            consumed_stack(&selected_stack),
        );
    }
}

fn use_block(
    tx: &mut GameplayTransaction<'_>,
    player_id: PlayerId,
    hand: InteractionHand,
    position: BlockPos,
    face: Option<BlockFace>,
    held_item: Option<&ItemStack>,
) {
    let target_block = tx.block_state(position);
    if target_block.key.as_str() == catalog::CHEST {
        if !tx.can_edit_block(player_id, position) {
            tx.emit_event(
                EventTarget::Player(player_id),
                CoreEvent::BlockChanged {
                    position,
                    block: target_block,
                },
            );
            return;
        }
        tx.clear_mining(player_id);
        tx.open_chest(player_id, position);
        return;
    }
    if target_block.key.as_str() == catalog::FURNACE {
        if !tx.can_edit_block(player_id, position) {
            tx.emit_event(
                EventTarget::Player(player_id),
                CoreEvent::BlockChanged {
                    position,
                    block: target_block,
                },
            );
            return;
        }
        tx.clear_mining(player_id);
        tx.open_furnace(player_id, position);
        return;
    }
    place_block(tx, player_id, hand, position, face, held_item);
}

fn reject_inventory_slot_events(
    tx: &mut GameplayTransaction<'_>,
    player_id: PlayerId,
    slot: InventorySlot,
    player: &PlayerSnapshot,
) {
    tx.emit_event(
        EventTarget::Player(player_id),
        CoreEvent::InventorySlotChanged {
            window_id: 0,
            container: InventoryContainer::Player,
            slot,
            stack: player.inventory.get_slot(slot).cloned(),
        },
    );
    tx.emit_event(
        EventTarget::Player(player_id),
        CoreEvent::SelectedHotbarSlotChanged {
            slot: player.selected_hotbar_slot,
        },
    );
}

fn place_rejection(
    tx: &mut GameplayTransaction<'_>,
    player_id: PlayerId,
    hand: InteractionHand,
    place_pos: BlockPos,
    player: &PlayerSnapshot,
) {
    tx.emit_event(
        EventTarget::Player(player_id),
        CoreEvent::BlockChanged {
            position: place_pos,
            block: tx.block_state(place_pos),
        },
    );
    for event in ServerCore::place_inventory_correction(player_id, hand, player) {
        tx.emit_event(event.target, event.event);
    }
}

fn held_inventory_slot(selected_hotbar_slot: u8, hand: InteractionHand) -> InventorySlot {
    match hand {
        InteractionHand::Main => InventorySlot::Hotbar(selected_hotbar_slot),
        InteractionHand::Offhand => InventorySlot::Offhand,
    }
}

fn consumed_stack(stack: &ItemStack) -> Option<ItemStack> {
    let mut stack = stack.clone();
    stack.count = stack.count.saturating_sub(1);
    (stack.count > 0).then_some(stack)
}

fn local_block_x(position: BlockPos) -> u8 {
    u8::try_from(position.x.rem_euclid(crate::CHUNK_WIDTH)).expect("local x should fit into u8")
}

fn local_block_z(position: BlockPos) -> u8 {
    u8::try_from(position.z.rem_euclid(crate::CHUNK_WIDTH)).expect("local z should fit into u8")
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
