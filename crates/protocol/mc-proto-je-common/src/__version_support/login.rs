use crate::login::{read_login_byte_array as read_impl, write_login_byte_array as write_impl};
use mc_proto_common::{PacketReader, PacketWriter, ProtocolError};

pub fn read_login_byte_array(reader: &mut PacketReader<'_>) -> Result<Vec<u8>, ProtocolError> {
    read_impl(reader)
}

pub fn write_login_byte_array(
    writer: &mut PacketWriter,
    bytes: &[u8],
) -> Result<(), ProtocolError> {
    write_impl(writer, bytes)
}
