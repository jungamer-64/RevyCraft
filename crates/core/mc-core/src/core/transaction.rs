use super::{
    ServerCore,
    canonical::{ApplyCoreOpsOptions, CoreOp, apply_core_ops, reduce_core_op},
    login::default_player,
    state_backend::{
        BaseStateRef, CoreStateMut, CoreStateRead, OverlayState, OverlayStateRef, TxOverlay,
    },
};
use crate::catalog;
use crate::events::{CoreEvent, EventTarget, GameplayCommand, TargetedEvent};
use crate::inventory::{InventoryContainer, InventorySlot, ItemStack};
use crate::player::{InteractionHand, PlayerSnapshot};
use crate::world::{BlockEntityState, BlockFace, BlockPos, BlockState, Vec3, WorldMeta};
use crate::{ConnectionId, HOTBAR_SLOT_COUNT, PlayerId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
struct GameplayReadSet {
    world_meta: Option<WorldMeta>,
    player_snapshots: BTreeMap<PlayerId, Option<PlayerSnapshot>>,
    block_states: BTreeMap<BlockPos, BlockState>,
    block_entities: BTreeMap<BlockPos, Option<BlockEntityState>>,
    can_edit_block: BTreeMap<(PlayerId, BlockPos), bool>,
    online_player_count: Option<usize>,
    saved_players: BTreeMap<PlayerId, Option<PlayerSnapshot>>,
    player_entity_ids: BTreeMap<PlayerId, Option<crate::EntityId>>,
    next_entity_id: Option<i32>,
}

impl GameplayReadSet {
    fn matches(&self, core: &ServerCore) -> bool {
        let view = BaseStateRef::new(core);
        if let Some(world_meta) = &self.world_meta
            && &view.world_meta() != world_meta
        {
            return false;
        }
        for (player_id, snapshot) in &self.player_snapshots {
            if &view.compose_player_snapshot(*player_id) != snapshot {
                return false;
            }
        }
        for (position, block_state) in &self.block_states {
            if &view.block_state(*position) != block_state {
                return false;
            }
        }
        for (position, block_entity) in &self.block_entities {
            if &view.block_entity(*position) != block_entity {
                return false;
            }
        }
        for ((player_id, position), allowed) in &self.can_edit_block {
            let current = view
                .compose_player_snapshot(*player_id)
                .is_some_and(|player| view.can_edit_block_for_snapshot(&player, *position));
            if &current != allowed {
                return false;
            }
        }
        if let Some(online_player_count) = self.online_player_count
            && view.online_player_count() != online_player_count
        {
            return false;
        }
        for (player_id, saved_player) in &self.saved_players {
            if &view.saved_player(*player_id) != saved_player {
                return false;
            }
        }
        for (player_id, entity_id) in &self.player_entity_ids {
            if &view.player_entity_id(*player_id) != entity_id {
                return false;
            }
        }
        if let Some(next_entity_id) = self.next_entity_id
            && core.entities.next_entity_id != next_entity_id
        {
            return false;
        }
        true
    }
}

#[derive(Clone, Debug)]
pub struct GameplayJournal {
    now_ms: u64,
    ops: Vec<CoreOp>,
    read_set: GameplayReadSet,
}

impl GameplayJournal {
    #[must_use]
    pub fn empty(now_ms: u64) -> Self {
        Self {
            now_ms,
            ops: Vec::new(),
            read_set: GameplayReadSet::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GameplayJournalApplyResult {
    Applied(Vec<TargetedEvent>),
    Conflict,
}

pub struct GameplayTransaction<'a> {
    live_base: Option<&'a mut ServerCore>,
    snapshot: ServerCore,
    overlay: TxOverlay,
    now_ms: u64,
    prepared_players: BTreeMap<PlayerId, crate::EntityId>,
    finalized_players: BTreeSet<PlayerId>,
    journal: Vec<CoreOp>,
    read_set: GameplayReadSet,
}

impl<'a> GameplayTransaction<'a> {
    pub fn new(base: &'a mut ServerCore, now_ms: u64) -> Self {
        Self {
            snapshot: base.clone(),
            live_base: Some(base),
            overlay: TxOverlay::default(),
            now_ms,
            prepared_players: BTreeMap::new(),
            finalized_players: BTreeSet::new(),
            journal: Vec::new(),
            read_set: GameplayReadSet::default(),
        }
    }

    #[must_use]
    pub fn detached(snapshot: ServerCore, now_ms: u64) -> GameplayTransaction<'static> {
        GameplayTransaction {
            live_base: None,
            snapshot,
            overlay: TxOverlay::default(),
            now_ms,
            prepared_players: BTreeMap::new(),
            finalized_players: BTreeSet::new(),
            journal: Vec::new(),
            read_set: GameplayReadSet::default(),
        }
    }

    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    #[must_use]
    pub fn world_meta(&mut self) -> WorldMeta {
        let world_meta = self.overlay_view().world_meta();
        let _ = self
            .read_set
            .world_meta
            .get_or_insert_with(|| world_meta.clone());
        world_meta
    }

    #[must_use]
    pub fn player_snapshot(&mut self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        let snapshot = self.overlay_view().compose_player_snapshot(player_id);
        if !self.overlay_touches_player(player_id) {
            let _ = self
                .read_set
                .player_snapshots
                .entry(player_id)
                .or_insert_with(|| snapshot.clone());
        }
        snapshot
    }

    #[must_use]
    pub fn block_state(&mut self, position: BlockPos) -> BlockState {
        let block_state = self.overlay_view().block_state(position);
        if !self.overlay_touches_block(position) {
            let _ = self
                .read_set
                .block_states
                .entry(position)
                .or_insert_with(|| block_state.clone());
        }
        block_state
    }

    #[must_use]
    pub fn block_entity(&mut self, position: BlockPos) -> Option<BlockEntityState> {
        let block_entity = self.overlay_view().block_entity(position);
        if !self.overlay_touches_block_entity(position) {
            let _ = self
                .read_set
                .block_entities
                .entry(position)
                .or_insert_with(|| block_entity.clone());
        }
        block_entity
    }

    #[must_use]
    pub fn can_edit_block(&mut self, player_id: PlayerId, position: BlockPos) -> bool {
        let allowed = self
            .overlay_view()
            .compose_player_snapshot(player_id)
            .is_some_and(|player| self.can_edit_block_for_snapshot(&player, position));
        if !self.overlay_may_affect_can_edit_block(player_id, position) {
            let _ = self
                .read_set
                .can_edit_block
                .entry((player_id, position))
                .or_insert(allowed);
        }
        allowed
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
        let online_player_count = self.overlay_view().online_player_count();
        let _ = self
            .read_set
            .online_player_count
            .get_or_insert(online_player_count);
        if online_player_count >= usize::from(self.snapshot.world.config.max_players) {
            return Ok(Some(ServerCore::reject_connection(
                connection_id,
                "Server is full",
            )));
        }
        let existing_entity_id = self.overlay_view().player_entity_id(player_id);
        let _ = self
            .read_set
            .player_entity_ids
            .entry(player_id)
            .or_insert(existing_entity_id);
        if existing_entity_id.is_some() {
            return Ok(Some(ServerCore::reject_connection(
                connection_id,
                "Player is already online",
            )));
        }

        let saved_player = self.overlay_view().saved_player(player_id);
        let _ = self
            .read_set
            .saved_players
            .entry(player_id)
            .or_insert_with(|| saved_player.clone());
        let mut player = saved_player.unwrap_or_else(|| {
            default_player(
                player_id,
                username.clone(),
                self.snapshot.world.config.spawn,
            )
        });
        player.username = username;
        ServerCore::recompute_crafting_result_for_inventory(&mut player.inventory);

        let _ = self
            .read_set
            .next_entity_id
            .get_or_insert(self.current_next_entity_id());
        let entity_id = {
            let mut state = self.overlay_state();
            state.allocate_entity_id()
        };
        let mut materialized_players = self.materialized_preview_players();
        materialized_players.insert(player_id);
        let now_ms = self.now_ms;
        let prepare_login = CoreOp::PrepareLogin {
            player_id,
            player,
            expected_entity_id: entity_id,
        };
        {
            let mut state = self.overlay_state();
            reduce_core_op(
                &mut state,
                prepare_login.clone(),
                now_ms,
                &materialized_players,
            );
        }
        self.prepared_players.insert(player_id, entity_id);
        self.journal.push(prepare_login);
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
            live_base,
            snapshot: _,
            overlay: _,
            now_ms,
            prepared_players: _,
            finalized_players: _,
            journal,
            read_set,
        } = self;
        let live_base = live_base.expect(
            "committing a detached gameplay transaction requires extracting the journal first",
        );
        match live_base.validate_and_apply_gameplay_journal(GameplayJournal {
            now_ms,
            ops: journal,
            read_set,
        }) {
            GameplayJournalApplyResult::Applied(events) => events,
            GameplayJournalApplyResult::Conflict => {
                panic!("gameplay transaction conflicted while committing to an exclusive core")
            }
        }
    }

    #[must_use]
    pub fn into_journal(self) -> GameplayJournal {
        let GameplayTransaction {
            live_base: _,
            snapshot: _,
            overlay: _,
            now_ms,
            prepared_players: _,
            finalized_players: _,
            journal,
            read_set,
        } = self;
        GameplayJournal {
            now_ms,
            ops: journal,
            read_set,
        }
    }

    fn overlay_view(&self) -> OverlayStateRef<'_> {
        OverlayStateRef::new(&self.snapshot, &self.overlay)
    }

    fn overlay_state(&mut self) -> OverlayState<'_> {
        OverlayState::new(&mut self.snapshot, &mut self.overlay)
    }

    fn materialized_preview_players(&self) -> BTreeSet<PlayerId> {
        self.prepared_players.keys().copied().collect()
    }

    fn push_previewed_op(&mut self, op: CoreOp) {
        let materialized_players = self.materialized_preview_players();
        let now_ms = self.now_ms;
        {
            let mut state = self.overlay_state();
            reduce_core_op(&mut state, op.clone(), now_ms, &materialized_players);
        }
        self.journal.push(op);
    }

    fn can_edit_block_for_snapshot(&self, actor: &PlayerSnapshot, position: BlockPos) -> bool {
        self.overlay_view()
            .can_edit_block_for_snapshot(actor, position)
    }

    fn current_next_entity_id(&self) -> i32 {
        self.overlay
            .next_entity_id
            .unwrap_or(self.snapshot.entities.next_entity_id)
    }

    fn overlay_touches_block(&self, position: BlockPos) -> bool {
        self.overlay.chunks.contains_key(&position.chunk_pos())
    }

    fn overlay_touches_block_entity(&self, position: BlockPos) -> bool {
        self.overlay.block_entities.contains_key(&position) || self.overlay_touches_block(position)
    }

    fn overlay_touches_player(&self, player_id: PlayerId) -> bool {
        if self.overlay.saved_players.contains_key(&player_id)
            || self.overlay.players_by_player_id.contains_key(&player_id)
            || self.overlay.player_sessions.contains_key(&player_id)
        {
            return true;
        }
        let entity_id = self
            .overlay
            .players_by_player_id
            .get(&player_id)
            .copied()
            .flatten()
            .or_else(|| self.snapshot.player_entity_id(player_id));
        let Some(entity_id) = entity_id else {
            return false;
        };
        self.overlay.player_identity.contains_key(&entity_id)
            || self.overlay.player_transform.contains_key(&entity_id)
            || self.overlay.player_vitals.contains_key(&entity_id)
            || self.overlay.player_inventory.contains_key(&entity_id)
            || self.overlay.player_selected_hotbar.contains_key(&entity_id)
            || self.overlay.player_active_mining.contains_key(&entity_id)
    }

    fn overlay_may_affect_can_edit_block(&self, player_id: PlayerId, position: BlockPos) -> bool {
        self.overlay_touches_player(player_id)
            || self.overlay_touches_block(position)
            || !self.overlay.player_transform.is_empty()
            || !self.overlay.players_by_player_id.is_empty()
            || !self.overlay.entity_kinds.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn dropped_item_ids(&self) -> Vec<crate::EntityId> {
        self.overlay_view().dropped_item_ids()
    }

    #[cfg(test)]
    pub(crate) fn request_keep_alive(&mut self, player_id: PlayerId) {
        self.push_previewed_op(CoreOp::RequestKeepAlive { player_id });
    }

    #[cfg(test)]
    pub(crate) fn acknowledge_keep_alive(&mut self, player_id: PlayerId, keep_alive_id: i32) {
        self.push_previewed_op(CoreOp::AcknowledgeKeepAlive {
            player_id,
            keep_alive_id,
        });
    }

    #[cfg(test)]
    pub(crate) fn player_session_state(
        &self,
        player_id: PlayerId,
    ) -> Option<super::PlayerSessionState> {
        self.overlay_view().player_session(player_id)
    }

    #[cfg(test)]
    pub(crate) fn next_keep_alive_id(&self) -> i32 {
        self.overlay
            .next_keep_alive_id
            .unwrap_or(self.snapshot.sessions.next_keep_alive_id)
    }
}

impl ServerCore {
    pub fn begin_gameplay_transaction(&mut self, now_ms: u64) -> GameplayTransaction<'_> {
        GameplayTransaction::new(self, now_ms)
    }

    pub fn validate_and_apply_gameplay_journal(
        &mut self,
        journal: GameplayJournal,
    ) -> GameplayJournalApplyResult {
        if !journal.read_set.matches(self) {
            return GameplayJournalApplyResult::Conflict;
        }
        GameplayJournalApplyResult::Applied(apply_core_ops(
            self,
            journal.ops,
            journal.now_ms,
            ApplyCoreOpsOptions::default(),
        ))
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
    let block = tx.block_state(place_pos);
    tx.emit_event(
        EventTarget::Player(player_id),
        CoreEvent::BlockChanged {
            position: place_pos,
            block,
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
