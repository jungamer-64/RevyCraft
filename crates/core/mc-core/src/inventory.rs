use crate::{
    AUXILIARY_SLOT_COUNT, HOTBAR_SLOT_COUNT, HOTBAR_START_SLOT, MAIN_INVENTORY_SLOT_COUNT,
    PLAYER_INVENTORY_SLOT_COUNT,
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

    pub fn get_slot_mut(&mut self, slot: InventorySlot) -> Option<&mut Option<ItemStack>> {
        match slot {
            InventorySlot::Offhand => Some(&mut self.offhand),
            _ => slot
                .legacy_window_index()
                .and_then(|legacy_slot| self.slots.get_mut(usize::from(legacy_slot))),
        }
    }

    #[must_use]
    pub fn selected_stack(
        &self,
        selected_hotbar_slot: u8,
        hand: crate::InteractionHand,
    ) -> Option<&ItemStack> {
        match hand {
            crate::InteractionHand::Main => self.selected_hotbar_stack(selected_hotbar_slot),
            crate::InteractionHand::Offhand => self.offhand.as_ref(),
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
pub enum InventorySlot {
    WindowLocal(u16),
    MainInventory(u8),
    Hotbar(u8),
    Offhand,
}

impl InventorySlot {
    #[must_use]
    pub const fn crafting_result() -> Self {
        Self::WindowLocal(0)
    }

    #[must_use]
    pub const fn crafting_input(index: u8) -> Option<Self> {
        if index < 4 {
            Some(Self::WindowLocal((index + 1) as u16))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn container(index: u8) -> Self {
        Self::WindowLocal(index as u16)
    }

    #[must_use]
    pub(crate) const fn legacy_window_index(self) -> Option<u8> {
        match self {
            Self::WindowLocal(index) if index < AUXILIARY_SLOT_COUNT as u16 => Some(index as u8),
            Self::MainInventory(index) if index < MAIN_INVENTORY_SLOT_COUNT => {
                Some(AUXILIARY_SLOT_COUNT + index)
            }
            Self::Hotbar(index) if index < HOTBAR_SLOT_COUNT => Some(HOTBAR_START_SLOT + index),
            _ => None,
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
    pub const fn container_index(self) -> Option<u8> {
        match self {
            Self::WindowLocal(index) if index <= u8::MAX as u16 => Some(index as u8),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_crafting_result(self) -> bool {
        matches!(self, Self::WindowLocal(0))
    }

    #[must_use]
    pub const fn crafting_input_index(self) -> Option<u8> {
        match self {
            Self::WindowLocal(index) if index >= 1 && index <= 4 => Some(index as u8 - 1),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_reserved_auxiliary(self) -> bool {
        matches!(self, Self::WindowLocal(5..=8))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryWindowContents {
    pub player_inventory: PlayerInventory,
    #[serde(default, alias = "container_slots")]
    pub local_slots: Vec<Option<ItemStack>>,
}

impl InventoryWindowContents {
    #[must_use]
    pub fn player(player_inventory: PlayerInventory) -> Self {
        Self {
            player_inventory,
            local_slots: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_local_slots(
        player_inventory: PlayerInventory,
        local_slots: Vec<Option<ItemStack>>,
    ) -> Self {
        Self {
            player_inventory,
            local_slots,
        }
    }

    #[must_use]
    pub fn with_container(
        player_inventory: PlayerInventory,
        container_slots: Vec<Option<ItemStack>>,
    ) -> Self {
        Self::with_local_slots(player_inventory, container_slots)
    }

    #[must_use]
    pub fn get_slot(&self, slot: InventorySlot) -> Option<&ItemStack> {
        match slot {
            InventorySlot::WindowLocal(_index) if self.local_slots.is_empty() => {
                self.player_inventory.get_slot(slot)
            }
            InventorySlot::WindowLocal(index) => self
                .local_slots
                .get(usize::from(index))
                .and_then(std::option::Option::as_ref),
            _ => self.player_inventory.get_slot(slot),
        }
    }
}
