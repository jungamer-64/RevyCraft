use super::super::PlayerSessionState;
use super::super::canonical::InventoryClickDelta;
use super::super::state_backend::CoreStateMut;
use super::crafting::{
    apply_active_container_crafting_result_click, apply_player_crafting_result_click,
    recompute_crafting_result_for_active_container, recompute_player_crafting_result,
};
use super::furnace::normalize_furnace_window;
use super::state::OpenInventoryWindow;
use super::sync::{active_window_container, resolve_inventory_target, window_contents};
use super::util::{MAX_STACK_SIZE, decrement_cursor, reduce_slot_stack, stack_keys_match};
use super::{sync_world_chest_viewers_state, sync_world_furnace_state};
use crate::PlayerId;
use crate::events::{
    InventoryClickButton, InventoryClickTarget, InventoryClickValidation,
    InventoryTransactionContext,
};
use crate::inventory::{
    InventoryContainer, InventorySlot, InventoryWindowContents, ItemStack, PlayerInventory,
};

pub(in crate::core) fn apply_inventory_click_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    transaction: InventoryTransactionContext,
    target: InventoryClickTarget,
    button: InventoryClickButton,
    validation: &InventoryClickValidation,
) -> Option<InventoryClickDelta> {
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
    let Some(container) = active_window_container(&before_session, transaction.window_id) else {
        return Some(InventoryClickDelta {
            player_id,
            transaction,
            accepted: false,
            should_resync_on_reject: false,
            container: InventoryContainer::Player,
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
            viewer_syncs: Vec::new(),
        });
    };
    let before_contents = window_contents(&before_session, &before_inventory, container);
    let before_properties = before_session
        .active_container
        .as_ref()
        .filter(|window| window.window_id == transaction.window_id)
        .map(OpenInventoryWindow::property_entries)
        .unwrap_or_default();
    let world_chest_position = before_session
        .active_container
        .as_ref()
        .filter(|window| window.window_id == transaction.window_id)
        .and_then(OpenInventoryWindow::world_chest_position);
    let world_furnace_position = before_session
        .active_container
        .as_ref()
        .filter(|window| window.window_id == transaction.window_id)
        .and_then(OpenInventoryWindow::world_furnace_position);
    let resolved_slot = resolve_inventory_target(&target);

    let (applied, after_contents, after_properties, after_cursor) = {
        let (session, inventory) = state.player_session_inventory_mut(player_id)?;
        let applied = match transaction.window_id {
            0 => match target {
                InventoryClickTarget::Outside => apply_outside_click(&mut session.cursor, button),
                _ => {
                    apply_player_window_click(inventory, &mut session.cursor, resolved_slot, button)
                }
            },
            _ => apply_active_container_click(
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
            .map(OpenInventoryWindow::property_entries)
            .unwrap_or_default();
        (
            applied,
            window_contents(session, inventory, container),
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
    let viewer_syncs = if accepted {
        let mut deltas = Vec::new();
        if let Some(position) = world_chest_position {
            deltas.extend(sync_world_chest_viewers_state(state, position, player_id));
        }
        if let Some(position) = world_furnace_position {
            sync_world_furnace_state(state, position, player_id);
        }
        deltas
    } else {
        Vec::new()
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
        viewer_syncs,
    })
}

fn apply_player_window_click(
    inventory: &mut PlayerInventory,
    cursor: &mut Option<ItemStack>,
    slot: Option<InventorySlot>,
    button: InventoryClickButton,
) -> bool {
    let Some(slot) = slot else {
        return false;
    };
    if slot.is_reserved_auxiliary() || slot.container_index().is_some() {
        return false;
    }

    if slot.is_crafting_result() {
        return apply_player_crafting_result_click(inventory, cursor, button);
    }

    let Some(slot_stack) = inventory.get_slot_mut(slot) else {
        return false;
    };
    let applied = match button {
        InventoryClickButton::Left => apply_left_click_slot_value(cursor, slot_stack),
        InventoryClickButton::Right => apply_right_click_slot_value(cursor, slot_stack),
    };
    if applied && slot.crafting_input_index().is_some() {
        recompute_player_crafting_result(inventory);
    }
    applied
}

fn apply_active_container_click(
    session: &mut PlayerSessionState,
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

    match window.container {
        InventoryContainer::CraftingTable => {
            apply_crafting_table_click(window, player_inventory, &mut session.cursor, slot, button)
        }
        InventoryContainer::Chest => {
            apply_chest_click(window, player_inventory, &mut session.cursor, slot, button)
        }
        InventoryContainer::Furnace => {
            apply_furnace_click(window, player_inventory, &mut session.cursor, slot, button)
        }
        InventoryContainer::Player => false,
    }
}

fn apply_crafting_table_click(
    window: &mut OpenInventoryWindow,
    player_inventory: &mut PlayerInventory,
    cursor: &mut Option<ItemStack>,
    slot: InventorySlot,
    button: InventoryClickButton,
) -> bool {
    match slot {
        InventorySlot::Container(0) => {
            apply_active_container_crafting_result_click(window, cursor, button)
        }
        InventorySlot::Container(index) => {
            let Some(slot_stack) = window.local_slot_mut(index) else {
                return false;
            };
            let applied = match button {
                InventoryClickButton::Left => apply_left_click_slot_value(cursor, slot_stack),
                InventoryClickButton::Right => apply_right_click_slot_value(cursor, slot_stack),
            };
            if applied {
                recompute_crafting_result_for_active_container(window);
            }
            applied
        }
        InventorySlot::MainInventory(_) | InventorySlot::Hotbar(_) => {
            let Some(slot_stack) = player_inventory.get_slot_mut(slot) else {
                return false;
            };
            match button {
                InventoryClickButton::Left => apply_left_click_slot_value(cursor, slot_stack),
                InventoryClickButton::Right => apply_right_click_slot_value(cursor, slot_stack),
            }
        }
        InventorySlot::Auxiliary(_) | InventorySlot::Offhand => false,
    }
}

fn apply_chest_click(
    window: &mut OpenInventoryWindow,
    player_inventory: &mut PlayerInventory,
    cursor: &mut Option<ItemStack>,
    slot: InventorySlot,
    button: InventoryClickButton,
) -> bool {
    match slot {
        InventorySlot::Container(index) => {
            let Some(slot_stack) = window.local_slot_mut(index) else {
                return false;
            };
            match button {
                InventoryClickButton::Left => apply_left_click_slot_value(cursor, slot_stack),
                InventoryClickButton::Right => apply_right_click_slot_value(cursor, slot_stack),
            }
        }
        InventorySlot::MainInventory(_) | InventorySlot::Hotbar(_) => {
            let Some(slot_stack) = player_inventory.get_slot_mut(slot) else {
                return false;
            };
            match button {
                InventoryClickButton::Left => apply_left_click_slot_value(cursor, slot_stack),
                InventoryClickButton::Right => apply_right_click_slot_value(cursor, slot_stack),
            }
        }
        InventorySlot::Auxiliary(_) | InventorySlot::Offhand => false,
    }
}

fn apply_furnace_click(
    window: &mut OpenInventoryWindow,
    player_inventory: &mut PlayerInventory,
    cursor: &mut Option<ItemStack>,
    slot: InventorySlot,
    button: InventoryClickButton,
) -> bool {
    match slot {
        InventorySlot::Container(index @ 0..=1) => {
            let Some(slot_stack) = window.local_slot_mut(index) else {
                return false;
            };
            let applied = match button {
                InventoryClickButton::Left => apply_left_click_slot_value(cursor, slot_stack),
                InventoryClickButton::Right => apply_right_click_slot_value(cursor, slot_stack),
            };
            if applied {
                normalize_furnace_window(window);
            }
            applied
        }
        InventorySlot::Container(2) => {
            let Some(slot_stack) = window.local_slot_mut(2) else {
                return false;
            };
            apply_take_only_slot_value(cursor, slot_stack, button)
        }
        InventorySlot::MainInventory(_) | InventorySlot::Hotbar(_) => {
            let Some(slot_stack) = player_inventory.get_slot_mut(slot) else {
                return false;
            };
            match button {
                InventoryClickButton::Left => apply_left_click_slot_value(cursor, slot_stack),
                InventoryClickButton::Right => apply_right_click_slot_value(cursor, slot_stack),
            }
        }
        InventorySlot::Auxiliary(_) | InventorySlot::Offhand => false,
        InventorySlot::Container(_) => false,
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
        _ => false,
    }
}

fn apply_take_only_slot_value(
    cursor: &mut Option<ItemStack>,
    slot_stack: &mut Option<ItemStack>,
    button: InventoryClickButton,
) -> bool {
    match (cursor.as_ref(), slot_stack.as_ref()) {
        (None, None) => true,
        (Some(_), None) => false,
        (None, Some(existing_stack)) => {
            let take_count = match button {
                InventoryClickButton::Left => existing_stack.count,
                InventoryClickButton::Right => 1,
            };
            let mut taken = existing_stack.clone();
            taken.count = take_count;
            *cursor = Some(taken);
            reduce_slot_stack(slot_stack, take_count);
            true
        }
        (Some(cursor_stack), Some(existing_stack))
            if stack_keys_match(cursor_stack, existing_stack) =>
        {
            let take_count = match button {
                InventoryClickButton::Left => existing_stack.count,
                InventoryClickButton::Right => 1,
            };
            let total = u16::from(cursor_stack.count) + u16::from(take_count);
            if total > u16::from(MAX_STACK_SIZE) {
                return false;
            }
            if let Some(cursor_stack) = cursor.as_mut() {
                cursor_stack.count =
                    u8::try_from(total).expect("output cursor stack should fit into u8");
            }
            reduce_slot_stack(slot_stack, take_count);
            true
        }
        _ => false,
    }
}
