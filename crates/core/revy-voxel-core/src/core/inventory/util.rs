use crate::inventory::{InventorySlot, ItemStack, PlayerInventory};

pub(crate) const MAX_STACK_SIZE: u8 = 64;

pub(super) fn decrement_cursor(cursor: &mut Option<ItemStack>) {
    let Some(stack) = cursor.as_mut() else {
        return;
    };
    stack.count = stack.count.saturating_sub(1);
    if stack.count == 0 {
        *cursor = None;
    }
}

pub(crate) fn stack_keys_match(left: &ItemStack, right: &ItemStack) -> bool {
    left.key == right.key
        && left.damage == right.damage
        && left.components == right.components
        && left.extra == right.extra
}

pub(crate) fn merge_stack_into_player_inventory(
    inventory: &mut PlayerInventory,
    mut stack: ItemStack,
) -> Option<ItemStack> {
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
            return None;
        }
    }

    for slot in persistent_slot_order() {
        if inventory.get_slot(slot).is_some() {
            continue;
        }
        let _ = inventory.set_slot(slot, Some(stack));
        return None;
    }

    Some(stack)
}

fn persistent_slot_order() -> impl Iterator<Item = InventorySlot> {
    (0_u8..9)
        .map(InventorySlot::Hotbar)
        .chain((0_u8..27).map(InventorySlot::MainInventory))
        .chain(std::iter::once(InventorySlot::Offhand))
}

#[cfg(test)]
mod tests {
    use super::*;
    use revy_voxel_semantic::{ItemDataMap, ItemDataValue};

    fn component_stack(name: &str) -> ItemStack {
        let mut stack = ItemStack::new("minecraft:stone", 4, 0);
        let mut components = ItemDataMap::new();
        let _ = components.insert(
            "minecraft:custom_name",
            ItemDataValue::String(name.to_string()),
        );
        stack.components = components;
        stack
    }

    #[test]
    fn stack_keys_match_requires_matching_components() {
        assert!(!stack_keys_match(
            &component_stack("left"),
            &component_stack("right"),
        ));
    }

    #[test]
    fn merge_keeps_component_variants_separate() {
        let mut inventory = PlayerInventory::new_empty();
        let _ = inventory.set_slot(InventorySlot::Hotbar(0), Some(component_stack("left")));

        let remainder = merge_stack_into_player_inventory(&mut inventory, component_stack("right"));

        assert!(remainder.is_none());
        assert_eq!(
            inventory.get_slot(InventorySlot::Hotbar(0)),
            Some(&component_stack("left")),
        );
        assert_eq!(
            inventory.get_slot(InventorySlot::Hotbar(1)),
            Some(&component_stack("right")),
        );
    }
}
