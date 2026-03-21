use crate::errors::ProtocolError;
use crate::packet::{PacketWriter, peek_varint};
use crate::traits::WireCodec;
use bytes::BytesMut;

#[derive(Default)]
pub struct MinecraftWireCodec;

impl WireCodec for MinecraftWireCodec {
    fn encode_frame(&self, payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = PacketWriter::default();
        writer.write_varint(
            i32::try_from(payload.len())
                .map_err(|_| ProtocolError::InvalidPacket("frame too large"))?,
        );
        writer.write_bytes(payload);
        Ok(writer.into_inner())
    }

    fn try_decode_frame(&self, buffer: &mut BytesMut) -> Result<Option<Vec<u8>>, ProtocolError> {
        let Some((length, header_len)) = peek_varint(buffer) else {
            return Ok(None);
        };
        let payload_length = usize::try_from(length)
            .map_err(|_| ProtocolError::InvalidPacket("negative frame length"))?;
        if buffer.len() < header_len + payload_length {
            return Ok(None);
        }
        let frame = buffer.split_to(header_len + payload_length);
        Ok(Some(frame[header_len..].to_vec()))
    }
}

#[derive(Default)]
pub struct RawPacketStreamWireCodec;

impl WireCodec for RawPacketStreamWireCodec {
    fn encode_frame(&self, payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
        Ok(payload.to_vec())
    }

    fn try_decode_frame(&self, buffer: &mut BytesMut) -> Result<Option<Vec<u8>>, ProtocolError> {
        if buffer.is_empty() {
            Ok(None)
        } else {
            Ok(Some(buffer.split_to(buffer.len()).to_vec()))
        }
    }
}
