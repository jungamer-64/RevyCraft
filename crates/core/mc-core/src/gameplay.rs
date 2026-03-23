use crate::catalog;
use crate::core::ServerCore;
use crate::events::{CoreCommand, CoreEvent, EventTarget, TargetedEvent};
use crate::player::{
    InteractionHand, InventoryContainer, InventorySlot, ItemStack, PlayerInventory, PlayerSnapshot,
};
use crate::world::{BlockFace, BlockPos, BlockState, Vec3, WorldMeta};
use crate::{CapabilitySet, GameplayProfileId, HOTBAR_SLOT_COUNT, PlayerId, SessionCapabilitySet};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameplayJoinEffect {
    pub inventory: Option<PlayerInventory>,
    pub selected_hotbar_slot: Option<u8>,
    pub emitted_events: Vec<TargetedEvent>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameplayEffect {
    pub mutations: Vec<GameplayMutation>,
    pub emitted_events: Vec<TargetedEvent>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GameplayMutation {
    PlayerPose {
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    },
    SelectedHotbarSlot {
        player_id: PlayerId,
        slot: u8,
    },
    InventorySlot {
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    },
    Block {
        position: BlockPos,
        block: BlockState,
    },
}

pub trait GameplayQuery {
    fn world_meta(&self) -> WorldMeta;
    fn player_snapshot(&self, player_id: PlayerId) -> Option<PlayerSnapshot>;
    fn block_state(&self, position: BlockPos) -> BlockState;
    fn can_edit_block(&self, player_id: PlayerId, position: BlockPos) -> bool;
}

pub trait GameplayPolicyResolver: Send + Sync {
    /// Produces join-time gameplay effects for a player snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the gameplay policy cannot evaluate the join flow for the
    /// provided query state or session capabilities.
    fn handle_player_join(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        player: &PlayerSnapshot,
    ) -> Result<GameplayJoinEffect, String>;

    /// Produces gameplay effects for a player-owned command.
    ///
    /// # Errors
    ///
    /// Returns an error when the gameplay policy cannot evaluate the command for the
    /// provided query state or session capabilities.
    fn handle_command(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        command: &CoreCommand,
    ) -> Result<GameplayEffect, String>;

    /// Produces gameplay effects for a player tick.
    ///
    /// # Errors
    ///
    /// Returns an error when the gameplay policy cannot evaluate the tick for the
    /// provided query state or session capabilities.
    fn handle_tick(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        player_id: PlayerId,
        now_ms: u64,
    ) -> Result<GameplayEffect, String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CanonicalGameplayPolicy;

#[derive(Clone, Copy, Debug, Default)]
pub struct ReadonlyGameplayPolicy;

impl CanonicalGameplayPolicy {
    fn move_intent_effect(
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    ) -> GameplayEffect {
        GameplayEffect {
            mutations: vec![GameplayMutation::PlayerPose {
                player_id,
                position,
                yaw,
                pitch,
                on_ground,
            }],
            emitted_events: Vec::new(),
        }
    }

    fn set_held_slot_effect(
        query: &dyn GameplayQuery,
        player_id: PlayerId,
        slot: i16,
    ) -> GameplayEffect {
        let Some(player) = query.player_snapshot(player_id) else {
            return GameplayEffect::default();
        };
        let Ok(slot) = u8::try_from(slot) else {
            return Self::rejected_held_slot_effect(player_id, player.selected_hotbar_slot);
        };
        if slot >= HOTBAR_SLOT_COUNT {
            return Self::rejected_held_slot_effect(player_id, player.selected_hotbar_slot);
        }
        GameplayEffect {
            mutations: vec![GameplayMutation::SelectedHotbarSlot { player_id, slot }],
            emitted_events: Vec::new(),
        }
    }

    fn creative_inventory_set_effect(
        query: &dyn GameplayQuery,
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<&ItemStack>,
    ) -> GameplayEffect {
        let Some(player) = query.player_snapshot(player_id) else {
            return GameplayEffect::default();
        };
        if query.world_meta().game_mode != 1
            || !slot.is_storage_slot()
            || stack.is_some_and(|stack| {
                !stack.is_supported_inventory_item() || stack.count == 0 || stack.count > 64
            })
        {
            return GameplayEffect {
                mutations: Vec::new(),
                emitted_events: reject_inventory_slot_events_snapshot(player_id, slot, &player),
            };
        }
        GameplayEffect {
            mutations: vec![GameplayMutation::InventorySlot {
                player_id,
                slot,
                stack: stack.cloned(),
            }],
            emitted_events: Vec::new(),
        }
    }

    fn dig_block_effect(
        query: &dyn GameplayQuery,
        player_id: PlayerId,
        position: BlockPos,
        status: u8,
    ) -> GameplayEffect {
        if !matches!(status, 0 | 2) {
            return GameplayEffect::default();
        }
        if query.player_snapshot(player_id).is_none() {
            return GameplayEffect::default();
        }
        if query.world_meta().game_mode != 1 || !query.can_edit_block(player_id, position) {
            return Self::block_changed_effect(player_id, position, query.block_state(position));
        }
        let current = query.block_state(position);
        if current.is_air() || current.key.as_str() == "minecraft:bedrock" {
            return Self::block_changed_effect(player_id, position, current);
        }
        GameplayEffect {
            mutations: vec![GameplayMutation::Block {
                position,
                block: BlockState::air(),
            }],
            emitted_events: Vec::new(),
        }
    }

    fn place_block_effect(
        query: &dyn GameplayQuery,
        player_id: PlayerId,
        hand: InteractionHand,
        position: BlockPos,
        face: Option<BlockFace>,
        held_item: Option<&ItemStack>,
    ) -> GameplayEffect {
        let Some(face) = face else {
            return GameplayEffect::default();
        };
        let Some(player) = query.player_snapshot(player_id) else {
            return GameplayEffect::default();
        };
        let place_pos = position.offset(face);
        let Some(selected_stack) = player
            .inventory
            .selected_stack(player.selected_hotbar_slot, hand)
            .cloned()
        else {
            return Self::place_rejection_effect(query, player_id, hand, place_pos, &player);
        };
        if held_item.is_some_and(|held_item| held_item != &selected_stack) {
            return Self::place_rejection_effect(query, player_id, hand, place_pos, &player);
        }
        let Some(block) = catalog::placeable_block_state_from_item_key(selected_stack.key.as_str())
        else {
            return Self::place_rejection_effect(query, player_id, hand, place_pos, &player);
        };
        if query.world_meta().game_mode != 1
            || !query.can_edit_block(player_id, place_pos)
            || query.block_state(position).is_air()
            || !query.block_state(place_pos).is_air()
        {
            return Self::place_rejection_effect(query, player_id, hand, place_pos, &player);
        }
        GameplayEffect {
            mutations: vec![GameplayMutation::Block {
                position: place_pos,
                block,
            }],
            emitted_events: Vec::new(),
        }
    }

    fn rejected_held_slot_effect(player_id: PlayerId, slot: u8) -> GameplayEffect {
        GameplayEffect {
            mutations: Vec::new(),
            emitted_events: vec![TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::SelectedHotbarSlotChanged { slot },
            }],
        }
    }

    fn block_changed_effect(
        player_id: PlayerId,
        position: BlockPos,
        block: BlockState,
    ) -> GameplayEffect {
        GameplayEffect {
            mutations: Vec::new(),
            emitted_events: vec![TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::BlockChanged { position, block },
            }],
        }
    }

    fn place_rejection_effect(
        query: &dyn GameplayQuery,
        player_id: PlayerId,
        hand: InteractionHand,
        place_pos: BlockPos,
        player: &PlayerSnapshot,
    ) -> GameplayEffect {
        GameplayEffect {
            mutations: Vec::new(),
            emitted_events: place_rejection_events_snapshot(
                query, player_id, hand, place_pos, player,
            ),
        }
    }
}

impl GameplayPolicyResolver for CanonicalGameplayPolicy {
    fn handle_player_join(
        &self,
        _query: &dyn GameplayQuery,
        _session: &SessionCapabilitySet,
        _player: &PlayerSnapshot,
    ) -> Result<GameplayJoinEffect, String> {
        Ok(GameplayJoinEffect::default())
    }

    fn handle_command(
        &self,
        query: &dyn GameplayQuery,
        _session: &SessionCapabilitySet,
        command: &CoreCommand,
    ) -> Result<GameplayEffect, String> {
        match command {
            CoreCommand::MoveIntent {
                player_id,
                position,
                yaw,
                pitch,
                on_ground,
            } => Ok(Self::move_intent_effect(
                *player_id, *position, *yaw, *pitch, *on_ground,
            )),
            CoreCommand::SetHeldSlot { player_id, slot } => {
                Ok(Self::set_held_slot_effect(query, *player_id, *slot))
            }
            CoreCommand::CreativeInventorySet {
                player_id,
                slot,
                stack,
            } => Ok(Self::creative_inventory_set_effect(
                query,
                *player_id,
                *slot,
                stack.as_ref(),
            )),
            CoreCommand::DigBlock {
                player_id,
                position,
                status,
                ..
            } => Ok(Self::dig_block_effect(
                query, *player_id, *position, *status,
            )),
            CoreCommand::PlaceBlock {
                player_id,
                hand,
                position,
                face,
                held_item,
            } => Ok(Self::place_block_effect(
                query,
                *player_id,
                *hand,
                *position,
                *face,
                held_item.as_ref(),
            )),
            _ => Ok(GameplayEffect::default()),
        }
    }

    fn handle_tick(
        &self,
        _query: &dyn GameplayQuery,
        _session: &SessionCapabilitySet,
        _player_id: PlayerId,
        _now_ms: u64,
    ) -> Result<GameplayEffect, String> {
        Ok(GameplayEffect::default())
    }
}

impl GameplayPolicyResolver for ReadonlyGameplayPolicy {
    fn handle_player_join(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        player: &PlayerSnapshot,
    ) -> Result<GameplayJoinEffect, String> {
        CanonicalGameplayPolicy.handle_player_join(query, session, player)
    }

    fn handle_command(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        command: &CoreCommand,
    ) -> Result<GameplayEffect, String> {
        match command {
            CoreCommand::MoveIntent { .. } | CoreCommand::SetHeldSlot { .. } => {
                CanonicalGameplayPolicy.handle_command(query, session, command)
            }
            _ => Ok(GameplayEffect::default()),
        }
    }

    fn handle_tick(
        &self,
        _query: &dyn GameplayQuery,
        _session: &SessionCapabilitySet,
        _player_id: PlayerId,
        _now_ms: u64,
    ) -> Result<GameplayEffect, String> {
        Ok(GameplayEffect::default())
    }
}

pub fn canonical_session_capabilities() -> SessionCapabilitySet {
    let mut gameplay = CapabilitySet::new();
    let _ = gameplay.insert("gameplay.profile.canonical");
    SessionCapabilitySet {
        protocol: CapabilitySet::new(),
        gameplay,
        gameplay_profile: GameplayProfileId::new("canonical"),
        entity_id: None,
        protocol_generation: None,
        gameplay_generation: None,
    }
}

fn reject_inventory_slot_events_snapshot(
    player_id: PlayerId,
    slot: InventorySlot,
    player: &PlayerSnapshot,
) -> Vec<TargetedEvent> {
    vec![
        TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::InventorySlotChanged {
                container: InventoryContainer::Player,
                slot,
                stack: player.inventory.get_slot(slot).cloned(),
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

fn place_rejection_events_snapshot(
    query: &dyn GameplayQuery,
    player_id: PlayerId,
    hand: InteractionHand,
    place_pos: BlockPos,
    player: &PlayerSnapshot,
) -> Vec<TargetedEvent> {
    let mut events = vec![TargetedEvent {
        target: EventTarget::Player(player_id),
        event: CoreEvent::BlockChanged {
            position: place_pos,
            block: query.block_state(place_pos),
        },
    }];
    events.extend(ServerCore::place_inventory_correction(
        player_id, hand, player,
    ));
    events
}
