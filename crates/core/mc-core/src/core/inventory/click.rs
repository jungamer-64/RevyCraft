use super::super::{OnlinePlayer, ServerCore};
use super::crafting::{
    apply_active_container_crafting_result_click, apply_player_crafting_result_click,
    recompute_crafting_result_for_active_container, recompute_player_crafting_result,
};
use super::furnace::normalize_furnace_window;
use super::state::OpenInventoryWindow;
use super::sync::{
    active_window_container, inventory_diff_events, property_diff_events, resolve_inventory_target,
    window_contents,
};
use super::util::{MAX_STACK_SIZE, decrement_cursor, reduce_slot_stack, stack_keys_match};
use crate::events::{
    CoreEvent, EventTarget, InventoryClickButton, InventoryClickTarget,
    InventoryTransactionContext, TargetedEvent,
};
use crate::inventory::{InventoryContainer, InventorySlot, ItemStack, PlayerInventory};
use crate::{PlayerId, ProtocolCapability, SessionCapabilitySet};

impl ServerCore {
    pub(in crate::core) fn apply_inventory_click(
        &mut self,
        player_id: PlayerId,
        transaction: InventoryTransactionContext,
        target: InventoryClickTarget,
        button: InventoryClickButton,
        clicked_item: Option<&ItemStack>,
        session: Option<&SessionCapabilitySet>,
    ) -> Vec<TargetedEvent> {
        let Some(before_player) = self.online_players.get(&player_id) else {
            return Vec::new();
        };
        let before_snapshot = before_player.snapshot.clone();
        let before_cursor = before_player.cursor.clone();
        let Some(container) = active_window_container(before_player, transaction.window_id) else {
            return vec![TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventoryTransactionProcessed {
                    transaction,
                    accepted: false,
                },
            }];
        };
        let before_contents = window_contents(before_player, container);
        let before_properties = before_player
            .active_container
            .as_ref()
            .filter(|window| window.window_id == transaction.window_id)
            .map(OpenInventoryWindow::property_entries)
            .unwrap_or_default();
        let world_chest_position = before_player
            .active_container
            .as_ref()
            .filter(|window| window.window_id == transaction.window_id)
            .and_then(OpenInventoryWindow::world_chest_position);
        let resolved_slot =
            resolve_inventory_target(&target, transaction.window_id, container, session);

        let (applied, after_contents, after_properties, after_cursor, selected_hotbar_slot) = {
            let player = self
                .online_players
                .get_mut(&player_id)
                .expect("online player should still exist");
            let applied = match transaction.window_id {
                0 => match target {
                    InventoryClickTarget::Outside => {
                        apply_outside_click(&mut player.cursor, button)
                    }
                    _ => apply_player_window_click(player, resolved_slot, button),
                },
                _ => apply_active_container_click(
                    player,
                    transaction.window_id,
                    resolved_slot,
                    &target,
                    button,
                ),
            };
            let after_properties = player
                .active_container
                .as_ref()
                .filter(|window| window.window_id == transaction.window_id)
                .map(OpenInventoryWindow::property_entries)
                .unwrap_or_default();
            (
                applied,
                window_contents(player, container),
                after_properties,
                player.cursor.clone(),
                player.snapshot.selected_hotbar_slot,
            )
        };

        let snapshot = self
            .online_players
            .get(&player_id)
            .expect("online player should still exist")
            .snapshot
            .clone();
        self.saved_players.insert(player_id, snapshot);

        let clicked_slot_stack =
            resolved_slot.and_then(|slot| after_contents.get_slot(slot).cloned());
        let bedrock_authoritative =
            session.is_some_and(|session| session.protocol.contains(&ProtocolCapability::Bedrock));
        let accepted = if bedrock_authoritative {
            applied
        } else {
            applied && resolved_slot.is_some() && clicked_item == clicked_slot_stack.as_ref()
        };

        let mut events = vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::InventoryTransactionProcessed {
                transaction,
                accepted,
            },
        }];

        if !accepted {
            events.extend(Self::window_resync_events(
                player_id,
                transaction.window_id,
                container,
                &after_contents,
                selected_hotbar_slot,
                after_cursor.as_ref(),
                resolved_slot,
            ));
            events.extend(property_diff_events(
                transaction.window_id,
                player_id,
                &before_properties,
                &after_properties,
            ));
            return events;
        }

        events.extend(inventory_diff_events(
            transaction.window_id,
            container,
            player_id,
            &before_contents,
            &after_contents,
        ));
        events.extend(property_diff_events(
            transaction.window_id,
            player_id,
            &before_properties,
            &after_properties,
        ));
        if before_cursor != after_cursor {
            events.push(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::CursorChanged {
                    stack: after_cursor,
                },
            });
        }
        if container == InventoryContainer::Player
            && before_snapshot.selected_hotbar_slot != selected_hotbar_slot
        {
            events.push(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::SelectedHotbarSlotChanged {
                    slot: selected_hotbar_slot,
                },
            });
        }
        if let Some(position) = world_chest_position {
            events.extend(self.sync_world_chest_viewers(position, player_id));
        }
        events
    }
}

fn apply_player_window_click(
    player: &mut OnlinePlayer,
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
        return apply_player_crafting_result_click(
            &mut player.snapshot.inventory,
            &mut player.cursor,
            button,
        );
    }

    let Some(slot_stack) = player.snapshot.inventory.get_slot_mut(slot) else {
        return false;
    };
    let applied = match button {
        InventoryClickButton::Left => apply_left_click_slot_value(&mut player.cursor, slot_stack),
        InventoryClickButton::Right => apply_right_click_slot_value(&mut player.cursor, slot_stack),
    };
    if applied && slot.crafting_input_index().is_some() {
        recompute_player_crafting_result(&mut player.snapshot.inventory);
    }
    applied
}

fn apply_active_container_click(
    player: &mut OnlinePlayer,
    window_id: u8,
    slot: Option<InventorySlot>,
    target: &InventoryClickTarget,
    button: InventoryClickButton,
) -> bool {
    if matches!(target, InventoryClickTarget::Outside) {
        return apply_outside_click(&mut player.cursor, button);
    }
    let Some(slot) = slot else {
        return false;
    };
    let Some(window) = player.active_container.as_mut() else {
        return false;
    };
    if window.window_id != window_id {
        return false;
    }

    match window.container {
        InventoryContainer::CraftingTable => apply_crafting_table_click(
            window,
            &mut player.snapshot.inventory,
            &mut player.cursor,
            slot,
            button,
        ),
        InventoryContainer::Chest => apply_chest_click(
            window,
            &mut player.snapshot.inventory,
            &mut player.cursor,
            slot,
            button,
        ),
        InventoryContainer::Furnace => apply_furnace_click(
            window,
            &mut player.snapshot.inventory,
            &mut player.cursor,
            slot,
            button,
        ),
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
