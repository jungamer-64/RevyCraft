use super::super::canonical::{
    ApplyCoreOpsOptions, CloseContainerDelta, CoreOp, DroppedItemTickDelta, EntityDespawnDelta,
    OpenContainerDelta, WindowDiffDelta, WorldContainerSyncDelta, apply_core_ops,
};
use super::super::state_backend::{CoreStateMut, CoreStateRead};
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

    pub(in crate::core) fn recompute_crafting_result_for_inventory(
        inventory: &mut PlayerInventory,
    ) {
        recompute_player_crafting_result(inventory);
    }
}

pub(in crate::core) fn open_world_chest_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    position: BlockPos,
) -> Option<OpenContainerDelta> {
    if state.block_state(position).key.as_str() != catalog::CHEST {
        return None;
    }
    let block_entity = state
        .block_entity(position)
        .unwrap_or_else(|| BlockEntityState::chest(CHEST_LOCAL_SLOT_COUNT));
    state.set_block_entity(position, Some(block_entity.clone()));
    let slots = block_entity
        .chest_slots()
        .expect("chest block entity should expose slots")
        .to_vec();
    let window_id = allocate_non_player_window_id(state, player_id)?;
    open_non_player_window_state(
        state,
        player_id,
        OpenInventoryWindow {
            window_id,
            container: InventoryContainer::Chest,
            state: OpenInventoryWindowState::Chest(ChestWindowState::new_block(position, slots)),
        },
        "Chest".to_string(),
    )
}

pub(in crate::core) fn open_world_furnace_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    position: BlockPos,
) -> Option<OpenContainerDelta> {
    if state.block_state(position).key.as_str() != catalog::FURNACE {
        return None;
    }
    let block_entity = state
        .block_entity(position)
        .unwrap_or_else(BlockEntityState::furnace);
    state.set_block_entity(position, Some(block_entity.clone()));
    let window_id = allocate_non_player_window_id(state, player_id)?;
    open_non_player_window_state(
        state,
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

pub(in crate::core) fn close_inventory_window_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    window_id: u8,
) -> Option<CloseContainerDelta> {
    if window_id == 0 {
        return None;
    }
    let Some(active_window_id) = state.player_session(player_id).and_then(|session| {
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
    close_player_active_container_state(state, player_id, true)
}

pub(in crate::core) fn persisted_online_player_snapshot_state(
    state: &impl CoreStateRead,
    player_id: PlayerId,
) -> Option<PlayerSnapshot> {
    let snapshot = state.compose_player_snapshot(player_id)?;
    let session = state.player_session(player_id)?;
    Some(persist_live_player_state(
        &snapshot,
        session.cursor.as_ref(),
        session.active_container.as_ref(),
    ))
}

pub(in crate::core) fn tick_active_container_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
) -> Option<WindowDiffDelta> {
    let before_session = state.player_session(player_id)?;
    let before_window = before_session.active_container.as_ref()?;
    if before_window.container != InventoryContainer::Furnace {
        return None;
    }
    let before_inventory = state.player_inventory(player_id)?;
    let before_contents = before_window.contents(&before_inventory);
    let before_properties = before_window.property_entries();
    let window_id = before_window.window_id;

    let (after_contents, after_properties, block_entity_update) = {
        let (session, inventory) = state.player_session_inventory_mut(player_id)?;
        match session.active_container.as_mut() {
            Some(window) if window.container == InventoryContainer::Furnace => {
                tick_furnace_window(window);
                (
                    window.contents(inventory),
                    window.property_entries(),
                    window.world_block_entity(),
                )
            }
            _ => return None,
        }
    };

    if let Some((position, block_entity)) = block_entity_update
        && matches!(block_entity, BlockEntityState::Furnace { .. })
        && state.block_state(position).key.as_str() == catalog::FURNACE
    {
        state.set_block_entity(position, Some(block_entity));
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

pub(in crate::core) fn close_world_container_if_invalid_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
    block: &BlockState,
) -> Vec<CloseContainerDelta> {
    let mut deltas = Vec::new();

    let had_chest_block_entity = matches!(
        state.block_entity(position),
        Some(BlockEntityState::Chest { .. })
    );
    if block.key.as_str() != catalog::CHEST
        && (had_chest_block_entity || state.chest_viewers(position).is_some())
    {
        state.set_block_entity(position, None);
        deltas.extend(close_world_chest_viewers_state(state, position));
    }

    let had_furnace_block_entity = matches!(
        state.block_entity(position),
        Some(BlockEntityState::Furnace { .. })
    );
    if block.key.as_str() != catalog::FURNACE
        && (had_furnace_block_entity || has_world_furnace_viewers(state, position))
    {
        state.set_block_entity(position, None);
        deltas.extend(close_world_furnace_viewers_state(state, position));
    }

    deltas
}

pub(in crate::core) fn sync_world_chest_viewers_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
    actor_player_id: PlayerId,
) -> WorldContainerSyncDelta {
    let Some(slots) = state
        .player_session(actor_player_id)
        .and_then(|session| session.active_container.as_ref().cloned())
        .and_then(|window| match window.state {
            OpenInventoryWindowState::Chest(chest) if chest.world_position() == Some(position) => {
                Some(chest.slots)
            }
            _ => None,
        })
    else {
        return WorldContainerSyncDelta::default();
    };

    state.set_block_entity(
        position,
        Some(BlockEntityState::Chest {
            slots: slots.clone(),
        }),
    );

    let viewer_ids = state
        .chest_viewers(position)
        .map(|viewers| viewers.keys().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut stale_viewers = Vec::new();
    let mut deltas = Vec::new();
    for viewer_id in viewer_ids {
        if viewer_id == actor_player_id {
            continue;
        }
        let Some(before_inventory) = state.player_inventory(viewer_id) else {
            stale_viewers.push(viewer_id);
            continue;
        };
        let Some(before_session) = state.player_session(viewer_id) else {
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
            InventoryWindowContents::with_container(before_inventory.clone(), chest.slots.clone());

        let Some(session) = state.player_session_mut(viewer_id) else {
            stale_viewers.push(viewer_id);
            continue;
        };
        let Some(window) = session.active_container.as_mut() else {
            stale_viewers.push(viewer_id);
            continue;
        };
        let Some(chest) = (match &mut window.state {
            OpenInventoryWindowState::Chest(chest) if chest.world_position() == Some(position) => {
                Some(chest)
            }
            _ => None,
        }) else {
            stale_viewers.push(viewer_id);
            continue;
        };
        chest.slots = slots.clone();
        let after_contents = window.contents(&before_inventory);
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
        unregister_world_chest_viewer_state(state, position, stale_viewer);
    }
    WorldContainerSyncDelta {
        window_diffs: deltas,
    }
}

pub(in crate::core) fn sync_world_furnace_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
    actor_player_id: PlayerId,
) {
    let Some((_, block_entity)) = state
        .player_session(actor_player_id)
        .and_then(|session| session.active_container.as_ref().cloned())
        .and_then(|window| window.world_block_entity())
        .filter(|(window_position, _)| *window_position == position)
    else {
        return;
    };
    if state.block_state(position).key.as_str() == catalog::FURNACE {
        state.set_block_entity(position, Some(block_entity));
    }
}

pub(in crate::core) fn tick_dropped_item_state(
    state: &mut impl CoreStateMut,
    entity_id: EntityId,
    now_ms: u64,
) -> DroppedItemTickDelta {
    let Some(mut item) = state.take_dropped_item(entity_id) else {
        return DroppedItemTickDelta {
            inventory_delta: None,
            despawn: None,
        };
    };
    advance_dropped_item_entity(state, &mut item, now_ms);
    if now_ms >= item.despawn_at_ms {
        state.set_entity_kind(entity_id, None);
        return DroppedItemTickDelta {
            inventory_delta: None,
            despawn: Some(EntityDespawnDelta {
                entity_ids: vec![entity_id],
            }),
        };
    }
    if now_ms < item.pickup_allowed_at_ms {
        state.set_dropped_item(entity_id, Some(item));
        return DroppedItemTickDelta {
            inventory_delta: None,
            despawn: None,
        };
    }
    let Some(player_id) = nearest_pickup_player_in(state, item.snapshot.position) else {
        state.set_dropped_item(entity_id, Some(item));
        return DroppedItemTickDelta {
            inventory_delta: None,
            despawn: None,
        };
    };
    let (inventory_delta, leftover) =
        merge_stack_into_online_player_inventory_state(state, player_id, item.snapshot.item);
    match leftover {
        Some(leftover) => {
            item.snapshot.item = leftover;
            state.set_dropped_item(entity_id, Some(item));
            DroppedItemTickDelta {
                inventory_delta,
                despawn: None,
            }
        }
        None => {
            state.set_entity_kind(entity_id, None);
            DroppedItemTickDelta {
                inventory_delta,
                despawn: Some(EntityDespawnDelta {
                    entity_ids: vec![entity_id],
                }),
            }
        }
    }
}

pub(in crate::core) fn open_non_player_window_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    mut window: OpenInventoryWindow,
    title: String,
) -> Option<OpenContainerDelta> {
    if state.player_session(player_id).is_none() {
        return None;
    }

    match window.container {
        InventoryContainer::CraftingTable => {
            recompute_crafting_result_for_active_container(&mut window);
        }
        InventoryContainer::Furnace => normalize_furnace_window(&mut window),
        InventoryContainer::Chest | InventoryContainer::Player => {}
    }

    let closed = close_player_active_container_state(state, player_id, false)
        .into_iter()
        .collect::<Vec<_>>();
    let properties = window.property_entries();
    let window_id = window.window_id;
    let container = window.container;
    let world_chest_position = window.world_chest_position();
    let contents = {
        let entity_id = state.player_entity_id(player_id)?;
        let inventory = state.player_inventory_by_entity(entity_id)?;
        let contents = window.contents(&inventory);
        let session = state.player_session_mut(player_id)?;
        session.active_container = Some(window);
        contents
    };
    if let Some(position) = world_chest_position {
        register_world_chest_viewer_state(state, position, player_id, window_id);
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

pub(in crate::core) fn close_player_active_container_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    include_player_contents: bool,
) -> Option<CloseContainerDelta> {
    let (window, contents) = {
        let (session, inventory) = state.player_session_inventory_mut(player_id)?;
        let window = close_active_container_window(session, inventory)?;
        let contents =
            include_player_contents.then(|| InventoryWindowContents::player(inventory.clone()));
        (window, contents)
    };

    writeback_world_container_state(state, &window);
    unregister_world_container_viewer_state(state, &window, player_id);

    Some(CloseContainerDelta {
        player_id,
        window_id: window.window_id,
        contents,
    })
}

fn allocate_non_player_window_id(state: &mut impl CoreStateMut, player_id: PlayerId) -> Option<u8> {
    let session = state.player_session_mut(player_id)?;
    let window_id = session.next_non_player_window_id.max(1);
    session.next_non_player_window_id = if window_id == u8::MAX {
        1
    } else {
        window_id + 1
    };
    Some(window_id)
}

pub(in crate::core) fn register_world_chest_viewer_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
    player_id: PlayerId,
    window_id: u8,
) {
    let mut viewers = state.chest_viewers(position).unwrap_or_default();
    viewers.insert(player_id, window_id);
    state.set_chest_viewers(position, Some(viewers));
}

fn unregister_world_chest_viewer_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
    player_id: PlayerId,
) {
    let Some(mut viewers) = state.chest_viewers(position) else {
        return;
    };
    viewers.remove(&player_id);
    if viewers.is_empty() {
        state.set_chest_viewers(position, None);
    } else {
        state.set_chest_viewers(position, Some(viewers));
    }
}

pub(in crate::core) fn writeback_world_container_state(
    state: &mut impl CoreStateMut,
    window: &OpenInventoryWindow,
) {
    let Some((position, block_entity)) = window.world_block_entity() else {
        return;
    };
    let expected_block_key = match &block_entity {
        BlockEntityState::Chest { .. } => catalog::CHEST,
        BlockEntityState::Furnace { .. } => catalog::FURNACE,
    };
    if state.block_state(position).key.as_str() == expected_block_key {
        state.set_block_entity(position, Some(block_entity));
    }
}

pub(in crate::core) fn unregister_world_container_viewer_state(
    state: &mut impl CoreStateMut,
    window: &OpenInventoryWindow,
    player_id: PlayerId,
) {
    if let Some(position) = window.world_chest_position() {
        unregister_world_chest_viewer_state(state, position, player_id);
    }
}

fn close_world_chest_viewers_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
) -> Vec<CloseContainerDelta> {
    let viewer_ids = state
        .chest_viewers(position)
        .map(|viewers| viewers.keys().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut deltas = Vec::new();
    for viewer_id in viewer_ids {
        if let Some(delta) = close_player_active_container_state(state, viewer_id, true) {
            deltas.push(delta);
        }
    }
    state.set_chest_viewers(position, None);
    deltas
}

fn has_world_furnace_viewers(state: &impl CoreStateRead, position: BlockPos) -> bool {
    state.player_ids().into_iter().any(|player_id| {
        state
            .player_session(player_id)
            .and_then(|session| session.active_container)
            .and_then(|window| window.world_furnace_position())
            == Some(position)
    })
}

fn close_world_furnace_viewers_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
) -> Vec<CloseContainerDelta> {
    let viewer_ids = state
        .player_ids()
        .into_iter()
        .filter(|player_id| {
            state
                .player_session(*player_id)
                .and_then(|session| session.active_container)
                .and_then(|window| window.world_furnace_position())
                == Some(position)
        })
        .collect::<Vec<_>>();
    let mut deltas = Vec::new();
    for viewer_id in viewer_ids {
        if let Some(delta) = close_player_active_container_state(state, viewer_id, true) {
            deltas.push(delta);
        }
    }
    deltas
}

fn merge_stack_into_online_player_inventory_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    stack: ItemStack,
) -> (Option<WindowDiffDelta>, Option<ItemStack>) {
    let Some(before_session) = state.player_session(player_id) else {
        return (None, Some(stack));
    };
    let Some(before_inventory) = state.player_inventory(player_id) else {
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
        .map(|window| window.contents(&before_inventory))
        .unwrap_or_else(|| InventoryWindowContents::player(before_inventory.clone()));

    let leftover = {
        let Some(entity_id) = state.player_entity_id(player_id) else {
            return (None, Some(stack));
        };
        let Some(inventory) = state.player_inventory_mut(entity_id) else {
            return (None, Some(stack));
        };
        merge_stack_into_player_inventory(inventory, stack)
    };

    let Some(after_session) = state.player_session(player_id) else {
        return (None, leftover);
    };
    let Some(after_inventory) = state.player_inventory(player_id) else {
        return (None, leftover);
    };
    let after_contents = after_session
        .active_container
        .as_ref()
        .map(|window| window.contents(&after_inventory))
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

fn advance_dropped_item_entity(
    state: &impl CoreStateRead,
    item: &mut DroppedItemState,
    now_ms: u64,
) {
    let elapsed_ms = now_ms.saturating_sub(item.last_updated_at_ms);
    let step_count = elapsed_ms / DROPPED_ITEM_PHYSICS_STEP_MS;
    if step_count == 0 {
        return;
    }

    for _ in 0..step_count {
        advance_dropped_item_step(state, item);
    }

    item.last_updated_at_ms = item
        .last_updated_at_ms
        .saturating_add(step_count.saturating_mul(DROPPED_ITEM_PHYSICS_STEP_MS));
}

fn advance_dropped_item_step(state: &impl CoreStateRead, item: &mut DroppedItemState) {
    let next_velocity_y =
        (item.snapshot.velocity.y - DROPPED_ITEM_GRAVITY_PER_STEP) * DROPPED_ITEM_DRAG;
    let next_y = item.snapshot.position.y + next_velocity_y;
    if let Some(rest_y) = dropped_item_rest_y(
        state,
        item.snapshot.position.x,
        next_y,
        item.snapshot.position.z,
    ) && next_y <= rest_y
    {
        item.snapshot.position.y = rest_y;
        item.snapshot.velocity.y = 0.0;
        return;
    }

    item.snapshot.position.y = next_y;
    item.snapshot.velocity.y = next_velocity_y;
}

fn dropped_item_rest_y(state: &impl CoreStateRead, x: f64, y: f64, z: f64) -> Option<f64> {
    let block_x = x.floor() as i32;
    let block_z = z.floor() as i32;
    let max_block_y = (y.floor() as i32).clamp(0, 255);
    for block_y in (0..=max_block_y).rev() {
        let position = BlockPos::new(block_x, block_y, block_z);
        if state.block_state(position).is_air() {
            continue;
        }
        return Some(f64::from(block_y) + 1.0 + DROPPED_ITEM_REST_HEIGHT);
    }
    None
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

fn nearest_pickup_player_in(state: &impl CoreStateRead, position: Vec3) -> Option<PlayerId> {
    let mut best = None;
    for player_id in state.player_ids() {
        let Some(transform) = state.player_transform(player_id) else {
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
