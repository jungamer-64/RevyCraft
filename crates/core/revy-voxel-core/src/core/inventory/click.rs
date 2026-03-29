use super::super::canonical::{InventoryClickDelta, WorldContainerSyncDelta};
use super::super::state_backend::CoreStateMut;
use super::sync::{active_window_container, resolve_inventory_target, window_contents};
use super::sync_world_container_viewers_state;
use super::util::{MAX_STACK_SIZE, decrement_cursor, stack_keys_match};
use crate::PlayerId;
use crate::events::{
    InventoryClickButton, InventoryClickTarget, InventoryClickValidation,
    InventoryTransactionContext,
};
use crate::inventory::{InventorySlot, InventoryWindowContents, ItemStack, PlayerInventory};
use revy_voxel_rules::ContainerSlotRole;

pub(in crate::core) fn apply_inventory_click_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    transaction: InventoryTransactionContext,
    target: InventoryClickTarget,
    button: InventoryClickButton,
    validation: &InventoryClickValidation,
) -> Option<InventoryClickDelta> {
    let content_behavior = state.content_behavior_arc();
    let Some(before_session) = state.player_session(player_id) else {
        return None;
    };
    let Some(before_snapshot) = state.compose_player_snapshot(player_id) else {
        return None;
    };
    let Some(before_inventory) = state.player_inventory(player_id) else {
        return None;
    };
    let before_cursor = before_session.cursor.clone();
    let Some(container) = active_window_container(
        content_behavior.as_ref(),
        &before_session,
        transaction.window_id,
    ) else {
        let player_container = content_behavior.player_container_kind();
        return Some(InventoryClickDelta {
            player_id,
            transaction,
            accepted: false,
            should_resync_on_reject: false,
            container: player_container.clone(),
            window_id: transaction.window_id,
            resolved_slot: None,
            before_contents: InventoryWindowContents::player(before_inventory.clone()),
            after_contents: InventoryWindowContents::player(before_inventory.clone()),
            before_properties: Vec::new(),
            after_properties: Vec::new(),
            before_cursor,
            after_cursor: before_session.cursor.clone(),
            selected_hotbar_before: before_snapshot.selected_hotbar_slot,
            selected_hotbar_after: before_snapshot.selected_hotbar_slot,
            world_sync: WorldContainerSyncDelta::default(),
        });
    };

    let before_contents = window_contents(
        content_behavior.as_ref(),
        &before_session,
        &before_inventory,
        &container,
    );
    let before_properties = before_session
        .active_container
        .as_ref()
        .filter(|window| window.window_id == transaction.window_id)
        .map(|window| window.property_entries())
        .unwrap_or_default();
    let world_position = before_session
        .active_container
        .as_ref()
        .filter(|window| window.window_id == transaction.window_id)
        .and_then(|window| window.world_position());
    let resolved_slot = resolve_inventory_target(&target);

    let (applied, after_contents, after_properties, after_cursor) = {
        let (session, inventory) = state.player_session_inventory_mut(player_id)?;
        let applied = match transaction.window_id {
            0 => match target {
                InventoryClickTarget::Outside => apply_outside_click(&mut session.cursor, button),
                _ => apply_player_window_click(
                    content_behavior.as_ref(),
                    inventory,
                    &mut session.cursor,
                    resolved_slot,
                    button,
                ),
            },
            _ => apply_active_container_click(
                content_behavior.as_ref(),
                session,
                inventory,
                transaction.window_id,
                resolved_slot,
                &target,
                button,
            ),
        };
        let after_properties = session
            .active_container
            .as_ref()
            .filter(|window| window.window_id == transaction.window_id)
            .map(|window| window.property_entries())
            .unwrap_or_default();
        (
            applied,
            window_contents(content_behavior.as_ref(), session, inventory, &container),
            after_properties,
            session.cursor.clone(),
        )
    };

    let clicked_slot_stack = resolved_slot.and_then(|slot| after_contents.get_slot(slot).cloned());
    let accepted = match validation {
        InventoryClickValidation::Authoritative => applied,
        InventoryClickValidation::StrictSlotEcho { clicked_item } => {
            applied
                && resolved_slot.is_some()
                && clicked_item.as_ref() == clicked_slot_stack.as_ref()
        }
    };

    let selected_hotbar_slot = state.player_selected_hotbar(player_id).unwrap_or(0);
    let world_sync = if accepted {
        world_position
            .map(|position| sync_world_container_viewers_state(state, position, player_id))
            .unwrap_or_default()
    } else {
        WorldContainerSyncDelta::default()
    };

    Some(InventoryClickDelta {
        player_id,
        transaction,
        accepted,
        should_resync_on_reject: true,
        container,
        window_id: transaction.window_id,
        resolved_slot,
        before_contents,
        after_contents,
        before_properties,
        after_properties,
        before_cursor,
        after_cursor,
        selected_hotbar_before: before_snapshot.selected_hotbar_slot,
        selected_hotbar_after: selected_hotbar_slot,
        world_sync,
    })
}

fn apply_player_window_click(
    content_behavior: &dyn revy_voxel_rules::ContentBehavior,
    inventory: &mut PlayerInventory,
    cursor: &mut Option<ItemStack>,
    slot: Option<InventorySlot>,
    button: InventoryClickButton,
) -> bool {
    let Some(slot) = slot else {
        return false;
    };
    let player_container = content_behavior.player_container_kind();
    let Some(spec) = content_behavior.container_spec(&player_container) else {
        return false;
    };

    match slot {
        InventorySlot::WindowLocal(index) if index < spec.local_slot_count => {
            match spec.slot_role(index) {
                ContainerSlotRole::Unavailable => false,
                ContainerSlotRole::OutputOnly => {
                    let mut local_slots =
                        collect_player_local_slots(inventory, spec.local_slot_count);
                    let applied = content_behavior.try_take_output(
                        &player_container,
                        &mut local_slots,
                        cursor,
                        button,
                    );
                    if applied {
                        store_player_local_slots(inventory, &local_slots);
                        content_behavior.normalize_player_inventory(inventory);
                    }
                    applied
                }
                ContainerSlotRole::Generic => {
                    let Some(slot_stack) = inventory.get_slot_mut(slot) else {
                        return false;
                    };
                    let applied = match button {
                        InventoryClickButton::Left => {
                            apply_left_click_slot_value(cursor, slot_stack)
                        }
                        InventoryClickButton::Right => {
                            apply_right_click_slot_value(cursor, slot_stack)
                        }
                    };
                    if applied {
                        content_behavior.normalize_player_inventory(inventory);
                    }
                    applied
                }
            }
        }
        InventorySlot::MainInventory(_) | InventorySlot::Hotbar(_) | InventorySlot::Offhand => {
            let Some(slot_stack) = inventory.get_slot_mut(slot) else {
                return false;
            };
            match button {
                InventoryClickButton::Left => apply_left_click_slot_value(cursor, slot_stack),
                InventoryClickButton::Right => apply_right_click_slot_value(cursor, slot_stack),
            }
        }
        InventorySlot::WindowLocal(_) => false,
    }
}

fn apply_active_container_click(
    content_behavior: &dyn revy_voxel_rules::ContentBehavior,
    session: &mut super::super::PlayerSessionState,
    player_inventory: &mut PlayerInventory,
    window_id: u8,
    slot: Option<InventorySlot>,
    target: &InventoryClickTarget,
    button: InventoryClickButton,
) -> bool {
    if matches!(target, InventoryClickTarget::Outside) {
        return apply_outside_click(&mut session.cursor, button);
    }
    let Some(slot) = slot else {
        return false;
    };
    let Some(window) = session.active_container.as_mut() else {
        return false;
    };
    if window.window_id != window_id {
        return false;
    }

    let kind = window.container.kind.clone();
    let Some(spec) = content_behavior.container_spec(&kind) else {
        return false;
    };

    match slot {
        InventorySlot::WindowLocal(index) if index < spec.local_slot_count => {
            match spec.slot_role(index) {
                ContainerSlotRole::Unavailable => false,
                ContainerSlotRole::OutputOnly => {
                    let applied = content_behavior.try_take_output(
                        &kind,
                        &mut window.container.local_slots,
                        &mut session.cursor,
                        button,
                    );
                    if applied {
                        content_behavior.normalize_container(&mut window.container);
                    }
                    applied
                }
                ContainerSlotRole::Generic => {
                    let Some(slot_stack) = window.local_slot_mut(index) else {
                        return false;
                    };
                    let applied = match button {
                        InventoryClickButton::Left => {
                            apply_left_click_slot_value(&mut session.cursor, slot_stack)
                        }
                        InventoryClickButton::Right => {
                            apply_right_click_slot_value(&mut session.cursor, slot_stack)
                        }
                    };
                    if applied {
                        content_behavior.normalize_container(&mut window.container);
                    }
                    applied
                }
            }
        }
        InventorySlot::MainInventory(_) | InventorySlot::Hotbar(_) | InventorySlot::Offhand => {
            let Some(slot_stack) = player_inventory.get_slot_mut(slot) else {
                return false;
            };
            match button {
                InventoryClickButton::Left => {
                    apply_left_click_slot_value(&mut session.cursor, slot_stack)
                }
                InventoryClickButton::Right => {
                    apply_right_click_slot_value(&mut session.cursor, slot_stack)
                }
            }
        }
        InventorySlot::WindowLocal(_) => false,
    }
}

fn collect_player_local_slots(
    inventory: &PlayerInventory,
    local_slot_count: u16,
) -> Vec<Option<ItemStack>> {
    (0_u16..local_slot_count)
        .map(|index| {
            inventory
                .get_slot(InventorySlot::WindowLocal(index))
                .cloned()
        })
        .collect()
}

fn store_player_local_slots(inventory: &mut PlayerInventory, local_slots: &[Option<ItemStack>]) {
    for (index, stack) in local_slots.iter().cloned().enumerate() {
        let index = u16::try_from(index).expect("player local slot index fits into u16");
        let _ = inventory.set_slot(InventorySlot::WindowLocal(index), stack);
    }
}

fn apply_outside_click(cursor: &mut Option<ItemStack>, button: InventoryClickButton) -> bool {
    match button {
        InventoryClickButton::Left => {
            *cursor = None;
            true
        }
        InventoryClickButton::Right => {
            decrement_cursor(cursor);
            true
        }
    }
}

fn apply_left_click_slot_value(
    cursor: &mut Option<ItemStack>,
    slot_stack: &mut Option<ItemStack>,
) -> bool {
    match (cursor.as_ref(), slot_stack.as_ref()) {
        (None, None) => true,
        (None, Some(_)) => {
            *cursor = slot_stack.take();
            true
        }
        (Some(_), None) => {
            *slot_stack = cursor.take();
            true
        }
        (Some(cursor_stack), Some(existing_stack))
            if stack_keys_match(cursor_stack, existing_stack) =>
        {
            let total = u16::from(cursor_stack.count) + u16::from(existing_stack.count);
            let placed = total.min(u16::from(MAX_STACK_SIZE));
            let remainder = total.saturating_sub(placed);
            let mut next_slot = existing_stack.clone();
            next_slot.count = u8::try_from(placed).expect("placed stack count should fit into u8");
            *slot_stack = Some(next_slot);
            if remainder == 0 {
                *cursor = None;
            } else if let Some(cursor_stack) = cursor.as_mut() {
                cursor_stack.count =
                    u8::try_from(remainder).expect("cursor remainder should fit into u8");
            }
            true
        }
        (Some(_), Some(_)) => {
            std::mem::swap(cursor, slot_stack);
            true
        }
    }
}

fn apply_right_click_slot_value(
    cursor: &mut Option<ItemStack>,
    slot_stack: &mut Option<ItemStack>,
) -> bool {
    match (cursor.as_ref(), slot_stack.as_ref()) {
        (None, None) => true,
        (None, Some(existing_stack)) => {
            let cursor_count = (existing_stack.count.saturating_add(1)) / 2;
            let slot_count = existing_stack.count / 2;
            let mut next_cursor = existing_stack.clone();
            next_cursor.count = cursor_count;
            *cursor = Some(next_cursor);
            if slot_count == 0 {
                *slot_stack = None;
            } else {
                let mut next_slot = existing_stack.clone();
                next_slot.count = slot_count;
                *slot_stack = Some(next_slot);
            }
            true
        }
        (Some(cursor_stack), None) => {
            let mut placed = cursor_stack.clone();
            placed.count = 1;
            *slot_stack = Some(placed);
            decrement_cursor(cursor);
            true
        }
        (Some(cursor_stack), Some(existing_stack))
            if stack_keys_match(cursor_stack, existing_stack)
                && existing_stack.count < MAX_STACK_SIZE =>
        {
            let mut next_slot = existing_stack.clone();
            next_slot.count = next_slot.count.saturating_add(1);
            *slot_stack = Some(next_slot);
            decrement_cursor(cursor);
            true
        }
        (Some(_), Some(_)) => false,
    }
}
