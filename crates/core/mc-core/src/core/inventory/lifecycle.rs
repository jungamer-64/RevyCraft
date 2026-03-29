use super::super::canonical::{
    CloseContainerDelta, DroppedItemTickDelta, EntityDespawnDelta, OpenContainerDelta,
    WindowDiffDelta, WorldContainerSyncDelta,
};
use super::super::state_backend::{CoreStateMut, CoreStateRead};
use super::super::{DroppedItemState, PlayerSessionState};
use super::state::OpenInventoryWindow;
use super::util::merge_stack_into_player_inventory;
use super::{ContainerBinding, OpenContainerState};
use crate::inventory::{InventorySlot, InventoryWindowContents, ItemStack, PlayerInventory};
use crate::world::{BlockEntityState, BlockPos, BlockState, Vec3};
use crate::{EntityId, PlayerId, PlayerSnapshot};
use mc_content_api::ContainerSlotRole;
use std::collections::BTreeMap;

const DROPPED_ITEM_PICKUP_RADIUS_SQUARED: f64 = 1.5 * 1.5;
const DROPPED_ITEM_PHYSICS_STEP_MS: u64 = 50;
const DROPPED_ITEM_GRAVITY_PER_STEP: f64 = 0.04;
const DROPPED_ITEM_DRAG: f64 = 0.98;
const DROPPED_ITEM_REST_HEIGHT: f64 = 0.25;

pub(in crate::core) fn open_virtual_container_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    kind: mc_content_api::ContainerKindId,
) -> Option<OpenContainerDelta> {
    let window_id = allocate_non_player_window_id(state, player_id)?;
    let mut window = OpenInventoryWindow {
        window_id,
        container: build_virtual_container_state(state, kind)?,
    };
    state
        .content_behavior()
        .normalize_container(&mut window.container);
    let title = state
        .content_behavior()
        .container_title(&window.container.kind);
    open_non_player_window_state(state, player_id, window, title)
}

pub(in crate::core) fn open_world_container_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    position: BlockPos,
) -> Option<OpenContainerDelta> {
    let block = state.block_state(position)?;
    let kind = state.content_behavior().container_kind_for_block(&block)?;
    let window_id = allocate_non_player_window_id(state, player_id)?;
    let mut window = OpenInventoryWindow {
        window_id,
        container: build_world_container_state(state, position, kind)?,
    };
    state
        .content_behavior()
        .normalize_container(&mut window.container);
    let title = state
        .content_behavior()
        .container_title(&window.container.kind);
    open_non_player_window_state(state, player_id, window, title)
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
        state.content_behavior(),
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
    let before_inventory = state.player_inventory(player_id)?;
    let before_contents = before_window.contents(&before_inventory);
    let before_properties = before_window.property_entries();
    let window_id = before_window.window_id;
    let container = before_window.container.kind.clone();
    let world_position = before_window.world_position();

    let content_behavior = state.content_behavior_arc();
    let (after_contents, after_properties) = {
        let (session, inventory) = state.player_session_inventory_mut(player_id)?;
        let Some(window) = session.active_container.as_mut() else {
            return None;
        };
        content_behavior.tick_container(&mut window.container);
        (window.contents(inventory), window.property_entries())
    };

    if before_contents == after_contents && before_properties == after_properties {
        return None;
    }

    if let Some(position) = world_position {
        let _ = sync_world_container_viewers_state(state, position, player_id);
    }

    Some(WindowDiffDelta {
        player_id,
        window_id,
        container,
        before_contents,
        after_contents,
        before_properties,
        after_properties,
    })
}

pub(in crate::core) fn close_world_container_if_invalid_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
    block: Option<&BlockState>,
) -> Vec<CloseContainerDelta> {
    let current_kind =
        block.and_then(|block| state.content_behavior().container_kind_for_block(block));
    let expected_block_entity_kind = current_kind.as_ref().and_then(|kind| {
        state
            .content_behavior()
            .block_entity_kind_for_container(kind)
    });

    if state.block_entity(position).is_some_and(|block_entity| {
        block_entity
            .container_state()
            .is_some_and(|container| Some(container.kind.clone()) != expected_block_entity_kind)
    }) {
        state.set_block_entity(position, None);
    }

    let Some(viewers) = state.container_viewers(position) else {
        return Vec::new();
    };
    if current_kind.as_ref() == Some(&viewers.kind) {
        return Vec::new();
    }
    close_world_container_viewers_state(state, position)
}

pub(in crate::core) fn sync_world_container_viewers_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
    actor_player_id: PlayerId,
) -> WorldContainerSyncDelta {
    let Some(source_window) = state
        .player_session(actor_player_id)
        .and_then(|session| session.active_container.clone())
        .filter(|window| window.world_position() == Some(position))
    else {
        return WorldContainerSyncDelta::default();
    };

    writeback_world_container_state(state, &source_window);

    let Some(viewers) = state.container_viewers(position) else {
        return WorldContainerSyncDelta::default();
    };
    let source_kind = source_window.container.kind.clone();
    let source_slots = source_window.container.local_slots.clone();
    let source_properties = source_window.container.properties.clone();

    let mut stale_viewers = Vec::new();
    let mut deltas = Vec::new();

    for viewer_id in viewers.viewers.keys().copied().collect::<Vec<_>>() {
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
        let Some(before_window) = before_session.active_container.as_ref() else {
            stale_viewers.push(viewer_id);
            continue;
        };
        if before_window.world_position() != Some(position)
            || before_window.container.kind != source_kind
        {
            stale_viewers.push(viewer_id);
            continue;
        }

        let before_contents = before_window.contents(&before_inventory);
        let before_properties = before_window.property_entries();

        let Some(session) = state.player_session_mut(viewer_id) else {
            stale_viewers.push(viewer_id);
            continue;
        };
        let Some(window) = session.active_container.as_mut() else {
            stale_viewers.push(viewer_id);
            continue;
        };
        if window.world_position() != Some(position) || window.container.kind != source_kind {
            stale_viewers.push(viewer_id);
            continue;
        }

        window.container.local_slots = source_slots.clone();
        window.container.properties = source_properties.clone();
        let after_contents = window.contents(&before_inventory);
        let after_properties = window.property_entries();
        deltas.push(WindowDiffDelta {
            player_id: viewer_id,
            window_id: window.window_id,
            container: source_kind.clone(),
            before_contents,
            after_contents,
            before_properties,
            after_properties,
        });
    }

    for stale_viewer in stale_viewers {
        unregister_world_container_viewer_at_state(state, position, stale_viewer);
    }

    WorldContainerSyncDelta {
        window_diffs: deltas,
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
    state.player_session(player_id)?;
    state
        .content_behavior()
        .normalize_container(&mut window.container);

    let closed = close_player_active_container_state(state, player_id, false)
        .into_iter()
        .collect::<Vec<_>>();
    let properties = window.property_entries();
    let window_id = window.window_id;
    let container = window.container.kind.clone();
    let world_position = window.world_position();

    let contents = {
        let entity_id = state.player_entity_id(player_id)?;
        let inventory = state.player_inventory_by_entity(entity_id)?;
        let contents = window.contents(&inventory);
        let session = state.player_session_mut(player_id)?;
        session.active_container = Some(window);
        contents
    };

    if let Some(position) = world_position {
        register_world_container_viewer_state(
            state,
            position,
            player_id,
            window_id,
            container.clone(),
        );
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
    let content_behavior = state.content_behavior_arc();
    let window = {
        let (session, inventory) = state.player_session_inventory_mut(player_id)?;
        close_active_container_window(content_behavior.as_ref(), session, inventory)?
    };
    let contents = include_player_contents.then(|| {
        let inventory = state.player_inventory(player_id).unwrap_or_default();
        InventoryWindowContents::player(inventory)
    });

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

fn build_virtual_container_state(
    state: &impl CoreStateRead,
    kind: mc_content_api::ContainerKindId,
) -> Option<OpenContainerState> {
    let spec = state.content_behavior().container_spec(&kind)?;
    let (local_slots, properties) = default_container_contents(state, &kind, spec.local_slot_count);
    Some(OpenContainerState {
        kind,
        binding: ContainerBinding::Virtual,
        local_slots,
        properties,
    })
}

fn build_world_container_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
    kind: mc_content_api::ContainerKindId,
) -> Option<OpenContainerState> {
    let spec = state.content_behavior().container_spec(&kind)?;
    let Some(block_entity_kind) = state
        .content_behavior()
        .block_entity_kind_for_container(&kind)
    else {
        return Some(OpenContainerState {
            kind,
            binding: ContainerBinding::Virtual,
            local_slots: vec![None; usize::from(spec.local_slot_count)],
            properties: BTreeMap::new(),
        });
    };

    let block_entity = state
        .block_entity(position)
        .and_then(|entity| entity.container_state().cloned())
        .filter(|entity| entity.kind == block_entity_kind)
        .or_else(|| {
            state
                .content_behavior()
                .default_block_entity_for_kind(&block_entity_kind)
        })?;
    state.set_block_entity(
        position,
        Some(BlockEntityState::Container(block_entity.clone())),
    );

    Some(OpenContainerState {
        kind,
        binding: ContainerBinding::Block {
            position,
            block_entity_kind,
        },
        local_slots: block_entity.slots,
        properties: block_entity.properties,
    })
}

fn default_container_contents(
    state: &impl CoreStateRead,
    kind: &mc_content_api::ContainerKindId,
    local_slot_count: u16,
) -> (
    Vec<Option<ItemStack>>,
    BTreeMap<mc_content_api::ContainerPropertyKey, i16>,
) {
    if let Some(block_entity_kind) = state
        .content_behavior()
        .block_entity_kind_for_container(kind)
        && let Some(block_entity) = state
            .content_behavior()
            .default_block_entity_for_kind(&block_entity_kind)
    {
        return (block_entity.slots, block_entity.properties);
    }
    (vec![None; usize::from(local_slot_count)], BTreeMap::new())
}

fn register_world_container_viewer_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
    player_id: PlayerId,
    window_id: u8,
    kind: mc_content_api::ContainerKindId,
) {
    let mut entry =
        state
            .container_viewers(position)
            .unwrap_or(super::super::WorldContainerViewers {
                kind,
                viewers: BTreeMap::new(),
            });
    entry.viewers.insert(player_id, window_id);
    state.set_container_viewers(position, Some(entry));
}

fn unregister_world_container_viewer_at_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
    player_id: PlayerId,
) {
    let Some(mut entry) = state.container_viewers(position) else {
        return;
    };
    entry.viewers.remove(&player_id);
    if entry.viewers.is_empty() {
        state.set_container_viewers(position, None);
    } else {
        state.set_container_viewers(position, Some(entry));
    }
}

pub(in crate::core) fn writeback_world_container_state(
    state: &mut impl CoreStateMut,
    window: &OpenInventoryWindow,
) {
    let Some((position, block_entity)) = window.world_block_entity() else {
        return;
    };
    let Some(block) = state.block_state(position) else {
        return;
    };
    if state
        .content_behavior()
        .container_kind_for_block(&block)
        .as_ref()
        != Some(&window.container.kind)
    {
        return;
    }
    state.set_block_entity(position, Some(BlockEntityState::Container(block_entity)));
}

pub(in crate::core) fn unregister_world_container_viewer_state(
    state: &mut impl CoreStateMut,
    window: &OpenInventoryWindow,
    player_id: PlayerId,
) {
    if let Some(position) = window.world_position() {
        unregister_world_container_viewer_at_state(state, position, player_id);
    }
}

fn close_world_container_viewers_state(
    state: &mut impl CoreStateMut,
    position: BlockPos,
) -> Vec<CloseContainerDelta> {
    let viewer_ids = state
        .container_viewers(position)
        .map(|viewers| viewers.viewers.keys().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut deltas = Vec::new();
    for viewer_id in viewer_ids {
        if let Some(delta) = close_player_active_container_state(state, viewer_id, true) {
            deltas.push(delta);
        }
    }
    state.set_container_viewers(position, None);
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
    let player_container = state.content_behavior().player_container_kind();
    let (window_id, container) = before_session
        .active_container
        .as_ref()
        .map(|window| (window.window_id, window.container.kind.clone()))
        .unwrap_or((0, player_container.clone()));
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
        let block = state.block_state(position);
        if block
            .as_ref()
            .is_none_or(|block| state.content_behavior().is_air_block(block))
        {
            continue;
        }
        return Some(f64::from(block_y) + 1.0 + DROPPED_ITEM_REST_HEIGHT);
    }
    None
}

fn persist_live_player_state(
    content_behavior: &dyn mc_content_api::ContentBehavior,
    snapshot: &PlayerSnapshot,
    cursor: Option<&ItemStack>,
    active_container: Option<&OpenInventoryWindow>,
) -> PlayerSnapshot {
    let mut persisted = snapshot.clone();
    if let Some(window) = active_container {
        fold_active_container_items_into_player(content_behavior, &mut persisted.inventory, window);
    }

    if let Some(spec) = content_behavior.container_spec(&content_behavior.player_container_kind()) {
        let mut overflow = Vec::new();
        for index in 0_u16..spec.local_slot_count {
            let slot = InventorySlot::WindowLocal(index);
            if let Some(stack) = persisted.inventory.get_slot(slot).cloned() {
                overflow.push(stack);
            }
            let _ = persisted.inventory.set_slot(slot, None);
        }
        if let Some(cursor) = cursor.cloned() {
            overflow.push(cursor);
        }
        for stack in overflow {
            let _ = merge_stack_into_player_inventory(&mut persisted.inventory, stack);
        }
    }

    content_behavior.normalize_player_inventory(&mut persisted.inventory);
    persisted
}

fn close_active_container_window(
    content_behavior: &dyn mc_content_api::ContentBehavior,
    session: &mut PlayerSessionState,
    inventory: &mut PlayerInventory,
) -> Option<OpenInventoryWindow> {
    let window = session.active_container.take()?;
    fold_active_container_items_into_player(content_behavior, inventory, &window);
    Some(window)
}

fn fold_active_container_items_into_player(
    content_behavior: &dyn mc_content_api::ContentBehavior,
    inventory: &mut PlayerInventory,
    window: &OpenInventoryWindow,
) {
    if matches!(window.container.binding, ContainerBinding::Block { .. }) {
        return;
    }

    let spec = content_behavior.container_spec(&window.container.kind);
    for (index, stack) in window.container.local_slots.iter().enumerate() {
        let Some(stack) = stack.clone() else {
            continue;
        };
        if spec.as_ref().is_some_and(|spec| {
            spec.slot_role(u16::try_from(index).expect("local slot index fits"))
                == ContainerSlotRole::OutputOnly
                && index == 0
        }) {
            continue;
        }
        let _ = merge_stack_into_player_inventory(inventory, stack);
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
