use crate::__version_support::blocks::{legacy_item, semantic_item};
use mc_core::{InventoryContainer, InventorySlot, ItemStack, PlayerInventory};
use mc_proto_common::{PacketReader, PacketWriter, ProtocolError};

pub const PLAYER_WINDOW_CRAFTING_RESULT_SLOT: i16 = 0;
pub const PLAYER_WINDOW_CRAFTING_INPUT_SLOTS: [i16; 4] = [1, 2, 3, 4];
pub const CURSOR_WINDOW_ID: i8 = -1;
pub const CURSOR_SLOT_ID: i16 = -1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotNbtEncoding {
    LengthPrefixedBlob,
    RootTag,
}

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
pub const fn player_window_id(container: InventoryContainer) -> u8 {
    match container {
        InventoryContainer::Player => 0,
    }
}

#[must_use]
pub const fn signed_window_id(window_id: u8) -> i8 {
    i8::from_be_bytes([window_id])
}

#[must_use]
pub const fn player_window_id_signed(container: InventoryContainer) -> i8 {
    signed_window_id(player_window_id(container))
}

pub fn read_slot(
    reader: &mut PacketReader<'_>,
    slot_nbt: SlotNbtEncoding,
) -> Result<Option<ItemStack>, ProtocolError> {
    let item_id = reader.read_i16()?;
    if item_id < 0 {
        return Ok(None);
    }
    let count = reader.read_u8()?;
    let damage = u16::from_be_bytes(reader.read_i16()?.to_be_bytes());
    skip_slot_nbt(reader, slot_nbt)?;
    Ok(Some(semantic_item(item_id, damage, count)))
}

pub fn write_slot(
    writer: &mut PacketWriter,
    stack: Option<&ItemStack>,
    slot_nbt: SlotNbtEncoding,
) -> Result<(), ProtocolError> {
    let Some(stack) = stack else {
        writer.write_i16(-1);
        return Ok(());
    };
    let Some((item_id, damage)) = legacy_item(stack) else {
        return Err(ProtocolError::InvalidPacket("unsupported inventory item"));
    };
    writer.write_i16(item_id);
    writer.write_u8(stack.count);
    writer.write_i16(i16::from_be_bytes(damage.to_be_bytes()));
    match slot_nbt {
        SlotNbtEncoding::LengthPrefixedBlob => writer.write_i16(-1),
        SlotNbtEncoding::RootTag => writer.write_u8(0),
    }
    Ok(())
}

#[must_use]
pub const fn protocol_slot(layout: PlayerInventoryLayout, slot: InventorySlot) -> Option<i16> {
    match layout {
        PlayerInventoryLayout::Legacy => match slot.legacy_window_index() {
            Some(index) => Some(index as i16),
            None => None,
        },
        PlayerInventoryLayout::ModernWithOffhand => match slot {
            InventorySlot::Offhand => Some(45),
            _ => match slot.legacy_window_index() {
                Some(index) => Some(index as i16),
                None => None,
            },
        },
    }
}

#[must_use]
pub fn inventory_slot(layout: PlayerInventoryLayout, raw_slot: i16) -> Option<InventorySlot> {
    match layout {
        PlayerInventoryLayout::Legacy => u8::try_from(raw_slot)
            .ok()
            .and_then(InventorySlot::from_legacy_window_index),
        PlayerInventoryLayout::ModernWithOffhand => {
            if raw_slot == 45 {
                Some(InventorySlot::Offhand)
            } else {
                u8::try_from(raw_slot)
                    .ok()
                    .and_then(InventorySlot::from_legacy_window_index)
            }
        }
    }
}

#[must_use]
pub fn window_items(
    layout: PlayerInventoryLayout,
    inventory: &PlayerInventory,
) -> Vec<Option<ItemStack>> {
    match layout {
        PlayerInventoryLayout::Legacy => inventory.slots.clone(),
        PlayerInventoryLayout::ModernWithOffhand => {
            let mut items = inventory.slots.clone();
            items.push(inventory.offhand.clone());
            items
        }
    }
}

fn skip_slot_nbt(
    reader: &mut PacketReader<'_>,
    slot_nbt: SlotNbtEncoding,
) -> Result<(), ProtocolError> {
    match slot_nbt {
        SlotNbtEncoding::LengthPrefixedBlob => skip_slot_nbt_blob(reader),
        SlotNbtEncoding::RootTag => skip_slot_nbt_root(reader),
    }
}

fn skip_slot_nbt_root(reader: &mut PacketReader<'_>) -> Result<(), ProtocolError> {
    let tag_type = reader.read_u8()?;
    if tag_type == 0 {
        return Ok(());
    }
    skip_nbt_name(reader)?;
    skip_nbt_payload(reader, tag_type)
}

fn skip_nbt_name(reader: &mut PacketReader<'_>) -> Result<(), ProtocolError> {
    let length = usize::from(reader.read_u16()?);
    let _ = reader.read_bytes(length)?;
    Ok(())
}

fn skip_nbt_payload(reader: &mut PacketReader<'_>, tag_type: u8) -> Result<(), ProtocolError> {
    match tag_type {
        1 => {
            let _ = reader.read_u8()?;
        }
        2 => {
            let _ = reader.read_i16()?;
        }
        3 => {
            let _ = reader.read_i32()?;
        }
        4 => {
            let _ = reader.read_i64()?;
        }
        5 => {
            let _ = reader.read_f32()?;
        }
        6 => {
            let _ = reader.read_f64()?;
        }
        7 => skip_nbt_array(reader, 1)?,
        8 => skip_nbt_name(reader)?,
        9 => {
            let child_type = reader.read_u8()?;
            let len = read_nbt_length(reader)?;
            for _ in 0..len {
                skip_nbt_payload(reader, child_type)?;
            }
        }
        10 => loop {
            let child_type = reader.read_u8()?;
            if child_type == 0 {
                break;
            }
            skip_nbt_name(reader)?;
            skip_nbt_payload(reader, child_type)?;
        },
        11 => skip_nbt_array(reader, 4)?,
        12 => skip_nbt_array(reader, 8)?,
        _ => return Err(ProtocolError::InvalidPacket("invalid slot nbt tag type")),
    }
    Ok(())
}

fn skip_nbt_array(
    reader: &mut PacketReader<'_>,
    element_width: usize,
) -> Result<(), ProtocolError> {
    let len = read_nbt_length(reader)?;
    let bytes = len
        .checked_mul(element_width)
        .ok_or(ProtocolError::InvalidPacket("slot nbt array too large"))?;
    let _ = reader.read_bytes(bytes)?;
    Ok(())
}

fn read_nbt_length(reader: &mut PacketReader<'_>) -> Result<usize, ProtocolError> {
    usize::try_from(reader.read_i32()?)
        .map_err(|_| ProtocolError::InvalidPacket("negative slot nbt length"))
}

fn skip_slot_nbt_blob(reader: &mut PacketReader<'_>) -> Result<(), ProtocolError> {
    let length = reader.read_i16()?;
    if length < 0 {
        return Ok(());
    }
    let length = usize::try_from(length)
        .map_err(|_| ProtocolError::InvalidPacket("negative slot nbt length"))?;
    let _ = reader.read_bytes(length)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_core::ItemStack;

    fn stone_stack() -> ItemStack {
        ItemStack::new("minecraft:stone", 32, 0)
    }

    #[test]
    fn length_prefixed_slot_round_trips_with_empty_sentinel() {
        let mut writer = PacketWriter::default();
        write_slot(
            &mut writer,
            Some(&stone_stack()),
            JE_1_7_10_INVENTORY_SPEC.slot_nbt,
        )
        .expect("legacy slot should encode");

        let encoded = writer.into_inner();
        let mut reader = PacketReader::new(&encoded);
        assert_eq!(reader.read_i16().expect("item id should decode"), 1);
        assert_eq!(reader.read_u8().expect("count should decode"), 32);
        assert_eq!(reader.read_i16().expect("damage should decode"), 0);
        assert_eq!(reader.read_i16().expect("nbt sentinel should decode"), -1);

        let mut reader = PacketReader::new(&encoded);
        assert_eq!(
            read_slot(&mut reader, JE_1_7_10_INVENTORY_SPEC.slot_nbt)
                .expect("legacy slot should decode"),
            Some(stone_stack())
        );
    }

    #[test]
    fn root_tag_slot_round_trips_with_end_marker() {
        let mut writer = PacketWriter::default();
        write_slot(
            &mut writer,
            Some(&stone_stack()),
            JE_1_8_X_INVENTORY_SPEC.slot_nbt,
        )
        .expect("root-tag slot should encode");

        let encoded = writer.into_inner();
        let mut reader = PacketReader::new(&encoded);
        assert_eq!(reader.read_i16().expect("item id should decode"), 1);
        assert_eq!(reader.read_u8().expect("count should decode"), 32);
        assert_eq!(reader.read_i16().expect("damage should decode"), 0);
        assert_eq!(reader.read_u8().expect("nbt tag should decode"), 0);

        let mut reader = PacketReader::new(&encoded);
        assert_eq!(
            read_slot(&mut reader, JE_1_8_X_INVENTORY_SPEC.slot_nbt)
                .expect("root-tag slot should decode"),
            Some(stone_stack())
        );
    }

    #[test]
    fn modern_layout_exposes_offhand_slot_without_affecting_legacy_slots() {
        let mut inventory = PlayerInventory::creative_starter();
        inventory.offhand = Some(ItemStack::new("minecraft:brick_block", 1, 0));

        let legacy_items = window_items(PlayerInventoryLayout::Legacy, &inventory);
        let modern_items = window_items(PlayerInventoryLayout::ModernWithOffhand, &inventory);

        assert_eq!(legacy_items.len(), inventory.slots.len());
        assert_eq!(modern_items.len(), inventory.slots.len() + 1);
        assert_eq!(
            protocol_slot(PlayerInventoryLayout::Legacy, InventorySlot::Offhand),
            None
        );
        assert_eq!(
            protocol_slot(
                PlayerInventoryLayout::ModernWithOffhand,
                InventorySlot::Offhand
            ),
            Some(45)
        );
        assert_eq!(
            inventory_slot(PlayerInventoryLayout::ModernWithOffhand, 45),
            Some(InventorySlot::Offhand)
        );
        assert_eq!(inventory_slot(PlayerInventoryLayout::Legacy, 45), None);
        assert_eq!(
            modern_items.last().expect("offhand slot should exist"),
            &inventory.offhand
        );
    }
}
