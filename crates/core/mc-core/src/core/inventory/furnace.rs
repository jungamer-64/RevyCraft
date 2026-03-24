use super::state::{
    FURNACE_COOK_TOTAL, FurnaceWindowState, OpenInventoryWindow, OpenInventoryWindowState,
};
use super::util::{MAX_STACK_SIZE, consume_single_item, stack_keys_match};
use crate::inventory::ItemStack;

pub(super) fn normalize_furnace_window(window: &mut OpenInventoryWindow) {
    let OpenInventoryWindowState::Furnace(furnace) = &mut window.state else {
        return;
    };
    furnace.cook_total = FURNACE_COOK_TOTAL;
    if furnace.burn_left == 0 {
        furnace.burn_max = 0;
    }
    if furnace_available_output(furnace).is_none() {
        furnace.cook_progress = 0;
    }
}

pub(super) fn tick_furnace_window(window: &mut OpenInventoryWindow) {
    let OpenInventoryWindowState::Furnace(furnace) = &mut window.state else {
        return;
    };
    furnace.cook_total = FURNACE_COOK_TOTAL;
    if furnace.burn_left > 0 {
        furnace.burn_left = furnace.burn_left.saturating_sub(1);
        if furnace.burn_left == 0 {
            furnace.burn_max = 0;
        }
    }

    let available_output = furnace_available_output(furnace);
    if furnace.burn_left == 0
        && available_output.is_some()
        && let Some(burn_time) = furnace_fuel_burn_time(furnace.fuel.as_ref())
    {
        furnace.burn_left = burn_time;
        furnace.burn_max = burn_time;
        consume_single_item(&mut furnace.fuel);
    }

    if furnace.burn_left > 0 {
        if let Some(output) = furnace_available_output(furnace) {
            furnace.cook_progress = furnace.cook_progress.saturating_add(1);
            if furnace.cook_progress >= furnace.cook_total {
                smelt_furnace_once(furnace, &output);
                furnace.cook_progress = 0;
            }
        } else {
            furnace.cook_progress = 0;
        }
    } else {
        furnace.cook_progress = 0;
    }
}

fn furnace_available_output(furnace: &FurnaceWindowState) -> Option<ItemStack> {
    let input = furnace.input.as_ref()?;
    let output = furnace_recipe_output(input)?;
    match furnace.output.as_ref() {
        None => Some(output),
        Some(existing)
            if stack_keys_match(existing, &output) && existing.count < MAX_STACK_SIZE =>
        {
            Some(output)
        }
        _ => None,
    }
}

fn furnace_recipe_output(input: &ItemStack) -> Option<ItemStack> {
    match input.key.as_str() {
        "minecraft:sand" if input.count > 0 => Some(ItemStack::new("minecraft:glass", 1, 0)),
        "minecraft:cobblestone" if input.count > 0 => Some(ItemStack::new("minecraft:stone", 1, 0)),
        _ => None,
    }
}

fn furnace_fuel_burn_time(fuel: Option<&ItemStack>) -> Option<i16> {
    let fuel = fuel?;
    if fuel.count == 0 {
        return None;
    }
    match fuel.key.as_str() {
        "minecraft:stick" => Some(100),
        "minecraft:oak_planks" | "minecraft:oak_log" => Some(300),
        _ => None,
    }
}

fn smelt_furnace_once(furnace: &mut FurnaceWindowState, output: &ItemStack) {
    consume_single_item(&mut furnace.input);
    match furnace.output.as_mut() {
        Some(existing) if stack_keys_match(existing, output) => {
            existing.count = existing.count.saturating_add(output.count);
        }
        Some(_) => {}
        None => furnace.output = Some(output.clone()),
    }
}
