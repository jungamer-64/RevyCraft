use crate::inventory::ItemStack;

pub(super) const MAX_STACK_SIZE: u8 = 64;

pub(super) fn consume_single_item(slot: &mut Option<ItemStack>) {
    let Some(stack) = slot.as_mut() else {
        return;
    };
    stack.count = stack.count.saturating_sub(1);
    if stack.count == 0 {
        *slot = None;
    }
}

pub(super) fn decrement_cursor(cursor: &mut Option<ItemStack>) {
    let Some(stack) = cursor.as_mut() else {
        return;
    };
    stack.count = stack.count.saturating_sub(1);
    if stack.count == 0 {
        *cursor = None;
    }
}

pub(super) fn reduce_slot_stack(slot: &mut Option<ItemStack>, amount: u8) {
    let Some(stack) = slot.as_mut() else {
        return;
    };
    stack.count = stack.count.saturating_sub(amount);
    if stack.count == 0 {
        *slot = None;
    }
}

pub(super) fn stack_keys_match(left: &ItemStack, right: &ItemStack) -> bool {
    left.key == right.key && left.damage == right.damage
}
