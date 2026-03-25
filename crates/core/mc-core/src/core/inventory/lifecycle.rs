use super::super::canonical::{
    ApplyCoreOpsOptions, CloseContainerDelta, CoreOp, DroppedItemTickDelta, EntityDespawnDelta,
    OpenContainerDelta, WindowDiffDelta, apply_core_ops,
};
use super::super::{DroppedItemState, PlayerSessionState, ServerCore};
use super::crafting::{
    recompute_crafting_result_for_active_container, recompute_player_crafting_result,
};
use super::furnace::{normalize_furnace_window, tick_furnace_window};
use super::state::{
    CHEST_LOCAL_SLOT_COUNT, CRAFTING_TABLE_LOCAL_SLOT_COUNT, ChestWindowState, FurnaceWindowState,
    OpenInventoryWindow, OpenInventoryWindowState,
};
use super::util::merge_stack_into_player_inventory;
use crate::catalog;
use crate::events::TargetedEvent;
use crate::inventory::{
    InventoryContainer, InventorySlot, InventoryWindowContents, ItemStack, PlayerInventory,
};
use crate::world::{BlockEntityState, BlockPos, BlockState, Vec3};
use crate::{EntityId, PlayerId, PlayerSnapshot};

const DROPPED_ITEM_PICKUP_RADIUS_SQUARED: f64 = 1.5 * 1.5;
const DROPPED_ITEM_PHYSICS_STEP_MS: u64 = 50;
const DROPPED_ITEM_GRAVITY_PER_STEP: f64 = 0.04;
const DROPPED_ITEM_DRAG: f64 = 0.98;
const DROPPED_ITEM_REST_HEIGHT: f64 = 0.25;

impl ServerCore {
    pub fn open_crafting_table(
        &mut self,
        player_id: PlayerId,
        window_id: u8,
        title: impl Into<String>,
    ) -> Vec<TargetedEvent> {
        apply_core_ops(
            self,
            vec![CoreOp::OpenWindow {
                player_id,
                window: OpenInventoryWindow {
                    window_id,
                    container: InventoryContainer::CraftingTable,
                    state: OpenInventoryWindowState::CraftingTable {
                        slots: vec![None; CRAFTING_TABLE_LOCAL_SLOT_COUNT],
                    },
                },
                title: title.into(),
            }],
            0,
            ApplyCoreOpsOptions::default(),
        )
    }

    pub fn open_furnace(
        &mut self,
        player_id: PlayerId,
        window_id: u8,
        title: impl Into<String>,
    ) -> Vec<TargetedEvent> {
        apply_core_ops(
            self,
            vec![CoreOp::OpenWindow {
                player_id,
                window: OpenInventoryWindow {
                    window_id,
                    container: InventoryContainer::Furnace,
                    state: OpenInventoryWindowState::Furnace(FurnaceWindowState::new_virtual()),
                },
                title: title.into(),
            }],
            0,
            ApplyCoreOpsOptions::default(),
        )
    }

    pub fn open_chest(
        &mut self,
        player_id: PlayerId,
        window_id: u8,
        title: impl Into<String>,
    ) -> Vec<TargetedEvent> {
        apply_core_ops(
            self,
            vec![CoreOp::OpenWindow {
                player_id,
                window: OpenInventoryWindow {
                    window_id,
                    container: InventoryContainer::Chest,
                    state: OpenInventoryWindowState::Chest(ChestWindowState::new_virtual(
                        CHEST_LOCAL_SLOT_COUNT,
                    )),
                },
                title: title.into(),
            }],
            0,
            ApplyCoreOpsOptions::default(),
        )
    }

    pub(in crate::core) fn state_open_world_chest(
        &mut self,
        player_id: PlayerId,
        position: BlockPos,
    ) -> Option<OpenContainerDelta> {
        if self.block_at(position).key.as_str() != catalog::CHEST {
            return None;
        }
        let slots = self
            .world
            .block_entities
            .entry(position)
            .or_insert_with(|| BlockEntityState::chest(CHEST_LOCAL_SLOT_COUNT))
            .chest_slots()
            .expect("chest block entity should expose slots")
            .to_vec();
        let Some(window_id) = self.allocate_non_player_window_id(player_id) else {
            return None;
        };
        self.state_open_non_player_window(
            player_id,
            OpenInventoryWindow {
                window_id,
                container: InventoryContainer::Chest,
                state: OpenInventoryWindowState::Chest(ChestWindowState::new_block(
                    position, slots,
                )),
            },
            "Chest".to_string(),
        )
    }

    pub(in crate::core) fn state_open_world_furnace(
        &mut self,
        player_id: PlayerId,
        position: BlockPos,
    ) -> Option<OpenContainerDelta> {
        if self.block_at(position).key.as_str() != catalog::FURNACE {
            return None;
        }
        let block_entity = self
            .world
            .block_entities
            .entry(position)
            .or_insert_with(BlockEntityState::furnace)
            .clone();
        let Some(window_id) = self.allocate_non_player_window_id(player_id) else {
            return None;
        };
        self.state_open_non_player_window(
            player_id,
            OpenInventoryWindow {
                window_id,
                container: InventoryContainer::Furnace,
                state: OpenInventoryWindowState::Furnace(FurnaceWindowState::new_block(
                    position,
                    &block_entity,
                )),
            },
            "Furnace".to_string(),
        )
    }

    pub(in crate::core) fn state_close_inventory_window(
        &mut self,
        player_id: PlayerId,
        window_id: u8,
    ) -> Option<CloseContainerDelta> {
        if window_id == 0 {
            return None;
        }
        let Some(active_window_id) = self.player_session(player_id).and_then(|session| {
            session
                .active_container
                .as_ref()
                .map(|window| window.window_id)
        }) else {
            return None;
        };
        if active_window_id != window_id {
            return None;
        }
        self.state_close_player_active_container(player_id, true)
    }

    pub(in crate::core) fn persisted_online_player_snapshot(
        &self,
        player_id: PlayerId,
    ) -> Option<PlayerSnapshot> {
        let snapshot = self.compose_player_snapshot(player_id)?;
        let session = self.player_session(player_id)?;
        Some(persist_live_player_state(
            &snapshot,
            session.cursor.as_ref(),
            session.active_container.as_ref(),
        ))
    }

    pub(in crate::core) fn recompute_crafting_result_for_inventory(
        inventory: &mut PlayerInventory,
    ) {
        recompute_player_crafting_result(inventory);
    }

    pub(in crate::core) fn state_tick_active_container(
        &mut self,
        player_id: PlayerId,
    ) -> Option<WindowDiffDelta> {
        let before_session = self.player_session(player_id)?;
        let before_window = before_session.active_container.as_ref()?;
        if before_window.container != InventoryContainer::Furnace {
            return None;
        }
        let before_inventory = self.player_inventory(player_id)?;
        let before_contents = before_window.contents(before_inventory);
        let before_properties = before_window.property_entries();
        let window_id = before_window.window_id;

        let (after_contents, after_properties) = {
            let entity_id = self.player_entity_id(player_id)?;
            let session = self.sessions.player_sessions.get_mut(&player_id)?;
            let inventory = self.entities.player_inventory.get_mut(&entity_id)?;
            match session.active_container.as_mut() {
                Some(window) if window.container == InventoryContainer::Furnace => {
                    tick_furnace_window(window);
                    (window.contents(inventory), window.property_entries())
                }
                _ => return None,
            }
        };

        if let Some((position, block_entity)) = self
            .player_session(player_id)
            .and_then(|session| session.active_container.as_ref())
            .and_then(OpenInventoryWindow::world_block_entity)
            && matches!(block_entity, BlockEntityState::Furnace { .. })
            && self.block_at(position).key.as_str() == catalog::FURNACE
        {
            self.world.block_entities.insert(position, block_entity);
        }

        Some(WindowDiffDelta {
            player_id,
            window_id,
            container: InventoryContainer::Furnace,
            before_contents,
            after_contents,
            before_properties,
            after_properties,
        })
    }

    fn advance_dropped_item_entity(&self, item: &mut DroppedItemState, now_ms: u64) {
        let elapsed_ms = now_ms.saturating_sub(item.last_updated_at_ms);
        let step_count = elapsed_ms / DROPPED_ITEM_PHYSICS_STEP_MS;
        if step_count == 0 {
            return;
        }

        for _ in 0..step_count {
            self.advance_dropped_item_step(item);
        }

        item.last_updated_at_ms = item
            .last_updated_at_ms
            .saturating_add(step_count.saturating_mul(DROPPED_ITEM_PHYSICS_STEP_MS));
    }

    fn advance_dropped_item_step(&self, item: &mut DroppedItemState) {
        let next_velocity_y =
            (item.snapshot.velocity.y - DROPPED_ITEM_GRAVITY_PER_STEP) * DROPPED_ITEM_DRAG;
        let next_y = item.snapshot.position.y + next_velocity_y;
        if let Some(rest_y) =
            self.dropped_item_rest_y(item.snapshot.position.x, next_y, item.snapshot.position.z)
            && next_y <= rest_y
        {
            item.snapshot.position.y = rest_y;
            item.snapshot.velocity.y = 0.0;
            return;
        }

        item.snapshot.position.y = next_y;
        item.snapshot.velocity.y = next_velocity_y;
    }

    fn dropped_item_rest_y(&self, x: f64, y: f64, z: f64) -> Option<f64> {
        let block_x = x.floor() as i32;
        let block_z = z.floor() as i32;
        let max_block_y = (y.floor() as i32).clamp(0, 255);
        for block_y in (0..=max_block_y).rev() {
            let position = BlockPos::new(block_x, block_y, block_z);
            if self.block_at(position).is_air() {
                continue;
            }
            return Some(f64::from(block_y) + 1.0 + DROPPED_ITEM_REST_HEIGHT);
        }
        None
    }

    pub(in crate::core) fn unregister_world_chest_viewer(
        &mut self,
        position: BlockPos,
        player_id: PlayerId,
    ) {
        let Some(viewers) = self.world.chest_viewers.get_mut(&position) else {
            return;
        };
        viewers.remove(&player_id);
        if viewers.is_empty() {
            self.world.chest_viewers.remove(&position);
        }
    }

    fn state_merge_stack_into_online_player_inventory(
        &mut self,
        player_id: PlayerId,
        stack: ItemStack,
    ) -> (Option<WindowDiffDelta>, Option<ItemStack>) {
        let Some(before_session) = self.player_session(player_id) else {
            return (None, Some(stack));
        };
        let Some(before_inventory) = self.player_inventory(player_id) else {
            return (None, Some(stack));
        };
        let (window_id, container) = before_session
            .active_container
            .as_ref()
            .map(|window| (window.window_id, window.container))
            .unwrap_or((0, InventoryContainer::Player));
        let before_contents = before_session
            .active_container
            .as_ref()
            .map(|window| window.contents(before_inventory))
            .unwrap_or_else(|| InventoryWindowContents::player(before_inventory.clone()));

        let leftover = {
            let Some(entity_id) = self.player_entity_id(player_id) else {
                return (None, Some(stack));
            };
            let Some(inventory) = self.entities.player_inventory.get_mut(&entity_id) else {
                return (None, Some(stack));
            };
            merge_stack_into_player_inventory(inventory, stack)
        };

        let Some(after_session) = self.player_session(player_id) else {
            return (None, leftover);
        };
        let Some(after_inventory) = self.player_inventory(player_id) else {
            return (None, leftover);
        };
        let after_contents = after_session
            .active_container
            .as_ref()
            .map(|window| window.contents(after_inventory))
            .unwrap_or_else(|| InventoryWindowContents::player(after_inventory.clone()));

        (
            Some(WindowDiffDelta {
                player_id,
                window_id,
                container,
                before_contents,
                after_contents,
                before_properties: Vec::new(),
                after_properties: Vec::new(),
            }),
            leftover,
        )
    }

    pub(in crate::core) fn state_close_world_container_if_invalid(
        &mut self,
        position: BlockPos,
        block: &BlockState,
    ) -> Vec<CloseContainerDelta> {
        let mut deltas = Vec::new();

        let had_chest_block_entity = matches!(
            self.world.block_entities.get(&position),
            Some(BlockEntityState::Chest { .. })
        );
        if block.key.as_str() != catalog::CHEST
            && (had_chest_block_entity || self.world.chest_viewers.contains_key(&position))
        {
            self.world.block_entities.remove(&position);
            deltas.extend(self.state_close_world_chest_viewers(position));
        }

        let had_furnace_block_entity = matches!(
            self.world.block_entities.get(&position),
            Some(BlockEntityState::Furnace { .. })
        );
        if block.key.as_str() != catalog::FURNACE
            && (had_furnace_block_entity || self.has_world_furnace_viewers(position))
        {
            self.world.block_entities.remove(&position);
            deltas.extend(self.state_close_world_furnace_viewers(position));
        }

        deltas
    }

    pub(super) fn state_sync_world_chest_viewers(
        &mut self,
        position: BlockPos,
        actor_player_id: PlayerId,
    ) -> Vec<WindowDiffDelta> {
        let Some(slots) = self
            .player_session(actor_player_id)
            .and_then(|session| session.active_container.as_ref())
            .and_then(|window| match &window.state {
                OpenInventoryWindowState::Chest(chest)
                    if chest.world_position() == Some(position) =>
                {
                    Some(chest.slots.clone())
                }
                _ => None,
            })
        else {
            return Vec::new();
        };

        self.world.block_entities.insert(
            position,
            BlockEntityState::Chest {
                slots: slots.clone(),
            },
        );

        let viewer_ids = self
            .world
            .chest_viewers
            .get(&position)
            .map(|viewers| viewers.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut stale_viewers = Vec::new();
        let mut deltas = Vec::new();
        for viewer_id in viewer_ids {
            if viewer_id == actor_player_id {
                continue;
            }
            let Some(before_inventory) = self.player_inventory(viewer_id).cloned() else {
                stale_viewers.push(viewer_id);
                continue;
            };
            let Some(before_session) = self.player_session(viewer_id) else {
                stale_viewers.push(viewer_id);
                continue;
            };
            let Some(window) = before_session.active_container.as_ref() else {
                stale_viewers.push(viewer_id);
                continue;
            };
            let OpenInventoryWindowState::Chest(chest) = &window.state else {
                stale_viewers.push(viewer_id);
                continue;
            };
            if chest.world_position() != Some(position) {
                stale_viewers.push(viewer_id);
                continue;
            }
            let before_contents =
                InventoryWindowContents::with_container(before_inventory, chest.slots.clone());

            let Some(entity_id) = self.player_entity_id(viewer_id) else {
                stale_viewers.push(viewer_id);
                continue;
            };
            let Some(session) = self.sessions.player_sessions.get_mut(&viewer_id) else {
                stale_viewers.push(viewer_id);
                continue;
            };
            let Some(inventory) = self.entities.player_inventory.get(&entity_id) else {
                stale_viewers.push(viewer_id);
                continue;
            };
            let Some(window) = session.active_container.as_mut() else {
                stale_viewers.push(viewer_id);
                continue;
            };
            let Some(chest) = (match &mut window.state {
                OpenInventoryWindowState::Chest(chest)
                    if chest.world_position() == Some(position) =>
                {
                    Some(chest)
                }
                _ => None,
            }) else {
                stale_viewers.push(viewer_id);
                continue;
            };
            chest.slots = slots.clone();
            let after_contents = window.contents(inventory);
            deltas.push(WindowDiffDelta {
                player_id: viewer_id,
                window_id: window.window_id,
                container: InventoryContainer::Chest,
                before_contents,
                after_contents,
                before_properties: Vec::new(),
                after_properties: Vec::new(),
            });
        }

        for stale_viewer in stale_viewers {
            self.unregister_world_chest_viewer(position, stale_viewer);
        }
        deltas
    }

    pub(super) fn state_sync_world_furnace_state(
        &mut self,
        position: BlockPos,
        actor_player_id: PlayerId,
    ) {
        let Some((_, block_entity)) = self
            .player_session(actor_player_id)
            .and_then(|session| session.active_container.as_ref())
            .and_then(OpenInventoryWindow::world_block_entity)
            .filter(|(window_position, _)| *window_position == position)
        else {
            return;
        };
        if self.block_at(position).key.as_str() == catalog::FURNACE {
            self.world.block_entities.insert(position, block_entity);
        }
    }

    pub(in crate::core) fn state_tick_dropped_item(
        &mut self,
        entity_id: EntityId,
        now_ms: u64,
    ) -> DroppedItemTickDelta {
        let Some(mut item) = self.entities.dropped_items.remove(&entity_id) else {
            return DroppedItemTickDelta {
                inventory_delta: None,
                despawn: None,
            };
        };
        self.advance_dropped_item_entity(&mut item, now_ms);
        if now_ms >= item.despawn_at_ms {
            self.entities.entity_kinds.remove(&entity_id);
            return DroppedItemTickDelta {
                inventory_delta: None,
                despawn: Some(EntityDespawnDelta {
                    entity_ids: vec![entity_id],
                }),
            };
        }
        if now_ms < item.pickup_allowed_at_ms {
            self.entities.dropped_items.insert(entity_id, item);
            return DroppedItemTickDelta {
                inventory_delta: None,
                despawn: None,
            };
        }
        let Some(player_id) = nearest_pickup_player(self, item.snapshot.position) else {
            self.entities.dropped_items.insert(entity_id, item);
            return DroppedItemTickDelta {
                inventory_delta: None,
                despawn: None,
            };
        };
        let (inventory_delta, leftover) =
            self.state_merge_stack_into_online_player_inventory(player_id, item.snapshot.item);
        match leftover {
            Some(leftover) => {
                item.snapshot.item = leftover;
                self.entities.dropped_items.insert(entity_id, item);
                DroppedItemTickDelta {
                    inventory_delta,
                    despawn: None,
                }
            }
            None => {
                self.entities.entity_kinds.remove(&entity_id);
                DroppedItemTickDelta {
                    inventory_delta,
                    despawn: Some(EntityDespawnDelta {
                        entity_ids: vec![entity_id],
                    }),
                }
            }
        }
    }

    pub(in crate::core) fn state_open_non_player_window(
        &mut self,
        player_id: PlayerId,
        mut window: OpenInventoryWindow,
        title: String,
    ) -> Option<OpenContainerDelta> {
        if !self.sessions.player_sessions.contains_key(&player_id) {
            return None;
        }

        match window.container {
            InventoryContainer::CraftingTable => {
                recompute_crafting_result_for_active_container(&mut window);
            }
            InventoryContainer::Furnace => normalize_furnace_window(&mut window),
            InventoryContainer::Chest | InventoryContainer::Player => {}
        }

        let closed = self
            .state_close_player_active_container(player_id, false)
            .into_iter()
            .collect::<Vec<_>>();
        let properties = window.property_entries();
        let window_id = window.window_id;
        let container = window.container;
        let world_chest_position = window.world_chest_position();
        let Some(contents) = ({
            let Some(entity_id) = self.player_entity_id(player_id) else {
                return None;
            };
            let Some(session) = self.sessions.player_sessions.get_mut(&player_id) else {
                return None;
            };
            let Some(inventory) = self.entities.player_inventory.get(&entity_id) else {
                return None;
            };
            let contents = window.contents(inventory);
            session.active_container = Some(window);
            Some(contents)
        }) else {
            return None;
        };
        if let Some(position) = world_chest_position {
            self.register_world_chest_viewer(position, player_id, window_id);
        }

        Some(OpenContainerDelta {
            closed,
            player_id,
            window_id,
            container,
            title,
            contents,
            properties,
        })
    }

    fn allocate_non_player_window_id(&mut self, player_id: PlayerId) -> Option<u8> {
        let session = self.sessions.player_sessions.get_mut(&player_id)?;
        let window_id = session.next_non_player_window_id.max(1);
        session.next_non_player_window_id = if window_id == u8::MAX {
            1
        } else {
            window_id + 1
        };
        Some(window_id)
    }

    fn register_world_chest_viewer(
        &mut self,
        position: BlockPos,
        player_id: PlayerId,
        window_id: u8,
    ) {
        self.world
            .chest_viewers
            .entry(position)
            .or_default()
            .insert(player_id, window_id);
    }

    fn state_close_world_chest_viewers(&mut self, position: BlockPos) -> Vec<CloseContainerDelta> {
        let viewer_ids = self
            .world
            .chest_viewers
            .get(&position)
            .map(|viewers| viewers.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut deltas = Vec::new();
        for viewer_id in viewer_ids {
            if let Some(delta) = self.state_close_player_active_container(viewer_id, true) {
                deltas.push(delta);
            }
        }
        self.world.chest_viewers.remove(&position);
        deltas
    }

    fn has_world_furnace_viewers(&self, position: BlockPos) -> bool {
        self.sessions.player_sessions.values().any(|session| {
            session
                .active_container
                .as_ref()
                .and_then(OpenInventoryWindow::world_furnace_position)
                == Some(position)
        })
    }

    fn state_close_world_furnace_viewers(
        &mut self,
        position: BlockPos,
    ) -> Vec<CloseContainerDelta> {
        let viewer_ids = self
            .sessions
            .player_sessions
            .iter()
            .filter_map(|(player_id, session)| {
                (session
                    .active_container
                    .as_ref()
                    .and_then(OpenInventoryWindow::world_furnace_position)
                    == Some(position))
                .then_some(*player_id)
            })
            .collect::<Vec<_>>();
        let mut deltas = Vec::new();
        for viewer_id in viewer_ids {
            if let Some(delta) = self.state_close_player_active_container(viewer_id, true) {
                deltas.push(delta);
            }
        }
        deltas
    }

    pub(in crate::core) fn state_close_player_active_container(
        &mut self,
        player_id: PlayerId,
        include_player_contents: bool,
    ) -> Option<CloseContainerDelta> {
        let Some((window_id, world_block_entity, world_chest_position, contents)) = ({
            let Some(entity_id) = self.player_entity_id(player_id) else {
                return None;
            };
            let Some(session) = self.sessions.player_sessions.get_mut(&player_id) else {
                return None;
            };
            let Some(inventory) = self.entities.player_inventory.get_mut(&entity_id) else {
                return None;
            };
            let Some(window) = close_active_container_window(session, inventory) else {
                return None;
            };
            let world_block_entity = window.world_block_entity();
            let contents =
                include_player_contents.then(|| InventoryWindowContents::player(inventory.clone()));
            Some((
                window.window_id,
                world_block_entity,
                window.world_chest_position(),
                contents,
            ))
        }) else {
            return None;
        };

        if let Some((position, block_entity)) = world_block_entity {
            let expected_block_key = match &block_entity {
                BlockEntityState::Chest { .. } => catalog::CHEST,
                BlockEntityState::Furnace { .. } => catalog::FURNACE,
            };
            if self.block_at(position).key.as_str() == expected_block_key {
                self.world.block_entities.insert(position, block_entity);
            }
        }
        if let Some(position) = world_chest_position {
            self.unregister_world_chest_viewer(position, player_id);
        }

        Some(CloseContainerDelta {
            player_id,
            window_id,
            contents,
        })
    }
}

pub(crate) fn world_chest_position(window: &OpenInventoryWindow) -> Option<BlockPos> {
    window.world_chest_position()
}

pub(crate) fn world_block_entity(
    window: &OpenInventoryWindow,
) -> Option<(BlockPos, BlockEntityState)> {
    window.world_block_entity()
}

fn persist_live_player_state(
    snapshot: &PlayerSnapshot,
    cursor: Option<&ItemStack>,
    active_container: Option<&OpenInventoryWindow>,
) -> PlayerSnapshot {
    let mut persisted = snapshot.clone();
    if let Some(window) = active_container {
        fold_active_container_items_into_player(&mut persisted.inventory, window);
    }

    let transient_slots = [
        InventorySlot::crafting_result(),
        InventorySlot::crafting_input(0).expect("craft slot should exist"),
        InventorySlot::crafting_input(1).expect("craft slot should exist"),
        InventorySlot::crafting_input(2).expect("craft slot should exist"),
        InventorySlot::crafting_input(3).expect("craft slot should exist"),
        InventorySlot::Auxiliary(5),
        InventorySlot::Auxiliary(6),
        InventorySlot::Auxiliary(7),
        InventorySlot::Auxiliary(8),
    ];
    let mut overflow = transient_slots
        .into_iter()
        .filter_map(|slot| persisted.inventory.get_slot(slot).cloned())
        .collect::<Vec<_>>();
    if let Some(cursor) = cursor.cloned() {
        overflow.push(cursor);
    }
    for slot in transient_slots {
        let _ = persisted.inventory.set_slot(slot, None);
    }
    for stack in overflow {
        let _ = merge_stack_into_player_inventory(&mut persisted.inventory, stack);
    }
    recompute_player_crafting_result(&mut persisted.inventory);
    persisted
}

fn close_active_container_window(
    session: &mut PlayerSessionState,
    inventory: &mut PlayerInventory,
) -> Option<OpenInventoryWindow> {
    let window = session.active_container.take()?;
    fold_active_container_items_into_player(inventory, &window);
    Some(window)
}

fn fold_active_container_items_into_player(
    inventory: &mut PlayerInventory,
    window: &OpenInventoryWindow,
) {
    match &window.state {
        OpenInventoryWindowState::CraftingTable { slots } => {
            for stack in slots.iter().skip(1).flatten().cloned() {
                let _ = merge_stack_into_player_inventory(inventory, stack);
            }
        }
        OpenInventoryWindowState::Chest(chest) => {
            if chest.world_position().is_some() {
                return;
            }
            for stack in chest.slots.iter().flatten().cloned() {
                let _ = merge_stack_into_player_inventory(inventory, stack);
            }
        }
        OpenInventoryWindowState::Furnace(furnace) => {
            if furnace.world_position().is_some() {
                return;
            }
            for stack in furnace.local_slots().into_iter().flatten() {
                let _ = merge_stack_into_player_inventory(inventory, stack);
            }
        }
    }
}

fn nearest_pickup_player(core: &ServerCore, position: Vec3) -> Option<PlayerId> {
    let mut best = None;
    for player_id in core.sessions.player_sessions.keys().copied() {
        let Some(transform) = core.player_transform(player_id) else {
            continue;
        };
        let distance_squared = distance_squared(transform.position, position);
        if distance_squared > DROPPED_ITEM_PICKUP_RADIUS_SQUARED {
            continue;
        }
        match best {
            Some((_, best_distance_squared)) if distance_squared >= best_distance_squared => {}
            _ => best = Some((player_id, distance_squared)),
        }
    }
    best.map(|(player_id, _)| player_id)
}

fn distance_squared(left: Vec3, right: Vec3) -> f64 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    let dz = left.z - right.z;
    dx * dx + dy * dy + dz * dz
}
