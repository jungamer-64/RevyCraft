use super::slot_codec::{SlotEncoding, SlotNbtEncoding};
use mc_content_api::ContainerKindId;
use mc_model::InventorySlot;

pub const PLAYER_WINDOW_CRAFTING_RESULT_SLOT: i16 = 0;
pub const PLAYER_WINDOW_CRAFTING_INPUT_SLOTS: [i16; 4] = [1, 2, 3, 4];
pub const CURSOR_WINDOW_ID: i8 = -1;
pub const CURSOR_SLOT_ID: i16 = -1;
pub const CRAFTING_TABLE_WINDOW_TYPE: &str = "minecraft:crafting_table";
pub const CHEST_WINDOW_TYPE: &str = "minecraft:chest";
pub const FURNACE_WINDOW_TYPE: &str = "minecraft:furnace";

pub const PLAYER_KIND: &str = "canonical:player";
pub const CRAFTING_TABLE_KIND: &str = "canonical:crafting_table";
pub const CHEST_27_KIND: &str = "canonical:chest_27";
pub const FURNACE_KIND: &str = "canonical:furnace";

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
    pub slot: SlotEncoding,
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
    slot: SlotEncoding::Legacy {
        nbt: SlotNbtEncoding::LengthPrefixedBlob,
    },
    layout: PlayerInventoryLayout::Legacy,
};

pub const JE_1_8_X_INVENTORY_SPEC: InventoryProtocolSpec = InventoryProtocolSpec {
    slot: SlotEncoding::Legacy {
        nbt: SlotNbtEncoding::RootTag,
    },
    layout: PlayerInventoryLayout::Legacy,
};

pub const JE_1_12_2_INVENTORY_SPEC: InventoryProtocolSpec = InventoryProtocolSpec {
    slot: SlotEncoding::Legacy {
        nbt: SlotNbtEncoding::RootTag,
    },
    layout: PlayerInventoryLayout::ModernWithOffhand,
};

pub const JE_1_13_2_INVENTORY_SPEC: InventoryProtocolSpec = InventoryProtocolSpec {
    slot: SlotEncoding::PresentVarInt {
        nbt: SlotNbtEncoding::RootTag,
    },
    layout: PlayerInventoryLayout::ModernWithOffhand,
};

#[must_use]
pub const fn player_window_id(_container: &ContainerKindId) -> u8 {
    0
}

#[must_use]
pub const fn signed_window_id(window_id: u8) -> i8 {
    i8::from_be_bytes([window_id])
}

#[must_use]
pub fn is_player_container(container: &ContainerKindId) -> bool {
    container.as_str() == PLAYER_KIND
}

#[must_use]
pub fn unique_slot_count(container: &ContainerKindId) -> u8 {
    container_descriptor(container).unique_slot_count
}

#[must_use]
pub fn window_type(container: &ContainerKindId) -> &'static str {
    container_descriptor(container).window_type
}

#[must_use]
pub fn protocol_slot(
    container: &ContainerKindId,
    layout: PlayerInventoryLayout,
    slot: InventorySlot,
) -> Option<i16> {
    if is_player_container(container) {
        return match layout {
            PlayerInventoryLayout::Legacy => legacy_player_window_index(slot).map(i16::from),
            PlayerInventoryLayout::ModernWithOffhand => match slot {
                InventorySlot::Offhand => Some(45),
                _ => legacy_player_window_index(slot).map(i16::from),
            },
        };
    }

    let descriptor = container_descriptor(container);
    match slot {
        InventorySlot::WindowLocal(index) if index < descriptor.local_slot_count as u16 => {
            Some(index as i16)
        }
        InventorySlot::MainInventory(index) if index < 27 => {
            Some(index as i16 + descriptor.main_inventory_start)
        }
        InventorySlot::Hotbar(index) if index < 9 => Some(index as i16 + descriptor.hotbar_start),
        _ => None,
    }
}

#[must_use]
pub fn inventory_slot(
    container: &ContainerKindId,
    layout: PlayerInventoryLayout,
    raw_slot: i16,
) -> Option<InventorySlot> {
    if is_player_container(container) {
        return match layout {
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
        };
    }

    let descriptor = container_descriptor(container);
    match raw_slot {
        raw if raw >= 0 && raw < i16::from(descriptor.local_slot_count) => {
            Some(InventorySlot::WindowLocal(
                u16::try_from(raw).expect("container slot should fit into u16"),
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

pub(super) fn container_descriptor(container: &ContainerKindId) -> ContainerDescriptor {
    match container.as_str() {
        PLAYER_KIND => ContainerDescriptor {
            window_type: "minecraft:container",
            unique_slot_count: 10,
            local_slot_count: 9,
            main_inventory_start: 9,
            hotbar_start: 36,
        },
        CRAFTING_TABLE_KIND => ContainerDescriptor {
            window_type: CRAFTING_TABLE_WINDOW_TYPE,
            unique_slot_count: 0,
            local_slot_count: 10,
            main_inventory_start: 10,
            hotbar_start: 37,
        },
        CHEST_27_KIND => ContainerDescriptor {
            window_type: CHEST_WINDOW_TYPE,
            unique_slot_count: 27,
            local_slot_count: 27,
            main_inventory_start: 27,
            hotbar_start: 54,
        },
        FURNACE_KIND => ContainerDescriptor {
            window_type: FURNACE_WINDOW_TYPE,
            unique_slot_count: 3,
            local_slot_count: 3,
            main_inventory_start: 3,
            hotbar_start: 30,
        },
        _ => panic!("unsupported java inventory container kind: {container}"),
    }
}

const fn legacy_player_window_index(slot: InventorySlot) -> Option<u8> {
    match slot {
        InventorySlot::WindowLocal(index) if index < LEGACY_PLAYER_AUXILIARY_SLOT_COUNT as u16 => {
            Some(index as u8)
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
        Some(InventorySlot::WindowLocal(index as u16))
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
