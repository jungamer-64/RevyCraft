use super::canonical::{
    BlockDelta, DroppedItemSpawnDelta, InventorySlotDelta, PlayerPoseDelta, SelectedHotbarDelta,
};
use super::state_backend::CoreStateMut;
use super::{DroppedItemState, EntityKind, ServerCore};
use crate::events::{CoreEvent, EventTarget, TargetedEvent};
use crate::inventory::{InventorySlot, ItemStack};
use crate::player::InteractionHand;
use crate::world::{
    BlockEntityState, BlockPos, BlockState, ChunkColumn, DroppedItemSnapshot, Vec3,
};
use crate::{EntityId, HOTBAR_SLOT_COUNT, PlayerId};
use revy_voxel_semantic::ContainerKindId;

const DROPPED_ITEM_PICKUP_DELAY_MS: u64 = 500;
const DROPPED_ITEM_DESPAWN_MS: u64 = 5 * 60 * 1000;

pub(super) fn state_player_pose(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    position: Option<Vec3>,
    yaw: Option<f32>,
    pitch: Option<f32>,
    on_ground: bool,
) -> Option<PlayerPoseDelta> {
    let Some(entity_id) = state.player_entity_id(player_id) else {
        return None;
    };
    let previous_player = state.compose_player_snapshot(player_id)?;
    let previous_view = state.player_session(player_id)?.view;

    let current_chunk = {
        let Some(transform) = state.player_transform_mut(entity_id) else {
            return None;
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
        transform.position.chunk_pos()
    };

    let (added_chunks, current_view) = {
        let Some(session) = state.player_session_mut(player_id) else {
            return None;
        };
        let added_chunks = session
            .view
            .retarget(current_chunk, session.view.view_distance)
            .added;
        (added_chunks, session.view.clone())
    };

    let Some(snapshot) = state.compose_player_snapshot(player_id) else {
        return None;
    };

    let mut moved_for_viewers = Vec::new();
    let mut spawned_for_viewers = Vec::new();
    let mut despawned_for_viewers = Vec::new();
    let mut observed_players_entered = Vec::new();
    let mut observed_entities_left = Vec::new();
    for other_id in state
        .player_ids()
        .into_iter()
        .filter(|other_id| *other_id != player_id)
    {
        let Some(other_session) = state.player_session(other_id) else {
            continue;
        };
        let Some(other_player) = state.compose_player_snapshot(other_id) else {
            continue;
        };

        let viewer_saw_player = other_session
            .view
            .loaded_chunks
            .contains(&previous_player.position.chunk_pos());
        let viewer_sees_player = other_session
            .view
            .loaded_chunks
            .contains(&snapshot.position.chunk_pos());
        match (viewer_saw_player, viewer_sees_player) {
            (true, true) => moved_for_viewers.push(other_id),
            (false, true) => spawned_for_viewers.push(other_id),
            (true, false) => despawned_for_viewers.push(other_id),
            (false, false) => {}
        }

        let player_saw_other = previous_view
            .loaded_chunks
            .contains(&other_player.position.chunk_pos());
        let player_sees_other = current_view
            .loaded_chunks
            .contains(&other_player.position.chunk_pos());
        match (player_saw_other, player_sees_other) {
            (false, true) => {
                observed_players_entered.push((other_session.entity_id, other_player));
            }
            (true, false) => observed_entities_left.push(other_session.entity_id),
            _ => {}
        }
    }

    let mut observed_items_entered = Vec::new();
    for dropped_entity_id in state.dropped_item_ids() {
        let Some(item) = state.dropped_item_by_entity(dropped_entity_id) else {
            continue;
        };
        let item_chunk = item.snapshot.position.chunk_pos();
        match (
            previous_view.loaded_chunks.contains(&item_chunk),
            current_view.loaded_chunks.contains(&item_chunk),
        ) {
            (false, true) => observed_items_entered.push((dropped_entity_id, item.snapshot)),
            (true, false) => observed_entities_left.push(dropped_entity_id),
            _ => {}
        }
    }

    Some(PlayerPoseDelta {
        player_id,
        entity_id,
        player: snapshot,
        chunks: added_chunks
            .into_iter()
            .map(|chunk_pos| state.ensure_chunk(chunk_pos).clone())
            .collect::<Vec<ChunkColumn>>(),
        moved_for_viewers,
        spawned_for_viewers,
        despawned_for_viewers,
        observed_players_entered,
        observed_items_entered,
        observed_entities_left,
    })
}

pub(super) fn state_selected_hotbar_slot(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    slot: u8,
) -> Option<SelectedHotbarDelta> {
    let Some(entity_id) = state.player_entity_id(player_id) else {
        return None;
    };
    let Some(selected_hotbar_slot) = state.player_selected_hotbar_mut(entity_id) else {
        return None;
    };
    if slot >= HOTBAR_SLOT_COUNT {
        return None;
    }
    *selected_hotbar_slot = slot;
    Some(SelectedHotbarDelta { player_id, slot })
}

pub(super) fn state_inventory_slot(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    slot: InventorySlot,
    stack: Option<ItemStack>,
) -> Option<InventorySlotDelta> {
    let Some(entity_id) = state.player_entity_id(player_id) else {
        return None;
    };
    let content_behavior = state.content_behavior_arc();
    let Some(inventory) = state.player_inventory_mut(entity_id) else {
        return None;
    };
    let before_result = inventory.crafting_result().cloned();
    let _ = inventory.set_slot(slot, stack.clone());
    content_behavior.normalize_player_inventory(inventory);
    let after_result = inventory.crafting_result().cloned();
    Some(InventorySlotDelta {
        player_id,
        slot,
        stack,
        crafting_result: (before_result != after_result && !slot.is_crafting_result())
            .then_some(after_result),
    })
}

pub(super) fn state_set_block(
    state: &mut impl CoreStateMut,
    position: BlockPos,
    block: Option<BlockState>,
) -> BlockDelta {
    let cleared_mining = super::mining::state_clear_active_mining_at(state, position);
    state.set_block_state(position, block.clone());
    let closed_containers =
        super::inventory::close_world_container_if_invalid_state(state, position, block.as_ref());
    if let Some(block) = block.as_ref() {
        if let Some(block_entity) = state
            .content_behavior()
            .default_block_entity_for_block(block)
        {
            state.set_block_entity(position, Some(BlockEntityState::Container(block_entity)));
        }
    } else {
        state.set_block_entity(position, None);
    }
    BlockDelta {
        position,
        cleared_mining,
        closed_containers,
    }
}

pub(super) fn state_spawn_dropped_item(
    state: &mut impl CoreStateMut,
    expected_entity_id: Option<EntityId>,
    position: Vec3,
    item: ItemStack,
    now_ms: u64,
) -> Option<DroppedItemSpawnDelta> {
    let entity_id = state.allocate_entity_id();
    if let Some(expected_entity_id) = expected_entity_id {
        debug_assert_eq!(entity_id, expected_entity_id);
    }
    let snapshot = DroppedItemSnapshot {
        item,
        position,
        velocity: Vec3::new(0.0, 0.0, 0.0),
    };
    state.set_entity_kind(entity_id, Some(EntityKind::DroppedItem));
    state.set_dropped_item(
        entity_id,
        Some(DroppedItemState {
            snapshot: snapshot.clone(),
            last_updated_at_ms: now_ms,
            pickup_allowed_at_ms: now_ms.saturating_add(DROPPED_ITEM_PICKUP_DELAY_MS),
            despawn_at_ms: now_ms.saturating_add(DROPPED_ITEM_DESPAWN_MS),
        }),
    );
    Some(DroppedItemSpawnDelta {
        entity_id,
        item: snapshot,
    })
}

impl ServerCore {
    pub(crate) fn place_inventory_correction(
        player_id: PlayerId,
        hand: InteractionHand,
        player: &crate::player::PlayerSnapshot,
        player_container: ContainerKindId,
    ) -> Vec<TargetedEvent> {
        let selected_slot = match hand {
            InteractionHand::Main => InventorySlot::Hotbar(player.selected_hotbar_slot),
            InteractionHand::Offhand => InventorySlot::Offhand,
        };
        vec![
            TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventorySlotChanged {
                    window_id: 0,
                    container: player_container,
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
}
