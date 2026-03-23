use super::{OnlinePlayer, ServerCore};
use crate::PlayerId;
use crate::events::{
    CoreEvent, EventTarget, InventoryClickButton, InventoryClickTarget, TargetedEvent,
};
use crate::player::{
    InventoryContainer, InventorySlot, ItemStack, PlayerInventory, PlayerSnapshot,
};

const MAX_STACK_SIZE: u8 = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CraftingRecipe {
    output: ItemStack,
    consume: [u8; 4],
}

impl ServerCore {
    pub(super) fn apply_inventory_click(
        &mut self,
        player_id: PlayerId,
        target: InventoryClickTarget,
        button: InventoryClickButton,
    ) -> Vec<TargetedEvent> {
        let Some(before_player) = self.online_players.get(&player_id) else {
            return Vec::new();
        };
        let before_inventory = before_player.snapshot.inventory.clone();
        let before_cursor = before_player.cursor.clone();
        let selected_hotbar_slot = before_player.snapshot.selected_hotbar_slot;

        let (applied, after_inventory, after_cursor, snapshot) = {
            let player = self
                .online_players
                .get_mut(&player_id)
                .expect("online player should still exist");
            let applied = match target {
                InventoryClickTarget::Slot(slot) => apply_slot_click(player, slot, button),
                InventoryClickTarget::Outside | InventoryClickTarget::Unsupported => false,
            };
            (
                applied,
                player.snapshot.inventory.clone(),
                player.cursor.clone(),
                player.snapshot.clone(),
            )
        };

        self.saved_players.insert(player_id, snapshot);

        if !applied {
            return Self::window_zero_resync_events(
                player_id,
                &before_inventory,
                selected_hotbar_slot,
                before_cursor.as_ref(),
            );
        }

        let mut events = inventory_diff_events(player_id, &before_inventory, &after_inventory);
        if before_cursor != after_cursor {
            events.push(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::CursorChanged {
                    stack: after_cursor,
                },
            });
        }
        events
    }

    pub(super) fn persisted_online_player_snapshot(player: &OnlinePlayer) -> PlayerSnapshot {
        persist_window_zero_state(&player.snapshot, player.cursor.as_ref())
    }

    pub(super) fn recompute_crafting_result_for_inventory(inventory: &mut PlayerInventory) {
        recompute_crafting_result(inventory);
    }

    fn window_zero_resync_events(
        player_id: PlayerId,
        inventory: &PlayerInventory,
        selected_hotbar_slot: u8,
        cursor: Option<&ItemStack>,
    ) -> Vec<TargetedEvent> {
        vec![
            TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventoryContents {
                    container: InventoryContainer::Player,
                    inventory: inventory.clone(),
                },
            },
            TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::SelectedHotbarSlotChanged {
                    slot: selected_hotbar_slot,
                },
            },
            TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::CursorChanged {
                    stack: cursor.cloned(),
                },
            },
        ]
    }
}

fn apply_slot_click(
    player: &mut OnlinePlayer,
    slot: InventorySlot,
    button: InventoryClickButton,
) -> bool {
    if slot.is_reserved_auxiliary() {
        return false;
    }

    if slot.is_crafting_result() {
        return apply_crafting_result_click(
            &mut player.snapshot.inventory,
            &mut player.cursor,
            button,
        );
    }

    let is_crafting_input = slot.crafting_input_index().is_some();
    let applied = match button {
        InventoryClickButton::Left => {
            apply_left_click(&mut player.snapshot.inventory, &mut player.cursor, slot)
        }
        InventoryClickButton::Right => {
            apply_right_click(&mut player.snapshot.inventory, &mut player.cursor, slot)
        }
    };
    if applied && is_crafting_input {
        recompute_crafting_result(&mut player.snapshot.inventory);
    }
    applied
}

fn apply_left_click(
    inventory: &mut PlayerInventory,
    cursor: &mut Option<ItemStack>,
    slot: InventorySlot,
) -> bool {
    let slot_stack = inventory.get_slot(slot).cloned();
    match (cursor.as_ref(), slot_stack.as_ref()) {
        (None, None) => true,
        (None, Some(_)) => {
            *cursor = slot_stack;
            let _ = inventory.set_slot(slot, None);
            true
        }
        (Some(_), None) => {
            let _ = inventory.set_slot(slot, cursor.take());
            true
        }
        (Some(cursor_stack), Some(slot_stack)) if stack_keys_match(cursor_stack, slot_stack) => {
            let total = u16::from(cursor_stack.count) + u16::from(slot_stack.count);
            let placed = total.min(u16::from(MAX_STACK_SIZE));
            let remainder = total.saturating_sub(placed);
            let mut next_slot = slot_stack.clone();
            next_slot.count = u8::try_from(placed).expect("placed stack count should fit into u8");
            let _ = inventory.set_slot(slot, Some(next_slot));
            if remainder == 0 {
                *cursor = None;
            } else if let Some(cursor_stack) = cursor.as_mut() {
                cursor_stack.count =
                    u8::try_from(remainder).expect("cursor remainder should fit into u8");
            }
            true
        }
        (Some(_), Some(_)) => {
            let displaced = slot_stack;
            let _ = inventory.set_slot(slot, cursor.take());
            *cursor = displaced;
            true
        }
    }
}

fn apply_right_click(
    inventory: &mut PlayerInventory,
    cursor: &mut Option<ItemStack>,
    slot: InventorySlot,
) -> bool {
    let slot_stack = inventory.get_slot(slot).cloned();
    match (cursor.as_ref(), slot_stack.as_ref()) {
        (None, None) => true,
        (None, Some(slot_stack)) => {
            let cursor_count = (slot_stack.count.saturating_add(1)) / 2;
            let slot_count = slot_stack.count / 2;
            let mut next_cursor = slot_stack.clone();
            next_cursor.count = cursor_count;
            *cursor = Some(next_cursor);
            if slot_count == 0 {
                let _ = inventory.set_slot(slot, None);
            } else {
                let mut next_slot = slot_stack.clone();
                next_slot.count = slot_count;
                let _ = inventory.set_slot(slot, Some(next_slot));
            }
            true
        }
        (Some(cursor_stack), None) => {
            let mut placed = cursor_stack.clone();
            placed.count = 1;
            let _ = inventory.set_slot(slot, Some(placed));
            decrement_cursor(cursor);
            true
        }
        (Some(cursor_stack), Some(slot_stack))
            if stack_keys_match(cursor_stack, slot_stack) && slot_stack.count < MAX_STACK_SIZE =>
        {
            let mut next_slot = slot_stack.clone();
            next_slot.count = next_slot.count.saturating_add(1);
            let _ = inventory.set_slot(slot, Some(next_slot));
            decrement_cursor(cursor);
            true
        }
        _ => false,
    }
}

fn apply_crafting_result_click(
    inventory: &mut PlayerInventory,
    cursor: &mut Option<ItemStack>,
    button: InventoryClickButton,
) -> bool {
    let Some(recipe) = current_crafting_recipe(inventory) else {
        recompute_crafting_result(inventory);
        return false;
    };

    let take_count = match button {
        InventoryClickButton::Left => recipe.output.count,
        InventoryClickButton::Right => 1,
    };
    if take_count == 0 {
        return false;
    }

    match cursor.as_mut() {
        None => {
            let mut taken = recipe.output.clone();
            taken.count = take_count;
            *cursor = Some(taken);
        }
        Some(cursor_stack) if stack_keys_match(cursor_stack, &recipe.output) => {
            let total = u16::from(cursor_stack.count) + u16::from(take_count);
            if total > u16::from(MAX_STACK_SIZE) {
                return false;
            }
            cursor_stack.count =
                u8::try_from(total).expect("crafted cursor stack should fit into u8");
        }
        Some(_) => return false,
    }

    consume_crafting_inputs(inventory, &recipe);
    recompute_crafting_result(inventory);
    true
}

fn current_crafting_recipe(inventory: &PlayerInventory) -> Option<CraftingRecipe> {
    let inputs = [
        inventory.crafting_input(0).cloned(),
        inventory.crafting_input(1).cloned(),
        inventory.crafting_input(2).cloned(),
        inventory.crafting_input(3).cloned(),
    ];

    let oak_log_slots = occupied_recipe_slots(&inputs, "minecraft:oak_log");
    if oak_log_slots.len() == 1 {
        let mut consume = [0; 4];
        consume[oak_log_slots[0]] = 1;
        return Some(CraftingRecipe {
            output: ItemStack::new("minecraft:oak_planks", 4, 0),
            consume,
        });
    }

    let sand_slots = occupied_recipe_slots(&inputs, "minecraft:sand");
    if sand_slots == [0, 1, 2, 3] {
        return Some(CraftingRecipe {
            output: ItemStack::new("minecraft:sandstone", 1, 0),
            consume: [1, 1, 1, 1],
        });
    }

    let plank_slots = occupied_recipe_slots(&inputs, "minecraft:oak_planks");
    if plank_slots == [0, 2] || plank_slots == [1, 3] {
        let mut consume = [0; 4];
        for index in plank_slots {
            consume[index] = 1;
        }
        return Some(CraftingRecipe {
            output: ItemStack::new("minecraft:stick", 4, 0),
            consume,
        });
    }

    None
}

fn occupied_recipe_slots(inputs: &[Option<ItemStack>; 4], key: &str) -> Vec<usize> {
    let mut occupied = Vec::new();
    for (index, stack) in inputs.iter().enumerate() {
        let Some(stack) = stack else {
            continue;
        };
        if stack.key.as_str() != key || stack.count == 0 {
            return Vec::new();
        }
        occupied.push(index);
    }
    if occupied.is_empty() {
        return Vec::new();
    }
    let expected = occupied.len();
    if inputs.iter().filter(|stack| stack.is_some()).count() != expected {
        return Vec::new();
    }
    occupied
}

fn consume_crafting_inputs(inventory: &mut PlayerInventory, recipe: &CraftingRecipe) {
    for (index, amount) in recipe.consume.into_iter().enumerate() {
        if amount == 0 {
            continue;
        }
        let Some(mut stack) = inventory.crafting_input(index as u8).cloned() else {
            continue;
        };
        stack.count = stack.count.saturating_sub(amount);
        if stack.count == 0 {
            let _ = inventory.set_crafting_input(index as u8, None);
        } else {
            let _ = inventory.set_crafting_input(index as u8, Some(stack));
        }
    }
}

fn recompute_crafting_result(inventory: &mut PlayerInventory) {
    let result = current_crafting_recipe(inventory).map(|recipe| recipe.output);
    let _ = inventory.set_crafting_result(result);
}

fn inventory_diff_events(
    player_id: PlayerId,
    before: &PlayerInventory,
    after: &PlayerInventory,
) -> Vec<TargetedEvent> {
    let mut events = Vec::new();
    for raw_slot in 0_u8..45 {
        let Some(slot) = InventorySlot::from_legacy_window_index(raw_slot) else {
            continue;
        };
        let before_stack = before.get_slot(slot).cloned();
        let after_stack = after.get_slot(slot).cloned();
        if before_stack == after_stack {
            continue;
        }
        events.push(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::InventorySlotChanged {
                container: InventoryContainer::Player,
                slot,
                stack: after_stack,
            },
        });
    }
    if before.offhand != after.offhand {
        events.push(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::InventorySlotChanged {
                container: InventoryContainer::Player,
                slot: InventorySlot::Offhand,
                stack: after.offhand.clone(),
            },
        });
    }
    events
}

fn persist_window_zero_state(
    player: &PlayerSnapshot,
    cursor: Option<&ItemStack>,
) -> PlayerSnapshot {
    let mut persisted = player.clone();
    let transient_slots = [
        InventorySlot::crafting_result(),
        InventorySlot::crafting_input(0).expect("craft slot should exist"),
        InventorySlot::crafting_input(1).expect("craft slot should exist"),
        InventorySlot::crafting_input(2).expect("craft slot should exist"),
        InventorySlot::crafting_input(3).expect("craft slot should exist"),
        InventorySlot::Auxiliary(5),
        InventorySlot::Auxiliary(6),
        InventorySlot::Auxiliary(7),
        InventorySlot::Auxiliary(8),
    ];
    let mut overflow = transient_slots
        .into_iter()
        .filter_map(|slot| persisted.inventory.get_slot(slot).cloned())
        .collect::<Vec<_>>();
    if let Some(cursor) = cursor.cloned() {
        overflow.push(cursor);
    }
    for slot in transient_slots {
        let _ = persisted.inventory.set_slot(slot, None);
    }
    for stack in overflow {
        merge_into_persistent_inventory(&mut persisted.inventory, stack);
    }
    recompute_crafting_result(&mut persisted.inventory);
    persisted
}

fn merge_into_persistent_inventory(inventory: &mut PlayerInventory, mut stack: ItemStack) {
    for slot in persistent_slot_order() {
        let Some(existing) = inventory.get_slot(slot).cloned() else {
            continue;
        };
        if !stack_keys_match(&existing, &stack) || existing.count >= MAX_STACK_SIZE {
            continue;
        }
        let available = MAX_STACK_SIZE.saturating_sub(existing.count);
        let moved = available.min(stack.count);
        let mut next = existing;
        next.count = next.count.saturating_add(moved);
        let _ = inventory.set_slot(slot, Some(next));
        stack.count = stack.count.saturating_sub(moved);
        if stack.count == 0 {
            return;
        }
    }

    for slot in persistent_slot_order() {
        if inventory.get_slot(slot).is_some() {
            continue;
        }
        let _ = inventory.set_slot(slot, Some(stack));
        return;
    }
}

fn persistent_slot_order() -> impl Iterator<Item = InventorySlot> {
    (0_u8..27)
        .map(InventorySlot::MainInventory)
        .chain((0_u8..9).map(InventorySlot::Hotbar))
        .chain(std::iter::once(InventorySlot::Offhand))
}

fn decrement_cursor(cursor: &mut Option<ItemStack>) {
    let Some(stack) = cursor.as_mut() else {
        return;
    };
    stack.count = stack.count.saturating_sub(1);
    if stack.count == 0 {
        *cursor = None;
    }
}

fn stack_keys_match(left: &ItemStack, right: &ItemStack) -> bool {
    left.key == right.key && left.damage == right.damage
}
