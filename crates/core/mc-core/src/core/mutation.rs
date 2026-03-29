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
use mc_content_api::ContainerKindId;

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

    let added_chunks = {
        let Some(session) = state.player_session_mut(player_id) else {
            return None;
        };
        session
            .view
            .retarget(current_chunk, session.view.view_distance)
            .added
    };

    let Some(snapshot) = state.compose_player_snapshot(player_id) else {
        return None;
    };

    Some(PlayerPoseDelta {
        player_id,
        entity_id,
        player: snapshot,
        chunks: added_chunks
            .into_iter()
            .map(|chunk_pos| state.ensure_chunk_mut(chunk_pos).clone())
            .collect::<Vec<ChunkColumn>>(),
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
