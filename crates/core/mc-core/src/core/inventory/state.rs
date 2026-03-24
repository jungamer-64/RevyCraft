use crate::inventory::{InventoryContainer, InventoryWindowContents, ItemStack, PlayerInventory};
use crate::world::BlockPos;

pub(super) const CRAFTING_TABLE_LOCAL_SLOT_COUNT: usize = 10;
pub(super) const CHEST_LOCAL_SLOT_COUNT: usize = 27;
pub(super) const FURNACE_COOK_TOTAL: i16 = 200;
const FURNACE_PROPERTY_BURN_LEFT: u8 = 0;
const FURNACE_PROPERTY_BURN_MAX: u8 = 1;
const FURNACE_PROPERTY_COOK_PROGRESS: u8 = 2;
const FURNACE_PROPERTY_COOK_TOTAL: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContainerDescriptor {
    pub(crate) local_slot_count: u8,
    pub(crate) main_inventory_start: i16,
    pub(crate) hotbar_start: i16,
}

#[derive(Clone, Debug)]
pub(crate) struct OpenInventoryWindow {
    pub(crate) window_id: u8,
    pub(crate) container: InventoryContainer,
    pub(crate) state: OpenInventoryWindowState,
}

#[derive(Clone, Debug)]
pub(crate) enum OpenInventoryWindowState {
    CraftingTable { slots: Vec<Option<ItemStack>> },
    Chest(ChestWindowState),
    Furnace(FurnaceWindowState),
}

#[derive(Clone, Debug)]
pub(crate) struct ChestWindowState {
    pub(crate) binding: ChestWindowBinding,
    pub(crate) slots: Vec<Option<ItemStack>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChestWindowBinding {
    Virtual,
    Block(BlockPos),
}

#[derive(Clone, Debug)]
pub(crate) struct FurnaceWindowState {
    pub(crate) input: Option<ItemStack>,
    pub(crate) fuel: Option<ItemStack>,
    pub(crate) output: Option<ItemStack>,
    pub(crate) burn_left: i16,
    pub(crate) burn_max: i16,
    pub(crate) cook_progress: i16,
    pub(crate) cook_total: i16,
}

impl OpenInventoryWindow {
    pub(super) fn contents(&self, player_inventory: &PlayerInventory) -> InventoryWindowContents {
        InventoryWindowContents::with_container(
            player_inventory.clone(),
            match &self.state {
                OpenInventoryWindowState::CraftingTable { slots } => slots.clone(),
                OpenInventoryWindowState::Chest(chest) => chest.slots.clone(),
                OpenInventoryWindowState::Furnace(furnace) => furnace.local_slots(),
            },
        )
    }

    pub(super) fn local_slot_mut(&mut self, index: u8) -> Option<&mut Option<ItemStack>> {
        match &mut self.state {
            OpenInventoryWindowState::CraftingTable { slots } => slots.get_mut(usize::from(index)),
            OpenInventoryWindowState::Chest(chest) => chest.slots.get_mut(usize::from(index)),
            OpenInventoryWindowState::Furnace(furnace) => furnace.slot_mut(index),
        }
    }

    pub(super) fn property_entries(&self) -> Vec<(u8, i16)> {
        match &self.state {
            OpenInventoryWindowState::CraftingTable { .. } | OpenInventoryWindowState::Chest(_) => {
                Vec::new()
            }
            OpenInventoryWindowState::Furnace(furnace) => furnace.property_entries(),
        }
    }

    pub(super) fn world_chest_position(&self) -> Option<BlockPos> {
        match &self.state {
            OpenInventoryWindowState::Chest(chest) => chest.world_position(),
            OpenInventoryWindowState::CraftingTable { .. }
            | OpenInventoryWindowState::Furnace(_) => None,
        }
    }
}

impl ChestWindowState {
    pub(super) fn new_virtual(local_slot_count: usize) -> Self {
        Self {
            binding: ChestWindowBinding::Virtual,
            slots: vec![None; local_slot_count],
        }
    }

    pub(super) fn new_block(position: BlockPos, slots: Vec<Option<ItemStack>>) -> Self {
        Self {
            binding: ChestWindowBinding::Block(position),
            slots,
        }
    }

    pub(super) fn world_position(&self) -> Option<BlockPos> {
        match self.binding {
            ChestWindowBinding::Virtual => None,
            ChestWindowBinding::Block(position) => Some(position),
        }
    }
}

impl FurnaceWindowState {
    pub(super) const fn new() -> Self {
        Self {
            input: None,
            fuel: None,
            output: None,
            burn_left: 0,
            burn_max: 0,
            cook_progress: 0,
            cook_total: FURNACE_COOK_TOTAL,
        }
    }

    pub(super) fn local_slots(&self) -> Vec<Option<ItemStack>> {
        vec![self.input.clone(), self.fuel.clone(), self.output.clone()]
    }

    pub(super) fn slot_mut(&mut self, index: u8) -> Option<&mut Option<ItemStack>> {
        match index {
            0 => Some(&mut self.input),
            1 => Some(&mut self.fuel),
            2 => Some(&mut self.output),
            _ => None,
        }
    }

    pub(super) fn property_entries(&self) -> Vec<(u8, i16)> {
        vec![
            (FURNACE_PROPERTY_BURN_LEFT, self.burn_left),
            (FURNACE_PROPERTY_BURN_MAX, self.burn_max),
            (FURNACE_PROPERTY_COOK_PROGRESS, self.cook_progress),
            (FURNACE_PROPERTY_COOK_TOTAL, self.cook_total),
        ]
    }
}
