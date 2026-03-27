use super::state::{OpenInventoryWindow, OpenInventoryWindowState};
use super::util::{MAX_STACK_SIZE, stack_keys_match};
use crate::catalog;
use crate::events::InventoryClickButton;
use crate::inventory::{ItemStack, PlayerInventory};

#[derive(Clone, Debug, PartialEq, Eq)]
struct CraftingRecipe {
    output: ItemStack,
    consume: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RecipeDefinition {
    min_grid_width: usize,
    pattern_width: usize,
    pattern_height: usize,
    pattern: &'static [Option<&'static str>],
    output_key: &'static str,
    output_count: u8,
}

#[derive(Debug)]
struct NormalizedCraftingGrid<'a> {
    available_width: usize,
    width: usize,
    height: usize,
    cells: Vec<Option<&'a ItemStack>>,
    original_indices: Vec<Option<usize>>,
}

const LOG_TO_PLANKS_PATTERN: [Option<&str>; 1] = [Some(catalog::OAK_LOG)];
const STICK_PATTERN: [Option<&str>; 2] = [Some(catalog::OAK_PLANKS), Some(catalog::OAK_PLANKS)];
const SANDSTONE_PATTERN: [Option<&str>; 4] = [
    Some(catalog::SAND),
    Some(catalog::SAND),
    Some(catalog::SAND),
    Some(catalog::SAND),
];
const CRAFTING_TABLE_PATTERN: [Option<&str>; 4] = [
    Some(catalog::OAK_PLANKS),
    Some(catalog::OAK_PLANKS),
    Some(catalog::OAK_PLANKS),
    Some(catalog::OAK_PLANKS),
];
const CHEST_PATTERN: [Option<&str>; 9] = [
    Some(catalog::OAK_PLANKS),
    Some(catalog::OAK_PLANKS),
    Some(catalog::OAK_PLANKS),
    Some(catalog::OAK_PLANKS),
    None,
    Some(catalog::OAK_PLANKS),
    Some(catalog::OAK_PLANKS),
    Some(catalog::OAK_PLANKS),
    Some(catalog::OAK_PLANKS),
];
const FURNACE_PATTERN: [Option<&str>; 9] = [
    Some(catalog::COBBLESTONE),
    Some(catalog::COBBLESTONE),
    Some(catalog::COBBLESTONE),
    Some(catalog::COBBLESTONE),
    None,
    Some(catalog::COBBLESTONE),
    Some(catalog::COBBLESTONE),
    Some(catalog::COBBLESTONE),
    Some(catalog::COBBLESTONE),
];
const RECIPE_DEFINITIONS: [RecipeDefinition; 6] = [
    RecipeDefinition {
        min_grid_width: 1,
        pattern_width: 1,
        pattern_height: 1,
        pattern: &LOG_TO_PLANKS_PATTERN,
        output_key: catalog::OAK_PLANKS,
        output_count: 4,
    },
    RecipeDefinition {
        min_grid_width: 2,
        pattern_width: 1,
        pattern_height: 2,
        pattern: &STICK_PATTERN,
        output_key: catalog::STICK,
        output_count: 4,
    },
    RecipeDefinition {
        min_grid_width: 2,
        pattern_width: 2,
        pattern_height: 2,
        pattern: &SANDSTONE_PATTERN,
        output_key: catalog::SANDSTONE,
        output_count: 1,
    },
    RecipeDefinition {
        min_grid_width: 2,
        pattern_width: 2,
        pattern_height: 2,
        pattern: &CRAFTING_TABLE_PATTERN,
        output_key: catalog::CRAFTING_TABLE,
        output_count: 1,
    },
    RecipeDefinition {
        min_grid_width: 3,
        pattern_width: 3,
        pattern_height: 3,
        pattern: &CHEST_PATTERN,
        output_key: catalog::CHEST,
        output_count: 1,
    },
    RecipeDefinition {
        min_grid_width: 3,
        pattern_width: 3,
        pattern_height: 3,
        pattern: &FURNACE_PATTERN,
        output_key: catalog::FURNACE,
        output_count: 1,
    },
];

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
    let normalized = normalize_crafting_inputs(inputs, width)?;
    RECIPE_DEFINITIONS
        .iter()
        .find_map(|definition| match_recipe(definition, &normalized))
}

fn normalize_crafting_inputs(
    inputs: &[Option<ItemStack>],
    width: usize,
) -> Option<NormalizedCraftingGrid<'_>> {
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

    let min_x = occupied
        .iter()
        .map(|(index, _)| index % width)
        .min()
        .expect("occupied crafting inputs should not be empty");
    let max_x = occupied
        .iter()
        .map(|(index, _)| index % width)
        .max()
        .expect("occupied crafting inputs should not be empty");
    let min_y = occupied
        .iter()
        .map(|(index, _)| index / width)
        .min()
        .expect("occupied crafting inputs should not be empty");
    let max_y = occupied
        .iter()
        .map(|(index, _)| index / width)
        .max()
        .expect("occupied crafting inputs should not be empty");
    let normalized_width = max_x - min_x + 1;
    let normalized_height = max_y - min_y + 1;
    let mut cells = vec![None; normalized_width * normalized_height];
    let mut original_indices = vec![None; normalized_width * normalized_height];

    for (index, stack) in occupied {
        let x = index % width;
        let y = index / width;
        let normalized_index = (y - min_y) * normalized_width + (x - min_x);
        cells[normalized_index] = Some(stack);
        original_indices[normalized_index] = Some(index);
    }

    Some(NormalizedCraftingGrid {
        available_width: width,
        width: normalized_width,
        height: normalized_height,
        cells,
        original_indices,
    })
}

fn match_recipe(
    definition: &RecipeDefinition,
    normalized: &NormalizedCraftingGrid<'_>,
) -> Option<CraftingRecipe> {
    if normalized.available_width < definition.min_grid_width
        || normalized.width != definition.pattern_width
        || normalized.height != definition.pattern_height
        || normalized.cells.len() != definition.pattern.len()
    {
        return None;
    }

    for (expected, actual) in definition.pattern.iter().zip(&normalized.cells) {
        match (expected, actual) {
            (Some(expected_key), Some(stack))
                if stack.count > 0 && stack.key.as_str() == *expected_key => {}
            (None, None) => {}
            _ => return None,
        }
    }

    let consume = definition
        .pattern
        .iter()
        .zip(&normalized.original_indices)
        .filter_map(|(expected, index)| expected.is_some().then_some(*index).flatten())
        .collect();

    Some(CraftingRecipe {
        output: ItemStack::new(definition.output_key, definition.output_count, 0),
        consume,
    })
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
