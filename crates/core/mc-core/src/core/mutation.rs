use super::canonical::{
    BlockDelta, DroppedItemSpawnDelta, InventorySlotDelta, PlayerPoseDelta, SelectedHotbarDelta,
};
use super::{DroppedItemState, EntityKind, ServerCore};
use crate::events::{CoreEvent, EventTarget, TargetedEvent};
use crate::inventory::{InventoryContainer, InventorySlot, ItemStack};
use crate::player::InteractionHand;
use crate::world::{BlockPos, BlockState, ChunkColumn, DroppedItemSnapshot, Vec3};
use crate::{EntityId, HOTBAR_SLOT_COUNT, PlayerId};

const DROPPED_ITEM_PICKUP_DELAY_MS: u64 = 500;
const DROPPED_ITEM_DESPAWN_MS: u64 = 5 * 60 * 1000;

impl ServerCore {
    pub(super) fn state_player_pose(
        &mut self,
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    ) -> Option<PlayerPoseDelta> {
        let Some(entity_id) = self.player_entity_id(player_id) else {
            return None;
        };

        let current_chunk = {
            let Some(transform) = self.entities.player_transform.get_mut(&entity_id) else {
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
            let Some(session) = self.sessions.player_sessions.get_mut(&player_id) else {
                return None;
            };
            session
                .view
                .retarget(current_chunk, session.view.view_distance)
                .added
        };

        let Some(snapshot) = self.compose_player_snapshot(player_id) else {
            return None;
        };

        Some(PlayerPoseDelta {
            player_id,
            entity_id,
            player: snapshot,
            chunks: added_chunks
                .into_iter()
                .map(|chunk_pos| self.ensure_chunk(chunk_pos).clone())
                .collect::<Vec<ChunkColumn>>(),
        })
    }

    pub(super) fn state_selected_hotbar_slot(
        &mut self,
        player_id: PlayerId,
        slot: u8,
    ) -> Option<SelectedHotbarDelta> {
        let Some(selected_hotbar_slot) = self.player_selected_hotbar_mut(player_id) else {
            return None;
        };
        if slot >= HOTBAR_SLOT_COUNT {
            return None;
        }
        *selected_hotbar_slot = slot;
        Some(SelectedHotbarDelta { player_id, slot })
    }

    pub(super) fn state_inventory_slot(
        &mut self,
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    ) -> Option<InventorySlotDelta> {
        let Some(inventory) = self.player_inventory_mut(player_id) else {
            return None;
        };
        let before_result = inventory.crafting_result().cloned();
        let _ = inventory.set_slot(slot, stack.clone());
        if slot.is_crafting_result() || slot.crafting_input_index().is_some() {
            Self::recompute_crafting_result_for_inventory(inventory);
        }
        let after_result = inventory.crafting_result().cloned();
        Some(InventorySlotDelta {
            player_id,
            slot,
            stack,
            crafting_result: (before_result != after_result && !slot.is_crafting_result())
                .then_some(after_result),
        })
    }

    pub(super) fn state_set_block(&mut self, position: BlockPos, block: BlockState) -> BlockDelta {
        let cleared_mining = self.state_clear_active_mining_at(position);
        self.set_block_at(position, block.clone());
        let closed_containers = self.state_close_world_container_if_invalid(position, &block);
        if block.key.as_str() == crate::catalog::CHEST {
            self.world
                .block_entities
                .entry(position)
                .or_insert_with(|| crate::BlockEntityState::chest(27));
        }
        if block.key.as_str() == crate::catalog::FURNACE {
            self.world
                .block_entities
                .entry(position)
                .or_insert_with(crate::BlockEntityState::furnace);
        }
        BlockDelta {
            position,
            cleared_mining,
            closed_containers,
        }
    }

    pub(super) fn state_spawn_dropped_item(
        &mut self,
        expected_entity_id: Option<EntityId>,
        position: Vec3,
        item: ItemStack,
        now_ms: u64,
    ) -> Option<DroppedItemSpawnDelta> {
        let entity_id = EntityId(self.entities.next_entity_id);
        self.entities.next_entity_id = self.entities.next_entity_id.saturating_add(1);
        let snapshot = DroppedItemSnapshot {
            item,
            position,
            velocity: Vec3::new(0.0, 0.0, 0.0),
        };
        self.entities
            .entity_kinds
            .insert(entity_id, EntityKind::DroppedItem);
        self.entities.dropped_items.insert(
            entity_id,
            DroppedItemState {
                snapshot: snapshot.clone(),
                last_updated_at_ms: now_ms,
                pickup_allowed_at_ms: now_ms.saturating_add(DROPPED_ITEM_PICKUP_DELAY_MS),
                despawn_at_ms: now_ms.saturating_add(DROPPED_ITEM_DESPAWN_MS),
            },
        );
        if let Some(expected_entity_id) = expected_entity_id {
            debug_assert_eq!(entity_id, expected_entity_id);
        }
        Some(DroppedItemSpawnDelta {
            entity_id,
            item: snapshot,
        })
    }

    pub(crate) fn place_inventory_correction(
        player_id: PlayerId,
        hand: InteractionHand,
        player: &crate::player::PlayerSnapshot,
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
}
