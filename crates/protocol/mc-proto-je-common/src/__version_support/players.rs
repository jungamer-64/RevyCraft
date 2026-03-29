use mc_proto_common::{PacketWriter, ProtocolError};
use revy_voxel_core::PlayerSnapshot;

pub fn encode_player_info_add(
    packet_id: i32,
    player: &PlayerSnapshot,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(packet_id);
    writer.write_varint(0);
    writer.write_varint(1);
    writer.write_bytes(player.id.0.as_bytes());
    writer.write_string(&player.username)?;
    writer.write_varint(0);
    writer.write_varint(0);
    writer.write_varint(0);
    writer.write_bool(false);
    Ok(writer.into_inner())
}
