use crate::codec::__internal::binary::{Decoder, Encoder, ProtocolCodecError};
use crate::codec::__internal::shared::{decode_option, encode_option};
use mc_core::{
    InventoryClickButton, InventoryClickTarget, InventoryClickValidation, InventoryContainer,
    InventorySlot, InventoryTransactionContext, InventoryWindowContents, ItemStack,
    PlayerInventory,
};

pub(crate) fn encode_inventory_container(encoder: &mut Encoder, container: InventoryContainer) {
    encoder.write_u8(match container {
        InventoryContainer::Player => 1,
        InventoryContainer::CraftingTable => 2,
        InventoryContainer::Furnace => 3,
        InventoryContainer::Chest => 4,
    });
}

pub(crate) fn decode_inventory_container(
    decoder: &mut Decoder<'_>,
) -> Result<InventoryContainer, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(InventoryContainer::Player),
        2 => Ok(InventoryContainer::CraftingTable),
        3 => Ok(InventoryContainer::Furnace),
        4 => Ok(InventoryContainer::Chest),
        _ => Err(ProtocolCodecError::InvalidValue(
            "invalid inventory container",
        )),
    }
}

pub(crate) fn encode_inventory_slot(encoder: &mut Encoder, slot: InventorySlot) {
    match slot {
        InventorySlot::Auxiliary(index) => {
            encoder.write_u8(1);
            encoder.write_u8(index);
        }
        InventorySlot::Container(index) => {
            encoder.write_u8(2);
            encoder.write_u8(index);
        }
        InventorySlot::MainInventory(index) => {
            encoder.write_u8(3);
            encoder.write_u8(index);
        }
        InventorySlot::Hotbar(index) => {
            encoder.write_u8(4);
            encoder.write_u8(index);
        }
        InventorySlot::Offhand => encoder.write_u8(5),
    }
}

pub(crate) fn decode_inventory_slot(
    decoder: &mut Decoder<'_>,
) -> Result<InventorySlot, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(InventorySlot::Auxiliary(decoder.read_u8()?)),
        2 => Ok(InventorySlot::Container(decoder.read_u8()?)),
        3 => Ok(InventorySlot::MainInventory(decoder.read_u8()?)),
        4 => Ok(InventorySlot::Hotbar(decoder.read_u8()?)),
        5 => Ok(InventorySlot::Offhand),
        _ => Err(ProtocolCodecError::InvalidValue("invalid inventory slot")),
    }
}

pub(crate) fn encode_inventory_click_button(encoder: &mut Encoder, button: InventoryClickButton) {
    encoder.write_u8(match button {
        InventoryClickButton::Left => 1,
        InventoryClickButton::Right => 2,
    });
}

pub(crate) fn decode_inventory_click_button(
    decoder: &mut Decoder<'_>,
) -> Result<InventoryClickButton, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(InventoryClickButton::Left),
        2 => Ok(InventoryClickButton::Right),
        _ => Err(ProtocolCodecError::InvalidValue(
            "invalid inventory click button",
        )),
    }
}

pub(crate) fn encode_inventory_click_target(encoder: &mut Encoder, target: InventoryClickTarget) {
    match target {
        InventoryClickTarget::Slot(slot) => {
            encoder.write_u8(1);
            encode_inventory_slot(encoder, slot);
        }
        InventoryClickTarget::Outside => encoder.write_u8(2),
        InventoryClickTarget::Unsupported => encoder.write_u8(3),
    }
}

pub(crate) fn decode_inventory_click_target(
    decoder: &mut Decoder<'_>,
) -> Result<InventoryClickTarget, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(InventoryClickTarget::Slot(decode_inventory_slot(decoder)?)),
        2 => Ok(InventoryClickTarget::Outside),
        3 => Ok(InventoryClickTarget::Unsupported),
        _ => Err(ProtocolCodecError::InvalidValue(
            "invalid inventory click target",
        )),
    }
}

pub(crate) fn encode_inventory_click_validation(
    encoder: &mut Encoder,
    validation: &InventoryClickValidation,
) -> Result<(), ProtocolCodecError> {
    match validation {
        InventoryClickValidation::StrictSlotEcho { clicked_item } => {
            encoder.write_u8(1);
            encode_option(encoder, clicked_item.as_ref(), encode_item_stack)
        }
        InventoryClickValidation::Authoritative => {
            encoder.write_u8(2);
            Ok(())
        }
    }
}

pub(crate) fn decode_inventory_click_validation(
    decoder: &mut Decoder<'_>,
) -> Result<InventoryClickValidation, ProtocolCodecError> {
    match decoder.read_u8()? {
        1 => Ok(InventoryClickValidation::StrictSlotEcho {
            clicked_item: decode_option(decoder, decode_item_stack)?,
        }),
        2 => Ok(InventoryClickValidation::Authoritative),
        _ => Err(ProtocolCodecError::InvalidValue(
            "invalid inventory click validation",
        )),
    }
}

pub(crate) fn encode_inventory_transaction_context(
    encoder: &mut Encoder,
    transaction: InventoryTransactionContext,
) {
    encoder.write_u8(transaction.window_id);
    encoder.write_i16(transaction.action_number);
}

pub(crate) fn decode_inventory_transaction_context(
    decoder: &mut Decoder<'_>,
) -> Result<InventoryTransactionContext, ProtocolCodecError> {
    Ok(InventoryTransactionContext {
        window_id: decoder.read_u8()?,
        action_number: decoder.read_i16()?,
    })
}

pub(crate) fn encode_item_stack(
    encoder: &mut Encoder,
    stack: &ItemStack,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(stack.key.as_str())?;
    encoder.write_u8(stack.count);
    encoder.write_u16(stack.damage);
    Ok(())
}

pub(crate) fn decode_item_stack(
    decoder: &mut Decoder<'_>,
) -> Result<ItemStack, ProtocolCodecError> {
    Ok(ItemStack::new(
        decoder.read_string()?,
        decoder.read_u8()?,
        decoder.read_u16()?,
    ))
}

pub(crate) fn encode_player_inventory(
    encoder: &mut Encoder,
    inventory: &PlayerInventory,
) -> Result<(), ProtocolCodecError> {
    encoder.write_len(inventory.slots.len())?;
    for stack in &inventory.slots {
        encode_option(encoder, stack.as_ref(), encode_item_stack)?;
    }
    encode_option(encoder, inventory.offhand.as_ref(), encode_item_stack)
}

pub(crate) fn decode_player_inventory(
    decoder: &mut Decoder<'_>,
) -> Result<PlayerInventory, ProtocolCodecError> {
    let len = decoder.read_len()?;
    let mut slots = Vec::with_capacity(len);
    for _ in 0..len {
        slots.push(decode_option(decoder, decode_item_stack)?);
    }
    Ok(PlayerInventory {
        slots,
        offhand: decode_option(decoder, decode_item_stack)?,
    })
}

pub(crate) fn encode_inventory_window_contents(
    encoder: &mut Encoder,
    contents: &InventoryWindowContents,
) -> Result<(), ProtocolCodecError> {
    encode_player_inventory(encoder, &contents.player_inventory)?;
    encoder.write_len(contents.container_slots.len())?;
    for stack in &contents.container_slots {
        encode_option(encoder, stack.as_ref(), encode_item_stack)?;
    }
    Ok(())
}

pub(crate) fn decode_inventory_window_contents(
    decoder: &mut Decoder<'_>,
) -> Result<InventoryWindowContents, ProtocolCodecError> {
    let player_inventory = decode_player_inventory(decoder)?;
    let len = decoder.read_len()?;
    let mut container_slots = Vec::with_capacity(len);
    for _ in 0..len {
        container_slots.push(decode_option(decoder, decode_item_stack)?);
    }
    Ok(InventoryWindowContents {
        player_inventory,
        container_slots,
    })
}
