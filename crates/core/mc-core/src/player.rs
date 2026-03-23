use crate::catalog;
use crate::world::{DimensionId, Vec3};
use crate::{
    AUXILIARY_SLOT_COUNT, HOTBAR_SLOT_COUNT, HOTBAR_START_SLOT, MAIN_INVENTORY_SLOT_COUNT,
    PLAYER_INVENTORY_SLOT_COUNT, PlayerId,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ItemKey(String);

impl ItemKey {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    pub key: ItemKey,
    pub count: u8,
    pub damage: u16,
}

impl ItemStack {
    #[must_use]
    pub fn new(key: impl Into<String>, count: u8, damage: u16) -> Self {
        Self {
            key: ItemKey::new(key),
            count,
            damage,
        }
    }

    #[must_use]
    pub fn unsupported(count: u8, damage: u16) -> Self {
        Self::new("minecraft:unsupported", count, damage)
    }

    #[must_use]
    pub fn is_supported_placeable(&self) -> bool {
        catalog::is_supported_placeable_item(self.key.as_str())
    }

    #[must_use]
    pub fn is_supported_inventory_item(&self) -> bool {
        catalog::is_supported_inventory_item(self.key.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerInventory {
    pub slots: Vec<Option<ItemStack>>,
    #[serde(default)]
    pub offhand: Option<ItemStack>,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self::new_empty()
    }
}

impl PlayerInventory {
    #[must_use]
    pub fn new_empty() -> Self {
        Self {
            slots: vec![None; PLAYER_INVENTORY_SLOT_COUNT],
            offhand: None,
        }
    }

    #[must_use]
    pub fn creative_starter() -> Self {
        let mut inventory = Self::new_empty();
        for (slot, key) in (HOTBAR_START_SLOT..HOTBAR_START_SLOT + HOTBAR_SLOT_COUNT)
            .zip(catalog::starter_hotbar_item_keys())
        {
            let _ = inventory.set(slot, Some(ItemStack::new(key, 64, 0)));
        }
        inventory
    }

    #[must_use]
    pub fn get(&self, slot: u8) -> Option<&ItemStack> {
        self.slots
            .get(usize::from(slot))
            .and_then(std::option::Option::as_ref)
    }

    pub fn set(&mut self, slot: u8, stack: Option<ItemStack>) -> bool {
        if usize::from(slot) >= PLAYER_INVENTORY_SLOT_COUNT {
            return false;
        }
        self.slots[usize::from(slot)] = stack;
        true
    }

    #[must_use]
    pub fn selected_hotbar_stack(&self, selected_hotbar_slot: u8) -> Option<&ItemStack> {
        if selected_hotbar_slot >= HOTBAR_SLOT_COUNT {
            return None;
        }
        self.get(HOTBAR_START_SLOT + selected_hotbar_slot)
    }

    #[must_use]
    pub fn get_slot(&self, slot: InventorySlot) -> Option<&ItemStack> {
        match slot {
            InventorySlot::Offhand => self.offhand.as_ref(),
            _ => slot
                .legacy_window_index()
                .and_then(|legacy_slot| self.get(legacy_slot)),
        }
    }

    pub fn set_slot(&mut self, slot: InventorySlot, stack: Option<ItemStack>) -> bool {
        match slot {
            InventorySlot::Offhand => {
                self.offhand = stack;
                true
            }
            _ => slot
                .legacy_window_index()
                .is_some_and(|legacy_slot| self.set(legacy_slot, stack)),
        }
    }

    #[must_use]
    pub fn selected_stack(
        &self,
        selected_hotbar_slot: u8,
        hand: InteractionHand,
    ) -> Option<&ItemStack> {
        match hand {
            InteractionHand::Main => self.selected_hotbar_stack(selected_hotbar_slot),
            InteractionHand::Offhand => self.offhand.as_ref(),
        }
    }

    #[must_use]
    pub fn crafting_result(&self) -> Option<&ItemStack> {
        self.get_slot(InventorySlot::crafting_result())
    }

    pub fn set_crafting_result(&mut self, stack: Option<ItemStack>) -> bool {
        self.set_slot(InventorySlot::crafting_result(), stack)
    }

    #[must_use]
    pub fn crafting_input(&self, index: u8) -> Option<&ItemStack> {
        InventorySlot::crafting_input(index).and_then(|slot| self.get_slot(slot))
    }

    pub fn set_crafting_input(&mut self, index: u8, stack: Option<ItemStack>) -> bool {
        InventorySlot::crafting_input(index).is_some_and(|slot| self.set_slot(slot, stack))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InventoryContainer {
    Player,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InventorySlot {
    Auxiliary(u8),
    MainInventory(u8),
    Hotbar(u8),
    Offhand,
}

impl InventorySlot {
    #[must_use]
    pub const fn crafting_result() -> Self {
        Self::Auxiliary(0)
    }

    #[must_use]
    pub const fn crafting_input(index: u8) -> Option<Self> {
        if index < 4 {
            Some(Self::Auxiliary(index + 1))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn legacy_window_index(self) -> Option<u8> {
        match self {
            Self::Auxiliary(index) if index < AUXILIARY_SLOT_COUNT => Some(index),
            Self::MainInventory(index) if index < MAIN_INVENTORY_SLOT_COUNT => {
                Some(AUXILIARY_SLOT_COUNT + index)
            }
            Self::Hotbar(index) if index < HOTBAR_SLOT_COUNT => Some(HOTBAR_START_SLOT + index),
            _ => None,
        }
    }

    #[must_use]
    pub const fn from_legacy_window_index(index: u8) -> Option<Self> {
        if index < AUXILIARY_SLOT_COUNT {
            Some(Self::Auxiliary(index))
        } else if index < HOTBAR_START_SLOT {
            Some(Self::MainInventory(index - AUXILIARY_SLOT_COUNT))
        } else if index < HOTBAR_START_SLOT + HOTBAR_SLOT_COUNT {
            Some(Self::Hotbar(index - HOTBAR_START_SLOT))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn is_storage_slot(self) -> bool {
        matches!(
            self,
            Self::MainInventory(_) | Self::Hotbar(_) | Self::Offhand
        )
    }

    #[must_use]
    pub const fn is_crafting_result(self) -> bool {
        matches!(self, Self::Auxiliary(0))
    }

    #[must_use]
    pub const fn crafting_input_index(self) -> Option<u8> {
        match self {
            Self::Auxiliary(index) if index >= 1 && index <= 4 => Some(index - 1),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_reserved_auxiliary(self) -> bool {
        matches!(self, Self::Auxiliary(5..=8))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InteractionHand {
    Main,
    Offhand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    pub id: PlayerId,
    pub username: String,
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub dimension: DimensionId,
    pub health: f32,
    pub food: i16,
    pub food_saturation: f32,
    pub inventory: PlayerInventory,
    pub selected_hotbar_slot: u8,
}
