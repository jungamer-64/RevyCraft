use super::ServerCore;
use crate::events::{CoreEvent, EventTarget, TargetedEvent};
use crate::gameplay::{GameplayEffect, GameplayMutation};
use crate::player::{
    InteractionHand, InventoryContainer, InventorySlot, ItemStack, PlayerSnapshot,
};
use crate::world::{BlockPos, BlockState, Vec3};
use crate::{HOTBAR_SLOT_COUNT, PlayerId};

impl ServerCore {
    pub fn apply_gameplay_effect(&mut self, effect: GameplayEffect) -> Vec<TargetedEvent> {
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
                GameplayMutation::Block { position, block } => {
                    events.extend(self.apply_block_mutation(position, block));
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
        let _ = player.snapshot.inventory.set_slot(slot, stack.clone());
        self.saved_players
            .insert(player_id, player.snapshot.clone());
        vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::InventorySlotChanged {
                container: InventoryContainer::Player,
                slot,
                stack,
            },
        }]
    }

    pub(super) fn apply_block_mutation(
        &mut self,
        position: BlockPos,
        block: BlockState,
    ) -> Vec<TargetedEvent> {
        self.set_block_at(position, block);
        self.emit_block_change(position)
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
