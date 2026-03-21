use mc_proto_common::{Edition, HandshakeIntent, HandshakeNextState, PacketReader, ProtocolError};

const PACKET_HANDSHAKE: i32 = 0x00;

pub(crate) fn decode_handshake_frame(
    frame: &[u8],
) -> Result<Option<HandshakeIntent>, ProtocolError> {
    let mut reader = PacketReader::new(frame);
    let packet_id = reader.read_varint()?;
    if packet_id != PACKET_HANDSHAKE {
        return Ok(None);
    }
    let protocol_number = reader.read_varint()?;
    let server_host = reader.read_string(255)?;
    let server_port = reader.read_u16()?;
    let next_state = match reader.read_varint()? {
        1 => HandshakeNextState::Status,
        2 => HandshakeNextState::Login,
        _ => {
            return Err(ProtocolError::InvalidPacket(
                "unsupported handshake next state",
            ));
        }
    };
    Ok(Some(HandshakeIntent {
        edition: Edition::Je,
        protocol_number,
        server_host,
        server_port,
        next_state,
    }))
}
