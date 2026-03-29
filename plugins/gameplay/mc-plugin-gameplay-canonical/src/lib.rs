#![allow(clippy::multiple_crate_versions)]
use mc_content_api::ContainerKindId;
use mc_content_canonical::{
    canonical_content, item_supported_for_inventory, placeable_block_state_from_item_key,
};
use mc_core::{
    CoreEvent, EventTarget, GameplayCapability, GameplayCommand, PlayerId, PlayerSnapshot,
    TargetedEvent,
};
use mc_model::{BlockFace, BlockPos, InteractionHand, InventorySlot, ItemStack};
use mc_plugin_sdk_rust::capabilities;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::gameplay::{self, GameplayHost, RustGameplayPlugin};
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;

#[derive(Default)]
pub struct CanonicalGameplayPlugin;

const HOTBAR_SLOT_COUNT: u8 = 9;

fn player_container_kind() -> ContainerKindId {
    canonical_content().player_container_kind()
}

impl RustGameplayPlugin for CanonicalGameplayPlugin {
    fn descriptor(&self) -> mc_plugin_api::codec::gameplay::GameplayDescriptor {
        gameplay::gameplay_descriptor("canonical")
    }

    fn capability_set(&self) -> mc_core::GameplayCapabilitySet {
        capabilities::gameplay_capabilities(&[GameplayCapability::RuntimeReload])
    }

    fn handle_player_join(
        &self,
        _host: &dyn GameplayHost,
        _session: &mc_plugin_api::codec::gameplay::GameplaySessionSnapshot,
        _player_id: PlayerId,
    ) -> Result<(), String> {
        Ok(())
    }

    fn handle_command(
        &self,
        host: &dyn GameplayHost,
        _session: &mc_plugin_api::codec::gameplay::GameplaySessionSnapshot,
        command: &GameplayCommand,
    ) -> Result<(), String> {
        match command {
            GameplayCommand::MoveIntent {
                player_id,
                position,
                yaw,
                pitch,
                on_ground,
            } => host.set_player_pose(*player_id, *position, *yaw, *pitch, *on_ground),
            GameplayCommand::SetHeldSlot { player_id, slot } => {
                set_held_slot(host, *player_id, *slot)
            }
            GameplayCommand::CreativeInventorySet {
                player_id,
                slot,
                stack,
            } => creative_inventory_set(host, *player_id, *slot, stack.as_ref()),
            GameplayCommand::DigBlock {
                player_id,
                position,
                status,
                ..
            } => dig_block(host, *player_id, *position, *status),
            GameplayCommand::PlaceBlock {
                player_id,
                hand,
                position,
                face,
                held_item,
            } => place_block(
                host,
                *player_id,
                *hand,
                *position,
                *face,
                held_item.as_ref(),
            ),
            GameplayCommand::UseBlock {
                player_id,
                hand,
                position,
                face,
                held_item,
            } => use_block(
                host,
                *player_id,
                *hand,
                *position,
                *face,
                held_item.as_ref(),
            ),
        }
    }

    fn handle_tick(
        &self,
        _host: &dyn GameplayHost,
        _session: &mc_plugin_api::codec::gameplay::GameplaySessionSnapshot,
        _now_ms: u64,
    ) -> Result<(), String> {
        Ok(())
    }

    fn export_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &mc_plugin_api::codec::gameplay::GameplaySessionSnapshot,
    ) -> Result<Vec<u8>, String> {
        Ok(option_env!("REVY_PLUGIN_BUILD_TAG")
            .unwrap_or("canonical")
            .as_bytes()
            .to_vec())
    }

    fn import_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &mc_plugin_api::codec::gameplay::GameplaySessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), String> {
        if capabilities::build_tag_contains("reload-fail") {
            return Err("canonical gameplay plugin refused session import".to_string());
        }
        Ok(())
    }
}

fn set_held_slot(host: &dyn GameplayHost, player_id: PlayerId, slot: i16) -> Result<(), String> {
    let Some(player) = host.read_player_snapshot(player_id)? else {
        return Ok(());
    };
    let Ok(slot) = u8::try_from(slot) else {
        return host.emit_event(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::SelectedHotbarSlotChanged {
                slot: player.selected_hotbar_slot,
            },
        });
    };
    if slot >= HOTBAR_SLOT_COUNT {
        return host.emit_event(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::SelectedHotbarSlotChanged {
                slot: player.selected_hotbar_slot,
            },
        });
    }
    host.clear_mining(player_id)?;
    host.set_selected_hotbar_slot(player_id, slot)
}

fn creative_inventory_set(
    host: &dyn GameplayHost,
    player_id: PlayerId,
    slot: InventorySlot,
    stack: Option<&ItemStack>,
) -> Result<(), String> {
    let Some(player) = host.read_player_snapshot(player_id)? else {
        return Ok(());
    };
    if host.read_world_meta()?.game_mode != 1
        || !slot.is_storage_slot()
        || stack.is_some_and(|stack| {
            !item_supported_for_inventory(stack.key.as_str())
                || stack.count == 0
                || stack.count > 64
        })
    {
        return reject_inventory_slot_events(host, player_id, slot, &player);
    }
    host.set_inventory_slot(player_id, slot, stack.cloned())
}

fn dig_block(
    host: &dyn GameplayHost,
    player_id: PlayerId,
    position: BlockPos,
    status: u8,
) -> Result<(), String> {
    let content = canonical_content();
    if !matches!(status, 0..=2) {
        return Ok(());
    }
    let Some(player) = host.read_player_snapshot(player_id)? else {
        return Ok(());
    };
    if status == 1 {
        return host.clear_mining(player_id);
    }
    let current = host.read_block_state(position)?;
    let protected_container = current
        .as_ref()
        .and_then(|block| content.container_kind_for_block(block))
        .is_some()
        && host
            .read_block_entity(position)?
            .is_some_and(|entity| entity.has_inventory_contents());
    if !host.can_edit_block(player_id, position)?
        || current
            .as_ref()
            .is_none_or(|block| content.is_air_block(block))
        || current
            .as_ref()
            .is_some_and(|block| content.is_unbreakable_block(block))
        || protected_container
    {
        host.clear_mining(player_id)?;
        return host.emit_event(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::BlockChanged {
                position,
                block: current,
            },
        });
    }
    if host.read_world_meta()?.game_mode == 1 {
        host.clear_mining(player_id)?;
        return host.set_block(position, None);
    }
    let current = current.ok_or_else(|| "missing block after mining validation".to_string())?;
    let duration_ms = content
        .survival_mining_duration_ms(
            &current,
            content.tool_spec_for_item(
                player
                    .inventory
                    .selected_hotbar_stack(player.selected_hotbar_slot),
            ),
        )
        .unwrap_or(50);
    host.begin_mining(player_id, position, duration_ms)
}

fn place_block(
    host: &dyn GameplayHost,
    player_id: PlayerId,
    hand: InteractionHand,
    position: BlockPos,
    face: Option<BlockFace>,
    held_item: Option<&ItemStack>,
) -> Result<(), String> {
    let content = canonical_content();
    let Some(face) = face else {
        return Ok(());
    };
    let Some(player) = host.read_player_snapshot(player_id)? else {
        return Ok(());
    };
    let place_pos = position.offset(face);
    let Some(selected_stack) = player
        .inventory
        .selected_stack(player.selected_hotbar_slot, hand)
        .cloned()
    else {
        return place_rejection(host, player_id, hand, place_pos, &player);
    };
    if held_item.is_some_and(|held_item| held_item != &selected_stack) {
        return place_rejection(host, player_id, hand, place_pos, &player);
    }
    let Some(block) = placeable_block_state_from_item_key(selected_stack.key.as_str()) else {
        return place_rejection(host, player_id, hand, place_pos, &player);
    };
    let target_block = host.read_block_state(position)?;
    let place_block = host.read_block_state(place_pos)?;
    if !host.can_edit_block(player_id, place_pos)?
        || target_block
            .as_ref()
            .is_none_or(|block| content.is_air_block(block))
        || !place_block
            .as_ref()
            .is_none_or(|block| content.is_air_block(block))
    {
        return place_rejection(host, player_id, hand, place_pos, &player);
    }
    host.clear_mining(player_id)?;
    host.set_block(place_pos, Some(block))?;
    if host.read_world_meta()?.game_mode != 1 {
        host.set_inventory_slot(
            player_id,
            held_inventory_slot(player.selected_hotbar_slot, hand),
            consumed_stack(&selected_stack),
        )?;
    }
    Ok(())
}

fn use_block(
    host: &dyn GameplayHost,
    player_id: PlayerId,
    hand: InteractionHand,
    position: BlockPos,
    face: Option<BlockFace>,
    held_item: Option<&ItemStack>,
) -> Result<(), String> {
    let content = canonical_content();
    let target_block = host.read_block_state(position)?;
    if target_block
        .as_ref()
        .and_then(|block| content.container_kind_for_block(block))
        .is_some()
    {
        if !host.can_edit_block(player_id, position)? {
            return host.emit_event(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::BlockChanged {
                    position,
                    block: target_block,
                },
            });
        }
        host.clear_mining(player_id)?;
        return host.open_container_at(player_id, position);
    }
    place_block(host, player_id, hand, position, face, held_item)
}

fn reject_inventory_slot_events(
    host: &dyn GameplayHost,
    player_id: PlayerId,
    slot: InventorySlot,
    player: &PlayerSnapshot,
) -> Result<(), String> {
    host.emit_event(TargetedEvent {
        target: EventTarget::Player(player_id),
        event: CoreEvent::InventorySlotChanged {
            window_id: 0,
            container: player_container_kind(),
            slot,
            stack: player.inventory.get_slot(slot).cloned(),
        },
    })?;
    host.emit_event(TargetedEvent {
        target: EventTarget::Player(player_id),
        event: CoreEvent::SelectedHotbarSlotChanged {
            slot: player.selected_hotbar_slot,
        },
    })
}

fn place_rejection(
    host: &dyn GameplayHost,
    player_id: PlayerId,
    hand: InteractionHand,
    place_pos: BlockPos,
    player: &PlayerSnapshot,
) -> Result<(), String> {
    host.emit_event(TargetedEvent {
        target: EventTarget::Player(player_id),
        event: CoreEvent::BlockChanged {
            position: place_pos,
            block: host.read_block_state(place_pos)?,
        },
    })?;
    place_inventory_correction(host, player_id, hand, player)
}

fn place_inventory_correction(
    host: &dyn GameplayHost,
    player_id: PlayerId,
    hand: InteractionHand,
    player: &PlayerSnapshot,
) -> Result<(), String> {
    let selected_slot = held_inventory_slot(player.selected_hotbar_slot, hand);
    host.emit_event(TargetedEvent {
        target: EventTarget::Player(player_id),
        event: CoreEvent::InventorySlotChanged {
            window_id: 0,
            container: player_container_kind(),
            slot: selected_slot,
            stack: player.inventory.get_slot(selected_slot).cloned(),
        },
    })?;
    host.emit_event(TargetedEvent {
        target: EventTarget::Player(player_id),
        event: CoreEvent::SelectedHotbarSlotChanged {
            slot: player.selected_hotbar_slot,
        },
    })
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

const MANIFEST: StaticPluginManifest = StaticPluginManifest::gameplay(
    "gameplay-canonical",
    "Canonical Gameplay Plugin",
    "canonical",
);

export_plugin!(gameplay, CanonicalGameplayPlugin, MANIFEST);
