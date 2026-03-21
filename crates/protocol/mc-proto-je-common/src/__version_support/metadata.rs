use mc_proto_common::PacketWriter;

pub fn write_empty_metadata_1_8(writer: &mut PacketWriter) {
    writer.write_u8(0x7f);
}

pub fn write_empty_metadata_1_12(writer: &mut PacketWriter) {
    writer.write_u8(0xff);
}
