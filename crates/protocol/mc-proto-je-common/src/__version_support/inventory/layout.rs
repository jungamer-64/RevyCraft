use super::slot_codec::SlotNbtEncoding;
use mc_core::{InventoryContainer, InventorySlot};

pub const PLAYER_WINDOW_CRAFTING_RESULT_SLOT: i16 = 0;
pub const PLAYER_WINDOW_CRAFTING_INPUT_SLOTS: [i16; 4] = [1, 2, 3, 4];
pub const CURSOR_WINDOW_ID: i8 = -1;
pub const CURSOR_SLOT_ID: i16 = -1;
pub const CRAFTING_TABLE_WINDOW_TYPE: &str = "minecraft:crafting_table";
pub const CHEST_WINDOW_TYPE: &str = "minecraft:chest";
pub const FURNACE_WINDOW_TYPE: &str = "minecraft:furnace";
const LEGACY_PLAYER_AUXILIARY_SLOT_COUNT: u8 = 9;
const LEGACY_PLAYER_HOTBAR_START_SLOT: u8 = 36;
const LEGACY_PLAYER_HOTBAR_SLOT_COUNT: u8 = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerInventoryLayout {
    Legacy,
    ModernWithOffhand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InventoryProtocolSpec {
    pub slot_nbt: SlotNbtEncoding,
    pub layout: PlayerInventoryLayout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ContainerDescriptor {
    pub(super) window_type: &'static str,
    pub(super) unique_slot_count: u8,
    pub(super) local_slot_count: u8,
    pub(super) main_inventory_start: i16,
    pub(super) hotbar_start: i16,
}

pub const JE_1_7_10_INVENTORY_SPEC: InventoryProtocolSpec = InventoryProtocolSpec {
    slot_nbt: SlotNbtEncoding::LengthPrefixedBlob,
    layout: PlayerInventoryLayout::Legacy,
};

pub const JE_1_8_X_INVENTORY_SPEC: InventoryProtocolSpec = InventoryProtocolSpec {
    slot_nbt: SlotNbtEncoding::RootTag,
    layout: PlayerInventoryLayout::Legacy,
};

pub const JE_1_12_2_INVENTORY_SPEC: InventoryProtocolSpec = InventoryProtocolSpec {
    slot_nbt: SlotNbtEncoding::RootTag,
    layout: PlayerInventoryLayout::ModernWithOffhand,
};

#[must_use]
pub const fn player_window_id(_container: InventoryContainer) -> u8 {
    0
}

#[must_use]
pub const fn signed_window_id(window_id: u8) -> i8 {
    i8::from_be_bytes([window_id])
}

#[must_use]
pub const fn unique_slot_count(container: InventoryContainer) -> u8 {
    container_descriptor(container).unique_slot_count
}

#[must_use]
pub const fn window_type(container: InventoryContainer) -> &'static str {
    container_descriptor(container).window_type
}

#[must_use]
pub const fn protocol_slot(
    container: InventoryContainer,
    layout: PlayerInventoryLayout,
    slot: InventorySlot,
) -> Option<i16> {
    match container {
        InventoryContainer::Player => match layout {
            PlayerInventoryLayout::Legacy => match legacy_player_window_index(slot) {
                Some(index) => Some(index as i16),
                None => None,
            },
            PlayerInventoryLayout::ModernWithOffhand => match slot {
                InventorySlot::Offhand => Some(45),
                _ => match legacy_player_window_index(slot) {
                    Some(index) => Some(index as i16),
                    None => None,
                },
            },
        },
        InventoryContainer::CraftingTable
        | InventoryContainer::Chest
        | InventoryContainer::Furnace => {
            let descriptor = container_descriptor(container);
            match slot {
                InventorySlot::Container(index) if index < descriptor.local_slot_count => {
                    Some(index as i16)
                }
                InventorySlot::MainInventory(index) if index < 27 => {
                    Some(index as i16 + descriptor.main_inventory_start)
                }
                InventorySlot::Hotbar(index) if index < 9 => {
                    Some(index as i16 + descriptor.hotbar_start)
                }
                _ => None,
            }
        }
    }
}

#[must_use]
pub fn inventory_slot(
    container: InventoryContainer,
    layout: PlayerInventoryLayout,
    raw_slot: i16,
) -> Option<InventorySlot> {
    match container {
        InventoryContainer::Player => match layout {
            PlayerInventoryLayout::Legacy => u8::try_from(raw_slot)
                .ok()
                .and_then(legacy_player_inventory_slot),
            PlayerInventoryLayout::ModernWithOffhand => {
                if raw_slot == 45 {
                    Some(InventorySlot::Offhand)
                } else {
                    u8::try_from(raw_slot)
                        .ok()
                        .and_then(legacy_player_inventory_slot)
                }
            }
        },
        InventoryContainer::CraftingTable
        | InventoryContainer::Chest
        | InventoryContainer::Furnace => {
            let descriptor = container_descriptor(container);
            match raw_slot {
                raw if raw >= 0 && raw < i16::from(descriptor.local_slot_count) => {
                    Some(InventorySlot::Container(
                        u8::try_from(raw).expect("container slot should fit into u8"),
                    ))
                }
                raw if raw >= descriptor.main_inventory_start && raw < descriptor.hotbar_start => {
                    Some(InventorySlot::MainInventory(
                        u8::try_from(raw - descriptor.main_inventory_start)
                            .expect("main inventory slot should fit into u8"),
                    ))
                }
                raw if raw >= descriptor.hotbar_start && raw < descriptor.hotbar_start + 9 => {
                    Some(InventorySlot::Hotbar(
                        u8::try_from(raw - descriptor.hotbar_start)
                            .expect("hotbar slot should fit into u8"),
                    ))
                }
                _ => None,
            }
        }
    }
}

pub(super) const fn container_descriptor(container: InventoryContainer) -> ContainerDescriptor {
    match container {
        InventoryContainer::Player => ContainerDescriptor {
            window_type: "minecraft:container",
            unique_slot_count: 10,
            local_slot_count: 9,
            main_inventory_start: 9,
            hotbar_start: 36,
        },
        InventoryContainer::CraftingTable => ContainerDescriptor {
            window_type: CRAFTING_TABLE_WINDOW_TYPE,
            unique_slot_count: 0,
            local_slot_count: 10,
            main_inventory_start: 10,
            hotbar_start: 37,
        },
        InventoryContainer::Chest => ContainerDescriptor {
            window_type: CHEST_WINDOW_TYPE,
            unique_slot_count: 27,
            local_slot_count: 27,
            main_inventory_start: 27,
            hotbar_start: 54,
        },
        InventoryContainer::Furnace => ContainerDescriptor {
            window_type: FURNACE_WINDOW_TYPE,
            unique_slot_count: 3,
            local_slot_count: 3,
            main_inventory_start: 3,
            hotbar_start: 30,
        },
    }
}

const fn legacy_player_window_index(slot: InventorySlot) -> Option<u8> {
    match slot {
        InventorySlot::Auxiliary(index) if index < LEGACY_PLAYER_AUXILIARY_SLOT_COUNT => {
            Some(index)
        }
        InventorySlot::MainInventory(index) if index < 27 => {
            Some(LEGACY_PLAYER_AUXILIARY_SLOT_COUNT + index)
        }
        InventorySlot::Hotbar(index) if index < LEGACY_PLAYER_HOTBAR_SLOT_COUNT => {
            Some(LEGACY_PLAYER_HOTBAR_START_SLOT + index)
        }
        _ => None,
    }
}

const fn legacy_player_inventory_slot(index: u8) -> Option<InventorySlot> {
    if index < LEGACY_PLAYER_AUXILIARY_SLOT_COUNT {
        Some(InventorySlot::Auxiliary(index))
    } else if index < LEGACY_PLAYER_HOTBAR_START_SLOT {
        Some(InventorySlot::MainInventory(
            index - LEGACY_PLAYER_AUXILIARY_SLOT_COUNT,
        ))
    } else if index < LEGACY_PLAYER_HOTBAR_START_SLOT + LEGACY_PLAYER_HOTBAR_SLOT_COUNT {
        Some(InventorySlot::Hotbar(
            index - LEGACY_PLAYER_HOTBAR_START_SLOT,
        ))
    } else {
        None
    }
}
