use super::state::{OpenInventoryWindow, OpenInventoryWindowState};
use super::util::{MAX_STACK_SIZE, stack_keys_match};
use crate::events::InventoryClickButton;
use crate::inventory::{ItemStack, PlayerInventory};

#[derive(Clone, Debug, PartialEq, Eq)]
struct CraftingRecipe {
    output: ItemStack,
    consume: Vec<usize>,
}

pub(super) fn apply_player_crafting_result_click(
    inventory: &mut PlayerInventory,
    cursor: &mut Option<ItemStack>,
    button: InventoryClickButton,
) -> bool {
    let inputs = player_crafting_inputs(inventory);
    let Some(recipe) = current_crafting_recipe(&inputs, 2) else {
        recompute_player_crafting_result(inventory);
        return false;
    };
    if !take_recipe_output(cursor, &recipe.output, button) {
        return false;
    }
    consume_player_crafting_inputs(inventory, &recipe);
    recompute_player_crafting_result(inventory);
    true
}

pub(super) fn apply_active_container_crafting_result_click(
    window: &mut OpenInventoryWindow,
    cursor: &mut Option<ItemStack>,
    button: InventoryClickButton,
) -> bool {
    let inputs = active_container_crafting_inputs(window);
    let Some(recipe) = current_crafting_recipe(&inputs, 3) else {
        recompute_crafting_result_for_active_container(window);
        return false;
    };
    if !take_recipe_output(cursor, &recipe.output, button) {
        return false;
    }
    consume_active_container_inputs(window, &recipe);
    recompute_crafting_result_for_active_container(window);
    true
}

pub(super) fn recompute_player_crafting_result(inventory: &mut PlayerInventory) {
    let result =
        current_crafting_recipe(&player_crafting_inputs(inventory), 2).map(|recipe| recipe.output);
    let _ = inventory.set_crafting_result(result);
}

pub(super) fn recompute_crafting_result_for_active_container(window: &mut OpenInventoryWindow) {
    let OpenInventoryWindowState::CraftingTable { slots } = &mut window.state else {
        return;
    };
    if slots.is_empty() {
        return;
    }
    slots[0] = current_crafting_recipe(&slots.iter().skip(1).cloned().collect::<Vec<_>>(), 3)
        .map(|recipe| recipe.output);
}

fn take_recipe_output(
    cursor: &mut Option<ItemStack>,
    output: &ItemStack,
    button: InventoryClickButton,
) -> bool {
    let take_count = match button {
        InventoryClickButton::Left => output.count,
        InventoryClickButton::Right => 1,
    };
    if take_count == 0 {
        return false;
    }

    match cursor.as_mut() {
        None => {
            let mut taken = output.clone();
            taken.count = take_count;
            *cursor = Some(taken);
            true
        }
        Some(cursor_stack) if stack_keys_match(cursor_stack, output) => {
            let total = u16::from(cursor_stack.count) + u16::from(take_count);
            if total > u16::from(MAX_STACK_SIZE) {
                return false;
            }
            cursor_stack.count =
                u8::try_from(total).expect("crafted cursor stack should fit into u8");
            true
        }
        Some(_) => false,
    }
}

fn player_crafting_inputs(inventory: &PlayerInventory) -> Vec<Option<ItemStack>> {
    (0_u8..4)
        .map(|index| inventory.crafting_input(index).cloned())
        .collect()
}

fn active_container_crafting_inputs(window: &OpenInventoryWindow) -> Vec<Option<ItemStack>> {
    match &window.state {
        OpenInventoryWindowState::CraftingTable { slots } => {
            slots.iter().skip(1).cloned().collect()
        }
        OpenInventoryWindowState::Chest(_) | OpenInventoryWindowState::Furnace(_) => Vec::new(),
    }
}

fn current_crafting_recipe(inputs: &[Option<ItemStack>], width: usize) -> Option<CraftingRecipe> {
    if inputs.len() != width * width {
        return None;
    }

    let occupied = inputs
        .iter()
        .enumerate()
        .filter_map(|(index, stack)| stack.as_ref().map(|stack| (index, stack)))
        .collect::<Vec<_>>();
    if occupied.is_empty() {
        return None;
    }

    if occupied.len() == 1
        && occupied[0].1.key.as_str() == "minecraft:oak_log"
        && occupied[0].1.count > 0
    {
        return Some(CraftingRecipe {
            output: ItemStack::new("minecraft:oak_planks", 4, 0),
            consume: vec![occupied[0].0],
        });
    }

    if occupied.len() == 4
        && occupied
            .iter()
            .all(|(_, stack)| stack.key.as_str() == "minecraft:sand" && stack.count > 0)
        && forms_shifted_square(&occupied, width)
    {
        return Some(CraftingRecipe {
            output: ItemStack::new("minecraft:sandstone", 1, 0),
            consume: occupied.iter().map(|(index, _)| *index).collect(),
        });
    }

    if occupied.len() == 2
        && occupied
            .iter()
            .all(|(_, stack)| stack.key.as_str() == "minecraft:oak_planks" && stack.count > 0)
        && forms_vertical_pair(&occupied, width)
    {
        return Some(CraftingRecipe {
            output: ItemStack::new("minecraft:stick", 4, 0),
            consume: occupied.iter().map(|(index, _)| *index).collect(),
        });
    }

    None
}

fn forms_shifted_square(occupied: &[(usize, &ItemStack)], width: usize) -> bool {
    let xs = occupied
        .iter()
        .map(|(index, _)| index % width)
        .collect::<Vec<_>>();
    let ys = occupied
        .iter()
        .map(|(index, _)| index / width)
        .collect::<Vec<_>>();
    let min_x = *xs.iter().min().expect("occupied should not be empty");
    let max_x = *xs.iter().max().expect("occupied should not be empty");
    let min_y = *ys.iter().min().expect("occupied should not be empty");
    let max_y = *ys.iter().max().expect("occupied should not be empty");
    if max_x != min_x + 1 || max_y != min_y + 1 {
        return false;
    }
    let wanted = [
        (min_x, min_y),
        (min_x + 1, min_y),
        (min_x, min_y + 1),
        (min_x + 1, min_y + 1),
    ];
    occupied.iter().all(|(index, _)| {
        let coord = (index % width, index / width);
        wanted.contains(&coord)
    })
}

fn forms_vertical_pair(occupied: &[(usize, &ItemStack)], width: usize) -> bool {
    let first = occupied[0].0;
    let second = occupied[1].0;
    let (first_x, first_y) = (first % width, first / width);
    let (second_x, second_y) = (second % width, second / width);
    first_x == second_x && first_y.abs_diff(second_y) == 1
}

fn consume_player_crafting_inputs(inventory: &mut PlayerInventory, recipe: &CraftingRecipe) {
    for index in &recipe.consume {
        let Some(index) = u8::try_from(*index).ok() else {
            continue;
        };
        let Some(mut stack) = inventory.crafting_input(index).cloned() else {
            continue;
        };
        stack.count = stack.count.saturating_sub(1);
        if stack.count == 0 {
            let _ = inventory.set_crafting_input(index, None);
        } else {
            let _ = inventory.set_crafting_input(index, Some(stack));
        }
    }
}

fn consume_active_container_inputs(window: &mut OpenInventoryWindow, recipe: &CraftingRecipe) {
    let OpenInventoryWindowState::CraftingTable { slots } = &mut window.state else {
        return;
    };
    for index in &recipe.consume {
        let slot_index = index.saturating_add(1);
        let Some(slot) = slots.get_mut(slot_index) else {
            continue;
        };
        let Some(stack) = slot.as_mut() else {
            continue;
        };
        stack.count = stack.count.saturating_sub(1);
        if stack.count == 0 {
            *slot = None;
        }
    }
}
