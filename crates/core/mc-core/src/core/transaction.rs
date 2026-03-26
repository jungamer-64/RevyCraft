use super::{
    ServerCore,
    canonical::{
        AppliedCoreOp, ApplyCoreOpsOptions, CoreOp, build_applied_core_events, reduce_core_op,
    },
    login::default_player,
    state_backend::{CoreStateMut, CoreStateRead, OverlayState, OverlayStateRef, TxOverlay},
};
use crate::catalog;
use crate::events::{CoreEvent, EventTarget, GameplayCommand, TargetedEvent};
use crate::inventory::{InventoryContainer, InventorySlot, ItemStack};
use crate::player::{InteractionHand, PlayerSnapshot};
use crate::world::{BlockEntityState, BlockFace, BlockPos, BlockState, Vec3, WorldMeta};
use crate::{ConnectionId, HOTBAR_SLOT_COUNT, PlayerId};
use std::collections::{BTreeMap, BTreeSet};

pub struct GameplayTransaction<'a> {
    base: &'a mut ServerCore,
    overlay: TxOverlay,
    now_ms: u64,
    prepared_players: BTreeMap<PlayerId, crate::EntityId>,
    finalized_players: BTreeSet<PlayerId>,
    applied_ops: Vec<AppliedCoreOp>,
}

impl<'a> GameplayTransaction<'a> {
    pub fn new(base: &'a mut ServerCore, now_ms: u64) -> Self {
        Self {
            base,
            overlay: TxOverlay::default(),
            now_ms,
            prepared_players: BTreeMap::new(),
            finalized_players: BTreeSet::new(),
            applied_ops: Vec::new(),
        }
    }

    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    #[must_use]
    pub fn world_meta(&self) -> WorldMeta {
        self.overlay_view().world_meta()
    }

    #[must_use]
    pub fn player_snapshot(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        self.overlay_view().compose_player_snapshot(player_id)
    }

    #[must_use]
    pub fn block_state(&self, position: BlockPos) -> BlockState {
        self.overlay_view().block_state(position)
    }

    #[must_use]
    pub fn block_entity(&self, position: BlockPos) -> Option<BlockEntityState> {
        self.overlay_view().block_entity(position)
    }

    #[must_use]
    pub fn can_edit_block(&self, player_id: PlayerId, position: BlockPos) -> bool {
        self.overlay_view()
            .compose_player_snapshot(player_id)
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
        self.push_previewed_op(CoreOp::SetPlayerPose {
            player_id,
            position,
            yaw,
            pitch,
            on_ground,
        });
    }

    pub fn set_selected_hotbar_slot(&mut self, player_id: PlayerId, slot: u8) {
        self.push_previewed_op(CoreOp::SetSelectedHotbarSlot { player_id, slot });
    }

    pub fn set_inventory_slot(
        &mut self,
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    ) {
        self.push_previewed_op(CoreOp::SetInventorySlot {
            player_id,
            slot,
            stack,
        });
    }

    pub fn clear_mining(&mut self, player_id: PlayerId) {
        self.push_previewed_op(CoreOp::ClearMining { player_id });
    }

    pub fn begin_mining(&mut self, player_id: PlayerId, position: BlockPos, duration_ms: u64) {
        self.push_previewed_op(CoreOp::BeginMining {
            player_id,
            position,
            duration_ms,
        });
    }

    pub fn open_chest(&mut self, player_id: PlayerId, position: BlockPos) {
        self.push_previewed_op(CoreOp::OpenChest {
            player_id,
            position,
        });
    }

    pub fn open_furnace(&mut self, player_id: PlayerId, position: BlockPos) {
        self.push_previewed_op(CoreOp::OpenFurnace {
            player_id,
            position,
        });
    }

    pub fn set_block(&mut self, position: BlockPos, block: BlockState) {
        self.push_previewed_op(CoreOp::SetBlock { position, block });
    }

    pub fn spawn_dropped_item(&mut self, position: Vec3, item: ItemStack) {
        self.push_previewed_op(CoreOp::SpawnDroppedItem {
            expected_entity_id: None,
            position,
            item,
        });
    }

    pub fn emit_event(&mut self, target: EventTarget, event: CoreEvent) {
        self.push_previewed_op(CoreOp::EmitEvent { target, event });
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
        if self.overlay_view().online_player_count()
            >= usize::from(self.base.world.config.max_players)
        {
            return Ok(Some(ServerCore::reject_connection(
                connection_id,
                "Server is full",
            )));
        }
        if self.overlay_view().player_entity_id(player_id).is_some() {
            return Ok(Some(ServerCore::reject_connection(
                connection_id,
                "Player is already online",
            )));
        }

        let mut player = self
            .overlay_view()
            .saved_player(player_id)
            .unwrap_or_else(|| {
                default_player(player_id, username.clone(), self.base.world.config.spawn)
            });
        player.username = username;
        ServerCore::recompute_crafting_result_for_inventory(&mut player.inventory);

        let entity_id = {
            let mut state = self.overlay_state();
            state.allocate_entity_id()
        };
        let mut materialized_players = self.materialized_preview_players();
        materialized_players.insert(player_id);
        let now_ms = self.now_ms;
        let applied = {
            let mut state = self.overlay_state();
            reduce_core_op(
                &mut state,
                CoreOp::PrepareLogin {
                    player_id,
                    player,
                    expected_entity_id: entity_id,
                },
                now_ms,
                &materialized_players,
            )
        };
        self.prepared_players.insert(player_id, entity_id);
        self.applied_ops.push(applied);
        Ok(None)
    }

    pub fn finalize_login(
        &mut self,
        connection_id: ConnectionId,
        player_id: PlayerId,
    ) -> Result<(), String> {
        if self
            .overlay_view()
            .compose_player_snapshot(player_id)
            .is_none()
        {
            return Err("cannot finalize login for missing player".to_string());
        }
        if self.overlay_view().player_session(player_id).is_none() {
            return Err("cannot finalize login for missing player session".to_string());
        }
        self.finalized_players.insert(player_id);
        self.push_previewed_op(CoreOp::FinalizeLogin {
            connection_id,
            player_id,
        });
        Ok(())
    }

    pub fn commit(self) -> Vec<TargetedEvent> {
        let GameplayTransaction {
            base,
            overlay,
            now_ms: _,
            prepared_players,
            finalized_players,
            applied_ops,
        } = self;
        overlay.materialize_into(base, &prepared_players, &finalized_players);
        build_applied_core_events(base, applied_ops, ApplyCoreOpsOptions::default())
    }

    fn overlay_view(&self) -> OverlayStateRef<'_> {
        OverlayStateRef::new(&*self.base, &self.overlay)
    }

    fn overlay_state(&mut self) -> OverlayState<'_> {
        OverlayState::new(self.base, &mut self.overlay)
    }

    fn materialized_preview_players(&self) -> BTreeSet<PlayerId> {
        self.prepared_players.keys().copied().collect()
    }

    fn push_previewed_op(&mut self, op: CoreOp) {
        let materialized_players = self.materialized_preview_players();
        let now_ms = self.now_ms;
        let applied = {
            let mut state = self.overlay_state();
            reduce_core_op(&mut state, op, now_ms, &materialized_players)
        };
        self.applied_ops.push(applied);
    }

    fn can_edit_block_for_snapshot(&self, actor: &PlayerSnapshot, position: BlockPos) -> bool {
        self.overlay_view()
            .can_edit_block_for_snapshot(actor, position)
    }

    #[cfg(test)]
    pub(crate) fn dropped_item_ids(&self) -> Vec<crate::EntityId> {
        self.overlay_view().dropped_item_ids()
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
