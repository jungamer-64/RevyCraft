use crate::catalog;
use crate::ids;
use mc_content_api::{
    BlockEntityKindId, ContainerKindId, ContainerPropertyKey, ContainerSlotRole, ContainerSpec,
    MiningToolSpec,
};
use mc_core::{
    BlockState, ChunkColumn, ChunkPos, ContainerBlockEntityState, ContentBehavior,
    InventoryClickButton, InventorySlot, ItemStack, OpenContainerState, PlayerInventory, WorldMeta,
};
use std::collections::BTreeMap;
use std::sync::Arc;

const PLAYER_LOCAL_SLOT_COUNT: u16 = 9;
const FURNACE_COOK_TOTAL: i16 = 200;

#[derive(Clone, Debug, Default)]
pub struct CanonicalContent;

#[must_use]
pub fn canonical_content() -> Arc<dyn ContentBehavior> {
    Arc::new(CanonicalContent)
}

#[must_use]
pub fn creative_starter_inventory() -> PlayerInventory {
    let mut inventory = PlayerInventory::new_empty();
    for (slot, key) in (36_u8..45).zip([
        catalog::STONE,
        catalog::DIRT,
        catalog::GRASS_BLOCK,
        catalog::COBBLESTONE,
        catalog::OAK_PLANKS,
        catalog::SAND,
        catalog::SANDSTONE,
        catalog::GLASS,
        catalog::BRICKS,
    ]) {
        let _ = inventory.set(slot, Some(ItemStack::new(key, 64, 0)));
    }
    inventory
}

#[must_use]
pub fn item_supported_for_inventory(key: &str) -> bool {
    matches!(
        key,
        catalog::STONE
            | catalog::DIRT
            | catalog::GRASS_BLOCK
            | catalog::COBBLESTONE
            | catalog::OAK_PLANKS
            | catalog::SAND
            | catalog::SANDSTONE
            | catalog::GLASS
            | catalog::BRICKS
            | catalog::OAK_LOG
            | catalog::STICK
            | catalog::CRAFTING_TABLE
            | catalog::CHEST
            | catalog::FURNACE
    )
}

#[must_use]
pub fn placeable_block_state_from_item_key(key: &str) -> Option<BlockState> {
    Some(BlockState::new(match key {
        catalog::STONE => catalog::STONE,
        catalog::DIRT => catalog::DIRT,
        catalog::GRASS_BLOCK => catalog::GRASS_BLOCK,
        catalog::COBBLESTONE => catalog::COBBLESTONE,
        catalog::OAK_PLANKS => catalog::OAK_PLANKS,
        catalog::SAND => catalog::SAND,
        catalog::SANDSTONE => catalog::SANDSTONE,
        catalog::GLASS => catalog::GLASS,
        catalog::BRICKS => catalog::BRICKS,
        catalog::CRAFTING_TABLE => catalog::CRAFTING_TABLE,
        catalog::CHEST => catalog::CHEST,
        catalog::FURNACE => catalog::FURNACE,
        _ => return None,
    }))
}

#[must_use]
pub fn default_chunk(chunk_pos: ChunkPos) -> ChunkColumn {
    let mut column = ChunkColumn::new(chunk_pos);
    for z in 0_u8..16 {
        for x in 0_u8..16 {
            column.set_block(x, 0, z, Some(BlockState::new(catalog::BEDROCK)));
            column.set_block(x, 1, z, Some(BlockState::new(catalog::STONE)));
            column.set_block(x, 2, z, Some(BlockState::new(catalog::DIRT)));
            column.set_block(x, 3, z, Some(BlockState::new(catalog::GRASS_BLOCK)));
        }
    }
    column
}

#[must_use]
pub fn default_block_entity_for_kind(
    kind: &BlockEntityKindId,
) -> Option<ContainerBlockEntityState> {
    match kind.as_str() {
        ids::CHEST_BLOCK_ENTITY => Some(ContainerBlockEntityState {
            kind: kind.clone(),
            slots: vec![None; 27],
            properties: BTreeMap::new(),
        }),
        ids::FURNACE_BLOCK_ENTITY => Some(ContainerBlockEntityState {
            kind: kind.clone(),
            slots: vec![None; 3],
            properties: furnace_default_properties(),
        }),
        _ => None,
    }
}

#[must_use]
pub fn default_block_entity_for_block(block: &BlockState) -> Option<ContainerBlockEntityState> {
    match block.key.as_str() {
        catalog::CHEST => {
            default_block_entity_for_kind(&BlockEntityKindId::new(ids::CHEST_BLOCK_ENTITY))
        }
        catalog::FURNACE => {
            default_block_entity_for_kind(&BlockEntityKindId::new(ids::FURNACE_BLOCK_ENTITY))
        }
        _ => None,
    }
}

fn furnace_default_properties() -> BTreeMap<ContainerPropertyKey, i16> {
    BTreeMap::from([
        (ContainerPropertyKey::new(ids::FURNACE_BURN_LEFT), 0),
        (ContainerPropertyKey::new(ids::FURNACE_BURN_MAX), 0),
        (ContainerPropertyKey::new(ids::FURNACE_COOK_PROGRESS), 0),
        (
            ContainerPropertyKey::new(ids::FURNACE_COOK_TOTAL),
            FURNACE_COOK_TOTAL,
        ),
    ])
}

fn container_spec(kind: &ContainerKindId) -> Option<ContainerSpec> {
    Some(match kind.as_str() {
        ids::PLAYER => ContainerSpec {
            local_slot_count: PLAYER_LOCAL_SLOT_COUNT,
            slot_roles: BTreeMap::from([
                (0, ContainerSlotRole::OutputOnly),
                (5, ContainerSlotRole::Unavailable),
                (6, ContainerSlotRole::Unavailable),
                (7, ContainerSlotRole::Unavailable),
                (8, ContainerSlotRole::Unavailable),
            ]),
        },
        ids::CRAFTING_TABLE => ContainerSpec {
            local_slot_count: 10,
            slot_roles: BTreeMap::from([(0, ContainerSlotRole::OutputOnly)]),
        },
        ids::CHEST_27 => ContainerSpec {
            local_slot_count: 27,
            slot_roles: BTreeMap::new(),
        },
        ids::FURNACE => ContainerSpec {
            local_slot_count: 3,
            slot_roles: BTreeMap::from([(2, ContainerSlotRole::OutputOnly)]),
        },
        _ => return None,
    })
}

impl ContentBehavior for CanonicalContent {
    fn generate_chunk(&self, _meta: &WorldMeta, chunk_pos: ChunkPos) -> ChunkColumn {
        default_chunk(chunk_pos)
    }

    fn player_container_kind(&self) -> ContainerKindId {
        ContainerKindId::new(ids::PLAYER)
    }

    fn container_spec(&self, kind: &ContainerKindId) -> Option<ContainerSpec> {
        container_spec(kind)
    }

    fn container_title(&self, kind: &ContainerKindId) -> String {
        match kind.as_str() {
            ids::PLAYER => "Player".to_string(),
            ids::CRAFTING_TABLE => "Crafting".to_string(),
            ids::CHEST_27 => "Chest".to_string(),
            ids::FURNACE => "Furnace".to_string(),
            _ => kind.to_string(),
        }
    }

    fn container_kind_for_block(&self, block: &BlockState) -> Option<ContainerKindId> {
        match block.key.as_str() {
            catalog::CRAFTING_TABLE => Some(ContainerKindId::new(ids::CRAFTING_TABLE)),
            catalog::CHEST => Some(ContainerKindId::new(ids::CHEST_27)),
            catalog::FURNACE => Some(ContainerKindId::new(ids::FURNACE)),
            _ => None,
        }
    }

    fn default_block_entity_for_block(
        &self,
        block: &BlockState,
    ) -> Option<ContainerBlockEntityState> {
        default_block_entity_for_block(block)
    }

    fn default_block_entity_for_kind(
        &self,
        kind: &BlockEntityKindId,
    ) -> Option<ContainerBlockEntityState> {
        default_block_entity_for_kind(kind)
    }

    fn block_entity_kind_for_container(&self, kind: &ContainerKindId) -> Option<BlockEntityKindId> {
        match kind.as_str() {
            ids::CHEST_27 => Some(BlockEntityKindId::new(ids::CHEST_BLOCK_ENTITY)),
            ids::FURNACE => Some(BlockEntityKindId::new(ids::FURNACE_BLOCK_ENTITY)),
            _ => None,
        }
    }

    fn container_kind_for_block_entity(
        &self,
        entity: &ContainerBlockEntityState,
    ) -> Option<ContainerKindId> {
        match entity.kind.as_str() {
            ids::CHEST_BLOCK_ENTITY => Some(ContainerKindId::new(ids::CHEST_27)),
            ids::FURNACE_BLOCK_ENTITY => Some(ContainerKindId::new(ids::FURNACE)),
            _ => None,
        }
    }

    fn is_air_block(&self, block: &BlockState) -> bool {
        block.key.as_str() == catalog::AIR
    }

    fn is_unbreakable_block(&self, block: &BlockState) -> bool {
        block.key.as_str() == catalog::BEDROCK
    }

    fn placeable_block_state_from_item_key(&self, key: &str) -> Option<BlockState> {
        placeable_block_state_from_item_key(key)
    }

    fn is_supported_inventory_item(&self, key: &str) -> bool {
        item_supported_for_inventory(key)
    }

    fn starter_inventory(&self) -> PlayerInventory {
        creative_starter_inventory()
    }

    fn tool_spec_for_item(&self, _item: Option<&ItemStack>) -> Option<MiningToolSpec> {
        None
    }

    fn survival_mining_duration_ms(
        &self,
        block: &BlockState,
        _tool: Option<MiningToolSpec>,
    ) -> Option<u64> {
        let hardness = match block.key.as_str() {
            catalog::STONE => 1.5,
            catalog::GRASS_BLOCK => 0.6,
            catalog::DIRT => 0.5,
            catalog::COBBLESTONE => 2.0,
            catalog::OAK_PLANKS => 2.0,
            catalog::SAND => 0.5,
            catalog::SANDSTONE => 0.8,
            catalog::GLASS => 0.3,
            catalog::BRICKS => 2.0,
            catalog::CRAFTING_TABLE => 2.5,
            catalog::CHEST => 2.5,
            catalog::FURNACE => 3.5,
            _ => return None,
        };
        let ticks = (hardness * 30.0_f32).ceil() as u64;
        Some(ticks.max(1) * 50)
    }

    fn survival_drop_for_block(&self, block: &BlockState) -> Option<ItemStack> {
        let key = match block.key.as_str() {
            catalog::STONE => catalog::COBBLESTONE,
            catalog::GRASS_BLOCK => catalog::DIRT,
            catalog::DIRT => catalog::DIRT,
            catalog::COBBLESTONE => catalog::COBBLESTONE,
            catalog::OAK_PLANKS => catalog::OAK_PLANKS,
            catalog::SAND => catalog::SAND,
            catalog::SANDSTONE => catalog::SANDSTONE,
            catalog::BRICKS => catalog::BRICKS,
            catalog::CRAFTING_TABLE => catalog::CRAFTING_TABLE,
            catalog::CHEST => catalog::CHEST,
            catalog::FURNACE => catalog::FURNACE,
            catalog::GLASS => return None,
            _ => return None,
        };
        Some(ItemStack::new(key, 1, 0))
    }

    fn normalize_container(&self, state: &mut OpenContainerState) {
        match state.kind.as_str() {
            ids::CRAFTING_TABLE => recompute_crafting_result(&mut state.local_slots, 3),
            ids::FURNACE => normalize_furnace(state),
            _ => {}
        }
    }

    fn normalize_player_inventory(&self, inventory: &mut PlayerInventory) {
        recompute_crafting_result_for_player(inventory);
    }

    fn try_take_output(
        &self,
        kind: &ContainerKindId,
        local_slots: &mut Vec<Option<ItemStack>>,
        cursor: &mut Option<ItemStack>,
        button: InventoryClickButton,
    ) -> bool {
        match kind.as_str() {
            ids::PLAYER => take_player_crafting_result(local_slots, cursor, button),
            ids::CRAFTING_TABLE => take_container_crafting_result(local_slots, cursor, button),
            ids::FURNACE => take_output_slot(local_slots, cursor, 2, button),
            _ => false,
        }
    }

    fn tick_container(&self, state: &mut OpenContainerState) {
        if state.kind.as_str() == ids::FURNACE {
            tick_furnace(state);
        }
    }
}

fn take_output_slot(
    local_slots: &mut Vec<Option<ItemStack>>,
    cursor: &mut Option<ItemStack>,
    output_index: usize,
    button: InventoryClickButton,
) -> bool {
    let Some(output) = local_slots.get(output_index).and_then(Clone::clone) else {
        return false;
    };
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
        }
        Some(existing)
            if existing.key == output.key
                && u16::from(existing.count) + u16::from(take_count) <= 64 =>
        {
            existing.count = existing.count.saturating_add(take_count);
        }
        Some(_) => return false,
    }
    if let Some(slot) = local_slots.get_mut(output_index)
        && let Some(existing) = slot.as_mut()
    {
        existing.count = existing.count.saturating_sub(take_count);
        if existing.count == 0 {
            *slot = None;
        }
    }
    true
}

fn recompute_crafting_result_for_player(inventory: &mut PlayerInventory) {
    let mut local_slots = (0_u16..PLAYER_LOCAL_SLOT_COUNT)
        .map(|index| {
            inventory
                .get_slot(InventorySlot::WindowLocal(index))
                .cloned()
        })
        .collect::<Vec<_>>();
    recompute_crafting_result(&mut local_slots, 2);
    for (index, stack) in local_slots.into_iter().enumerate() {
        let _ = inventory.set_slot(
            InventorySlot::WindowLocal(u16::try_from(index).expect("player local slot fits")),
            stack,
        );
    }
}

fn take_player_crafting_result(
    local_slots: &mut Vec<Option<ItemStack>>,
    cursor: &mut Option<ItemStack>,
    button: InventoryClickButton,
) -> bool {
    take_crafting_result(local_slots, 0, 1, 4, 2, cursor, button)
}

fn take_container_crafting_result(
    local_slots: &mut Vec<Option<ItemStack>>,
    cursor: &mut Option<ItemStack>,
    button: InventoryClickButton,
) -> bool {
    take_crafting_result(local_slots, 0, 1, 9, 3, cursor, button)
}

fn take_crafting_result(
    local_slots: &mut Vec<Option<ItemStack>>,
    result_index: usize,
    input_start: usize,
    input_count: usize,
    width: usize,
    cursor: &mut Option<ItemStack>,
    button: InventoryClickButton,
) -> bool {
    let inputs = local_slots
        .iter()
        .skip(input_start)
        .take(input_count)
        .cloned()
        .collect::<Vec<_>>();
    let Some((output, consume)) = current_crafting_recipe(&inputs, width) else {
        recompute_crafting_result(local_slots, width);
        return false;
    };
    let take_count = match button {
        InventoryClickButton::Left => output.count,
        InventoryClickButton::Right => 1,
    };
    match cursor.as_mut() {
        None => {
            let mut taken = output.clone();
            taken.count = take_count;
            *cursor = Some(taken);
        }
        Some(existing)
            if existing.key == output.key
                && u16::from(existing.count) + u16::from(take_count) <= 64 =>
        {
            existing.count = existing.count.saturating_add(take_count);
        }
        Some(_) => return false,
    }
    for index in consume {
        if let Some(Some(stack)) = local_slots.get_mut(input_start + index) {
            stack.count = stack.count.saturating_sub(1);
            if stack.count == 0 {
                local_slots[input_start + index] = None;
            }
        }
    }
    recompute_crafting_result(local_slots, width);
    let _ = result_index;
    true
}

fn recompute_crafting_result(local_slots: &mut [Option<ItemStack>], width: usize) {
    let (result_index, inputs) = if width == 2 {
        (
            0,
            local_slots
                .iter()
                .skip(1)
                .take(4)
                .cloned()
                .collect::<Vec<_>>(),
        )
    } else {
        (
            0,
            local_slots
                .iter()
                .skip(1)
                .take(9)
                .cloned()
                .collect::<Vec<_>>(),
        )
    };
    local_slots[result_index] = current_crafting_recipe(&inputs, width).map(|(output, _)| output);
}

fn current_crafting_recipe(
    inputs: &[Option<ItemStack>],
    width: usize,
) -> Option<(ItemStack, Vec<usize>)> {
    let occupied = inputs
        .iter()
        .enumerate()
        .filter_map(|(index, stack)| stack.as_ref().map(|stack| (index, stack)))
        .collect::<Vec<_>>();
    if occupied.is_empty() {
        return None;
    }
    let min_x = occupied.iter().map(|(index, _)| index % width).min()?;
    let max_x = occupied.iter().map(|(index, _)| index % width).max()?;
    let min_y = occupied.iter().map(|(index, _)| index / width).min()?;
    let max_y = occupied.iter().map(|(index, _)| index / width).max()?;
    let normalized_width = max_x - min_x + 1;
    let normalized_height = max_y - min_y + 1;
    let mut cells = vec![None; normalized_width * normalized_height];
    let mut original_indices = vec![None; normalized_width * normalized_height];
    for (index, stack) in occupied {
        let x = index % width - min_x;
        let y = index / width - min_y;
        let normalized = y * normalized_width + x;
        cells[normalized] = Some(stack.key.as_str());
        original_indices[normalized] = Some(index);
    }
    let defs = [
        (
            vec![Some(catalog::OAK_LOG)],
            1,
            1,
            catalog::OAK_PLANKS,
            4_u8,
        ),
        (
            vec![Some(catalog::OAK_PLANKS), Some(catalog::OAK_PLANKS)],
            1,
            2,
            catalog::STICK,
            4_u8,
        ),
        (
            vec![
                Some(catalog::SAND),
                Some(catalog::SAND),
                Some(catalog::SAND),
                Some(catalog::SAND),
            ],
            2,
            2,
            catalog::SANDSTONE,
            1_u8,
        ),
        (
            vec![
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
            ],
            2,
            2,
            catalog::CRAFTING_TABLE,
            1_u8,
        ),
        (
            vec![
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                None,
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
                Some(catalog::OAK_PLANKS),
            ],
            3,
            3,
            catalog::CHEST,
            1_u8,
        ),
        (
            vec![
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
                None,
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
                Some(catalog::COBBLESTONE),
            ],
            3,
            3,
            catalog::FURNACE,
            1_u8,
        ),
    ];
    for (pattern, pattern_width, pattern_height, output_key, output_count) in defs {
        if pattern_width != normalized_width || pattern_height != normalized_height {
            continue;
        }
        if pattern
            .iter()
            .zip(&cells)
            .all(|(expected, actual)| *expected == actual.as_ref().copied())
        {
            let consume = original_indices.into_iter().flatten().collect::<Vec<_>>();
            return Some((ItemStack::new(output_key, output_count, 0), consume));
        }
    }
    None
}

fn normalize_furnace(state: &mut OpenContainerState) {
    state.properties.insert(
        ContainerPropertyKey::new(ids::FURNACE_COOK_TOTAL),
        FURNACE_COOK_TOTAL,
    );
    let available_output = furnace_available_output(&state.local_slots);
    if state
        .properties
        .get(&ContainerPropertyKey::new(ids::FURNACE_BURN_LEFT))
        .copied()
        .unwrap_or_default()
        == 0
    {
        state
            .properties
            .insert(ContainerPropertyKey::new(ids::FURNACE_BURN_MAX), 0);
    }
    if available_output.is_none() {
        state
            .properties
            .insert(ContainerPropertyKey::new(ids::FURNACE_COOK_PROGRESS), 0);
    }
}

fn furnace_available_output(local_slots: &[Option<ItemStack>]) -> Option<ItemStack> {
    let input = local_slots.first().and_then(Option::as_ref)?;
    let output = match input.key.as_str() {
        catalog::SAND if input.count > 0 => ItemStack::new(catalog::GLASS, 1, 0),
        catalog::COBBLESTONE if input.count > 0 => ItemStack::new(catalog::STONE, 1, 0),
        _ => return None,
    };
    match local_slots.get(2).and_then(Option::as_ref) {
        None => Some(output),
        Some(existing) if existing.key == output.key && existing.count < 64 => Some(output),
        _ => None,
    }
}

fn furnace_fuel_burn_time(fuel: Option<&ItemStack>) -> Option<i16> {
    let fuel = fuel?;
    if fuel.count == 0 {
        return None;
    }
    match fuel.key.as_str() {
        catalog::STICK => Some(100),
        catalog::OAK_PLANKS | catalog::OAK_LOG => Some(300),
        _ => None,
    }
}

fn tick_furnace(state: &mut OpenContainerState) {
    normalize_furnace(state);
    let burn_left_key = ContainerPropertyKey::new(ids::FURNACE_BURN_LEFT);
    let burn_max_key = ContainerPropertyKey::new(ids::FURNACE_BURN_MAX);
    let cook_progress_key = ContainerPropertyKey::new(ids::FURNACE_COOK_PROGRESS);
    let cook_total = state
        .properties
        .get(&ContainerPropertyKey::new(ids::FURNACE_COOK_TOTAL))
        .copied()
        .unwrap_or(FURNACE_COOK_TOTAL);
    let burn_left = state.properties.get(&burn_left_key).copied().unwrap_or(0);
    if burn_left > 0 {
        state
            .properties
            .insert(burn_left_key.clone(), burn_left.saturating_sub(1));
    }
    let current_burn_left = state.properties.get(&burn_left_key).copied().unwrap_or(0);
    if current_burn_left == 0
        && furnace_available_output(&state.local_slots).is_some()
        && let Some(burn_time) =
            furnace_fuel_burn_time(state.local_slots.get(1).and_then(Option::as_ref))
    {
        state.properties.insert(burn_left_key.clone(), burn_time);
        state.properties.insert(burn_max_key.clone(), burn_time);
        if let Some(Some(fuel)) = state.local_slots.get_mut(1) {
            fuel.count = fuel.count.saturating_sub(1);
            if fuel.count == 0 {
                state.local_slots[1] = None;
            }
        }
    }
    let active_burn = state.properties.get(&burn_left_key).copied().unwrap_or(0);
    if active_burn > 0 {
        if let Some(output) = furnace_available_output(&state.local_slots) {
            let progress = state
                .properties
                .get(&cook_progress_key)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            if progress >= cook_total {
                if let Some(Some(input)) = state.local_slots.get_mut(0) {
                    input.count = input.count.saturating_sub(1);
                    if input.count == 0 {
                        state.local_slots[0] = None;
                    }
                }
                match state.local_slots.get_mut(2) {
                    Some(Some(existing)) if existing.key == output.key => {
                        existing.count = existing.count.saturating_add(output.count);
                    }
                    Some(slot) => *slot = Some(output),
                    None => {}
                }
                state.properties.insert(cook_progress_key, 0);
            } else {
                state.properties.insert(cook_progress_key, progress);
            }
        } else {
            state.properties.insert(cook_progress_key, 0);
        }
    } else {
        state.properties.insert(cook_progress_key, 0);
        state.properties.insert(burn_max_key, 0);
    }
}
