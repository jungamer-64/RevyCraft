use mc_proto_common::{PacketReader, PacketWriter, ProtocolError};
use revy_voxel_core::PlayerSnapshot;

pub(crate) fn write_login_byte_array(
    writer: &mut PacketWriter,
    bytes: &[u8],
) -> Result<(), ProtocolError> {
    writer.write_varint(
        i32::try_from(bytes.len())
            .map_err(|_| ProtocolError::InvalidPacket("login byte array too large"))?,
    );
    writer.write_bytes(bytes);
    Ok(())
}

pub(crate) fn read_login_byte_array(
    reader: &mut PacketReader<'_>,
) -> Result<Vec<u8>, ProtocolError> {
    let len = usize::try_from(reader.read_varint()?)
        .map_err(|_| ProtocolError::InvalidPacket("negative login byte array length"))?;
    Ok(reader.read_bytes(len)?.to_vec())
}

pub(crate) fn encode_login_success_packet(
    player: &PlayerSnapshot,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x02);
    writer.write_string(&player.id.0.hyphenated().to_string())?;
    writer.write_string(&player.username)?;
    Ok(writer.into_inner())
}
