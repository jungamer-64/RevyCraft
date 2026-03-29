use crate::__version_support::blocks::{
    flattened_item_id_1_13_2, legacy_item, semantic_flattened_item_1_13_2, semantic_item,
};
use mc_proto_common::{PacketReader, PacketWriter, ProtocolError};
use revy_voxel_model::ItemStack;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotNbtEncoding {
    LengthPrefixedBlob,
    RootTag,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotEncoding {
    Legacy { nbt: SlotNbtEncoding },
    PresentVarInt { nbt: SlotNbtEncoding },
}

pub fn read_slot(
    reader: &mut PacketReader<'_>,
    slot: SlotEncoding,
) -> Result<Option<ItemStack>, ProtocolError> {
    match slot {
        SlotEncoding::Legacy { nbt } => {
            let item_id = reader.read_i16()?;
            if item_id < 0 {
                return Ok(None);
            }
            let count = reader.read_u8()?;
            let damage = u16::from_be_bytes(reader.read_i16()?.to_be_bytes());
            skip_slot_nbt(reader, nbt)?;
            Ok(Some(semantic_item(item_id, damage, count)))
        }
        SlotEncoding::PresentVarInt { nbt } => {
            if !reader.read_bool()? {
                return Ok(None);
            }
            let item_id = reader.read_varint()?;
            let count = reader.read_u8()?;
            skip_slot_nbt(reader, nbt)?;
            Ok(Some(semantic_flattened_item_1_13_2(item_id, count)))
        }
    }
}

pub fn write_slot(
    writer: &mut PacketWriter,
    stack: Option<&ItemStack>,
    slot: SlotEncoding,
) -> Result<(), ProtocolError> {
    match slot {
        SlotEncoding::Legacy { nbt } => {
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
            write_empty_slot_nbt(writer, nbt);
        }
        SlotEncoding::PresentVarInt { nbt } => {
            let Some(stack) = stack else {
                writer.write_bool(false);
                return Ok(());
            };
            let Some(item_id) = flattened_item_id_1_13_2(stack) else {
                return Err(ProtocolError::InvalidPacket("unsupported inventory item"));
            };
            writer.write_bool(true);
            writer.write_varint(item_id);
            writer.write_u8(stack.count);
            write_empty_slot_nbt(writer, nbt);
        }
    }
    Ok(())
}

fn write_empty_slot_nbt(writer: &mut PacketWriter, slot_nbt: SlotNbtEncoding) {
    match slot_nbt {
        SlotNbtEncoding::LengthPrefixedBlob => writer.write_i16(-1),
        SlotNbtEncoding::RootTag => writer.write_u8(0),
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
