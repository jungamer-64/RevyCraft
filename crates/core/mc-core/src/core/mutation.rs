use super::{DroppedItemEntity, ServerCore};
use crate::events::{CoreEvent, EventTarget, TargetedEvent};
use crate::gameplay::{GameplayEffect, GameplayMutation};
use crate::inventory::{InventoryContainer, InventorySlot, ItemStack};
use crate::player::{InteractionHand, PlayerSnapshot};
use crate::world::{BlockPos, BlockState, DroppedItemSnapshot, Vec3};
use crate::{ConnectionId, EntityId, HOTBAR_SLOT_COUNT, PlayerId};

const DROPPED_ITEM_PICKUP_DELAY_MS: u64 = 500;
const DROPPED_ITEM_DESPAWN_MS: u64 = 5 * 60 * 1000;

impl ServerCore {
    pub fn apply_gameplay_effect(
        &mut self,
        effect: GameplayEffect,
        now_ms: u64,
    ) -> Vec<TargetedEvent> {
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
                GameplayMutation::OpenChest {
                    player_id,
                    position,
                } => {
                    events.extend(self.open_world_chest(player_id, position));
                }
                GameplayMutation::Block { position, block } => {
                    events.extend(self.apply_block_mutation(position, block));
                }
                GameplayMutation::DroppedItem { position, item } => {
                    events.extend(self.apply_dropped_item_mutation(position, item, now_ms));
                }
            }
        }
        events.extend(effect.emitted_events);
        events
    }

    pub(super) fn apply_player_pose_mutation(
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

    pub(super) fn apply_selected_hotbar_slot_mutation(
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

    pub(super) fn apply_inventory_slot_mutation(
        &mut self,
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    ) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return Vec::new();
        };
        let before_result = player.snapshot.inventory.crafting_result().cloned();
        let _ = player.snapshot.inventory.set_slot(slot, stack.clone());
        if slot.is_crafting_result() || slot.crafting_input_index().is_some() {
            Self::recompute_crafting_result_for_inventory(&mut player.snapshot.inventory);
        }
        let after_result = player.snapshot.inventory.crafting_result().cloned();
        self.saved_players
            .insert(player_id, player.snapshot.clone());
        let mut events = vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::InventorySlotChanged {
                window_id: 0,
                container: InventoryContainer::Player,
                slot,
                stack,
            },
        }];
        if before_result != after_result && !slot.is_crafting_result() {
            events.push(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventorySlotChanged {
                    window_id: 0,
                    container: InventoryContainer::Player,
                    slot: InventorySlot::crafting_result(),
                    stack: after_result,
                },
            });
        }
        events
    }

    pub(super) fn apply_block_mutation(
        &mut self,
        position: BlockPos,
        block: BlockState,
    ) -> Vec<TargetedEvent> {
        self.set_block_at(position, block.clone());
        let mut events = self.close_world_chest_if_invalid(position, &block);
        if block.key.as_str() == crate::catalog::CHEST {
            self.block_entities
                .entry(position)
                .or_insert_with(|| crate::BlockEntityState::chest(27));
        }
        events.extend(self.emit_block_change(position));
        events
    }

    pub(super) fn apply_dropped_item_mutation(
        &mut self,
        position: Vec3,
        item: ItemStack,
        now_ms: u64,
    ) -> Vec<TargetedEvent> {
        let entity_id = EntityId(self.next_entity_id);
        self.next_entity_id = self.next_entity_id.saturating_add(1);
        let snapshot = DroppedItemSnapshot {
            item,
            position,
            velocity: Vec3::new(0.0, 0.0, 0.0),
        };
        self.dropped_items.insert(
            entity_id,
            DroppedItemEntity {
                snapshot: snapshot.clone(),
                pickup_allowed_at_ms: now_ms.saturating_add(DROPPED_ITEM_PICKUP_DELAY_MS),
                despawn_at_ms: now_ms.saturating_add(DROPPED_ITEM_DESPAWN_MS),
            },
        );
        self.dropped_item_spawn_events(entity_id, &snapshot)
    }

    pub(super) fn dropped_item_spawn_events(
        &self,
        entity_id: EntityId,
        item: &DroppedItemSnapshot,
    ) -> Vec<TargetedEvent> {
        self.online_players
            .keys()
            .copied()
            .map(|player_id| TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::DroppedItemSpawned {
                    entity_id,
                    item: item.clone(),
                },
            })
            .collect()
    }

    pub(super) fn dropped_item_spawn_events_for_connection(
        &self,
        connection_id: ConnectionId,
    ) -> Vec<TargetedEvent> {
        self.dropped_items
            .iter()
            .map(|(entity_id, item)| TargetedEvent {
                target: EventTarget::Connection(connection_id),
                event: CoreEvent::DroppedItemSpawned {
                    entity_id: *entity_id,
                    item: item.snapshot.clone(),
                },
            })
            .collect()
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
