use crate::__version_support::blocks::{legacy_item, semantic_item};
use mc_core::{InventoryContainer, InventorySlot, ItemStack, PlayerInventory};
use mc_proto_common::{PacketReader, PacketWriter, ProtocolError};

#[must_use]
pub const fn player_window_id(container: InventoryContainer) -> u8 {
    match container {
        InventoryContainer::Player => 0,
    }
}

pub fn read_legacy_slot(reader: &mut PacketReader<'_>) -> Result<Option<ItemStack>, ProtocolError> {
    let item_id = reader.read_i16()?;
    if item_id < 0 {
        return Ok(None);
    }
    let count = reader.read_u8()?;
    let damage = u16::from_be_bytes(reader.read_i16()?.to_be_bytes());
    skip_slot_nbt(reader)?;
    Ok(Some(semantic_item(item_id, damage, count)))
}

pub fn write_legacy_slot(
    writer: &mut PacketWriter,
    stack: Option<&ItemStack>,
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
    writer.write_i16(-1);
    Ok(())
}

#[must_use]
pub fn legacy_window_slot(slot: InventorySlot) -> Option<i16> {
    slot.legacy_window_index().map(i16::from)
}

#[must_use]
pub const fn modern_window_slot(slot: InventorySlot) -> Option<i16> {
    match slot {
        InventorySlot::Offhand => Some(45),
        _ => match slot.legacy_window_index() {
            Some(index) => Some(index as i16),
            None => None,
        },
    }
}

#[must_use]
pub fn legacy_inventory_slot(raw_slot: i16) -> Option<InventorySlot> {
    u8::try_from(raw_slot)
        .ok()
        .and_then(InventorySlot::from_legacy_window_index)
}

#[must_use]
pub fn modern_inventory_slot(raw_slot: i16) -> Option<InventorySlot> {
    if raw_slot == 45 {
        Some(InventorySlot::Offhand)
    } else {
        legacy_inventory_slot(raw_slot)
    }
}

#[must_use]
pub fn legacy_window_items(inventory: &PlayerInventory) -> Vec<Option<ItemStack>> {
    inventory.slots.clone()
}

#[must_use]
pub fn modern_window_items(inventory: &PlayerInventory) -> Vec<Option<ItemStack>> {
    let mut items = inventory.slots.clone();
    items.push(inventory.offhand.clone());
    items
}

fn skip_slot_nbt(reader: &mut PacketReader<'_>) -> Result<(), ProtocolError> {
    let length = reader.read_i16()?;
    if length < 0 {
        return Ok(());
    }
    let length = usize::try_from(length)
        .map_err(|_| ProtocolError::InvalidPacket("negative slot nbt length"))?;
    let _ = reader.read_bytes(length)?;
    Ok(())
}
