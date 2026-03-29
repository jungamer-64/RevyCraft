use crate::codec::__internal::binary::{Decoder, Encoder, ProtocolCodecError};
use crate::codec::__internal::shared::{decode_option, encode_option};
use revy_voxel_model::{
    InventoryClickButton, InventoryClickTarget, InventoryClickValidation, InventorySlot,
    InventoryTransactionContext, InventoryWindowContents, ItemDataMap, ItemDataValue, ItemStack,
    OpaqueF32, OpaqueF64, PlayerInventory,
};
use revy_voxel_rules::ContainerKindId;

const ITEM_DATA_BYTE: u8 = 1;
const ITEM_DATA_SHORT: u8 = 2;
const ITEM_DATA_INT: u8 = 3;
const ITEM_DATA_LONG: u8 = 4;
const ITEM_DATA_FLOAT: u8 = 5;
const ITEM_DATA_DOUBLE: u8 = 6;
const ITEM_DATA_BYTE_ARRAY: u8 = 7;
const ITEM_DATA_STRING: u8 = 8;
const ITEM_DATA_LIST: u8 = 9;
const ITEM_DATA_COMPOUND: u8 = 10;
const ITEM_DATA_INT_ARRAY: u8 = 11;
const ITEM_DATA_LONG_ARRAY: u8 = 12;

pub(crate) fn encode_inventory_container(
    encoder: &mut Encoder,
    container: &ContainerKindId,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(container.as_str())
}

pub(crate) fn decode_inventory_container(
    decoder: &mut Decoder<'_>,
) -> Result<ContainerKindId, ProtocolCodecError> {
    Ok(ContainerKindId::new(decoder.read_string()?))
}

pub(crate) fn encode_inventory_slot(encoder: &mut Encoder, slot: InventorySlot) {
    match slot {
        InventorySlot::WindowLocal(index) => {
            encoder.write_u8(1);
            encoder.write_u8(
                u8::try_from(index).expect("window-local inventory slot should fit into u8"),
            );
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
        1 | 2 => Ok(InventorySlot::WindowLocal(u16::from(decoder.read_u8()?))),
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
    encode_item_data_map(encoder, &stack.components)?;
    encode_item_data_map(encoder, &stack.extra)?;
    Ok(())
}

pub(crate) fn decode_item_stack(
    decoder: &mut Decoder<'_>,
) -> Result<ItemStack, ProtocolCodecError> {
    let mut stack = ItemStack::new(
        decoder.read_string()?,
        decoder.read_u8()?,
        decoder.read_u16()?,
    );
    stack.components = decode_item_data_map(decoder)?;
    stack.extra = decode_item_data_map(decoder)?;
    Ok(stack)
}

fn encode_item_data_map(
    encoder: &mut Encoder,
    data: &ItemDataMap,
) -> Result<(), ProtocolCodecError> {
    encoder.write_len(data.as_map().len())?;
    for (key, value) in data.iter() {
        encoder.write_string(key)?;
        encode_item_data_value(encoder, value)?;
    }
    Ok(())
}

fn decode_item_data_map(decoder: &mut Decoder<'_>) -> Result<ItemDataMap, ProtocolCodecError> {
    let len = decoder.read_len()?;
    let mut data = ItemDataMap::new();
    for _ in 0..len {
        let key = decoder.read_string()?;
        let value = decode_item_data_value(decoder)?;
        let _ = data.insert(key, value);
    }
    Ok(data)
}

fn encode_item_data_value(
    encoder: &mut Encoder,
    value: &ItemDataValue,
) -> Result<(), ProtocolCodecError> {
    match value {
        ItemDataValue::Byte(value) => {
            encoder.write_u8(ITEM_DATA_BYTE);
            encoder.write_i8(*value);
        }
        ItemDataValue::Short(value) => {
            encoder.write_u8(ITEM_DATA_SHORT);
            encoder.write_i16(*value);
        }
        ItemDataValue::Int(value) => {
            encoder.write_u8(ITEM_DATA_INT);
            encoder.write_i32(*value);
        }
        ItemDataValue::Long(value) => {
            encoder.write_u8(ITEM_DATA_LONG);
            encoder.write_i64(*value);
        }
        ItemDataValue::Float(value) => {
            encoder.write_u8(ITEM_DATA_FLOAT);
            encoder.write_u32(u32::from_le_bytes(value.into_f32().to_le_bytes()));
        }
        ItemDataValue::Double(value) => {
            encoder.write_u8(ITEM_DATA_DOUBLE);
            encoder.write_u64(u64::from_le_bytes(value.into_f64().to_le_bytes()));
        }
        ItemDataValue::ByteArray(values) => {
            encoder.write_u8(ITEM_DATA_BYTE_ARRAY);
            encoder.write_bytes(values)?;
        }
        ItemDataValue::String(value) => {
            encoder.write_u8(ITEM_DATA_STRING);
            encoder.write_string(value)?;
        }
        ItemDataValue::List(values) => {
            encoder.write_u8(ITEM_DATA_LIST);
            encoder.write_len(values.len())?;
            for value in values {
                encode_item_data_value(encoder, value)?;
            }
        }
        ItemDataValue::Compound(values) => {
            encoder.write_u8(ITEM_DATA_COMPOUND);
            encoder.write_len(values.len())?;
            for (key, value) in values {
                encoder.write_string(key)?;
                encode_item_data_value(encoder, value)?;
            }
        }
        ItemDataValue::IntArray(values) => {
            encoder.write_u8(ITEM_DATA_INT_ARRAY);
            encoder.write_len(values.len())?;
            for value in values {
                encoder.write_i32(*value);
            }
        }
        ItemDataValue::LongArray(values) => {
            encoder.write_u8(ITEM_DATA_LONG_ARRAY);
            encoder.write_len(values.len())?;
            for value in values {
                encoder.write_i64(*value);
            }
        }
    }
    Ok(())
}

fn decode_item_data_value(decoder: &mut Decoder<'_>) -> Result<ItemDataValue, ProtocolCodecError> {
    match decoder.read_u8()? {
        ITEM_DATA_BYTE => Ok(ItemDataValue::Byte(decoder.read_i8()?)),
        ITEM_DATA_SHORT => Ok(ItemDataValue::Short(decoder.read_i16()?)),
        ITEM_DATA_INT => Ok(ItemDataValue::Int(decoder.read_i32()?)),
        ITEM_DATA_LONG => Ok(ItemDataValue::Long(decoder.read_i64()?)),
        ITEM_DATA_FLOAT => Ok(ItemDataValue::Float(OpaqueF32::from_f32(
            f32::from_le_bytes(decoder.read_u32()?.to_le_bytes()),
        ))),
        ITEM_DATA_DOUBLE => Ok(ItemDataValue::Double(OpaqueF64::from_f64(
            f64::from_le_bytes(decoder.read_u64()?.to_le_bytes()),
        ))),
        ITEM_DATA_BYTE_ARRAY => Ok(ItemDataValue::ByteArray(decoder.read_bytes()?)),
        ITEM_DATA_STRING => Ok(ItemDataValue::String(decoder.read_string()?)),
        ITEM_DATA_LIST => {
            let len = decoder.read_len()?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(decode_item_data_value(decoder)?);
            }
            Ok(ItemDataValue::List(values))
        }
        ITEM_DATA_COMPOUND => {
            let len = decoder.read_len()?;
            let mut values = std::collections::BTreeMap::new();
            for _ in 0..len {
                let key = decoder.read_string()?;
                let value = decode_item_data_value(decoder)?;
                values.insert(key, value);
            }
            Ok(ItemDataValue::Compound(values))
        }
        ITEM_DATA_INT_ARRAY => {
            let len = decoder.read_len()?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(decoder.read_i32()?);
            }
            Ok(ItemDataValue::IntArray(values))
        }
        ITEM_DATA_LONG_ARRAY => {
            let len = decoder.read_len()?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(decoder.read_i64()?);
            }
            Ok(ItemDataValue::LongArray(values))
        }
        _ => Err(ProtocolCodecError::InvalidValue("invalid item data tag")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_stack_codec_round_trips_components_and_extra_data() {
        let mut stack = ItemStack::new("minecraft:stone", 12, 0);
        let mut components = ItemDataMap::new();
        let _ = components.insert(
            "minecraft:custom_name",
            ItemDataValue::String("{\"text\":\"Stone\"}".to_string()),
        );
        let mut extra = ItemDataMap::new();
        let _ = extra.insert("foo", ItemDataValue::Long(42));
        stack.components = components;
        stack.extra = extra;

        let mut encoder = Encoder::default();
        encode_item_stack(&mut encoder, &stack).expect("item stack should encode");

        let encoded = encoder.into_inner();
        let decoded =
            decode_item_stack(&mut Decoder::new(&encoded)).expect("item stack should decode");
        assert_eq!(decoded, stack);
    }
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
    encoder.write_len(contents.local_slots.len())?;
    for stack in &contents.local_slots {
        encode_option(encoder, stack.as_ref(), encode_item_stack)?;
    }
    Ok(())
}

pub(crate) fn decode_inventory_window_contents(
    decoder: &mut Decoder<'_>,
) -> Result<InventoryWindowContents, ProtocolCodecError> {
    let player_inventory = decode_player_inventory(decoder)?;
    let len = decoder.read_len()?;
    let mut local_slots = Vec::with_capacity(len);
    for _ in 0..len {
        local_slots.push(decode_option(decoder, decode_item_stack)?);
    }
    Ok(InventoryWindowContents::with_local_slots(
        player_inventory,
        local_slots,
    ))
}
