use mc_proto_common::{PacketWriter, ProtocolError, ServerListStatus};
use serde_json::json;

#[must_use]
pub fn format_text_component(text: &str) -> String {
    json!({ "text": text }).to_string()
}

pub(crate) fn encode_status_response_packet(
    status: &ServerListStatus,
) -> Result<Vec<u8>, ProtocolError> {
    let payload = json!({
        "version": {
            "name": status.version.version_name,
            "protocol": status.version.protocol_number,
        },
        "players": {
            "max": status.max_players,
            "online": status.players_online,
            "sample": [],
        },
        "description": {
            "text": status.description,
        }
    });
    let mut writer = PacketWriter::default();
    writer.write_varint(0x00);
    writer.write_string(&payload.to_string())?;
    Ok(writer.into_inner())
}

pub(crate) fn encode_status_pong_packet(payload: i64) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0x01);
    writer.write_i64(payload);
    writer.into_inner()
}
