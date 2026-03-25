use crate::__version_support::inventory::{SlotNbtEncoding, write_slot};
use mc_core::ItemStack;
use mc_proto_common::{PacketWriter, ProtocolError};

pub fn write_empty_metadata_1_8(writer: &mut PacketWriter) {
    writer.write_u8(0x7f);
}

pub fn write_empty_metadata_1_12(writer: &mut PacketWriter) {
    writer.write_u8(0xff);
}

pub fn write_item_stack_metadata_1_8(
    writer: &mut PacketWriter,
    index: u8,
    stack: &ItemStack,
    slot_nbt: SlotNbtEncoding,
) -> Result<(), ProtocolError> {
    writer.write_u8((5_u8 << 5) | (index & 0x1f));
    write_slot(writer, Some(stack), slot_nbt)?;
    write_empty_metadata_1_8(writer);
    Ok(())
}

pub fn write_item_stack_metadata_1_12(
    writer: &mut PacketWriter,
    index: u8,
    stack: &ItemStack,
    slot_nbt: SlotNbtEncoding,
) -> Result<(), ProtocolError> {
    writer.write_u8(index);
    writer.write_varint(5);
    write_slot(writer, Some(stack), slot_nbt)?;
    write_empty_metadata_1_12(writer);
    Ok(())
}
