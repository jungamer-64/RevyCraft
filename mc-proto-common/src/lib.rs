#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::missing_errors_doc,
    clippy::module_name_repetitions
)]

use bytes::BytesMut;
use mc_core::{
    ConnectionId, CoreCommand, CoreEvent, EntityId, PlayerId, ProtocolVersion, WorldSnapshot,
};
use std::path::Path;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionPhase {
    Handshaking,
    Status,
    Login,
    Play,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeNextState {
    Status,
    Login,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandshakeIntent {
    pub protocol_version: ProtocolVersion,
    pub server_host: String,
    pub server_port: u16,
    pub next_state: HandshakeNextState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusRequest {
    Query,
    Ping { payload: i64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginRequest {
    LoginStart { username: String },
    EncryptionResponse,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServerListStatus {
    pub version_name: String,
    pub protocol: ProtocolVersion,
    pub players_online: usize,
    pub max_players: usize,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionEncodingContext {
    pub connection_id: ConnectionId,
    pub phase: ConnectionPhase,
    pub player_id: Option<PlayerId>,
    pub entity_id: Option<EntityId>,
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("unexpected end of packet")]
    UnexpectedEof,
    #[error("invalid varint encoding")]
    InvalidVarInt,
    #[error("invalid utf-8 string")]
    InvalidUtf8,
    #[error("string too long: {0}")]
    StringTooLong(usize),
    #[error("invalid packet: {0}")]
    InvalidPacket(&'static str),
    #[error("unsupported packet id 0x{0:02x}")]
    UnsupportedPacket(i32),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid data: {0}")]
    InvalidData(String),
}

pub trait WireCodec: Send + Sync {
    fn encode_frame(&self, payload: &[u8]) -> Result<Vec<u8>, ProtocolError>;
    fn try_decode_frame(&self, buffer: &mut BytesMut) -> Result<Option<Vec<u8>>, ProtocolError>;
}

pub trait StorageAdapter: Send + Sync {
    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError>;
    fn save_snapshot(&self, world_dir: &Path, snapshot: &WorldSnapshot)
    -> Result<(), StorageError>;
}

pub trait ProtocolAdapter: Send + Sync {
    fn protocol_version(&self) -> ProtocolVersion;
    fn version_name(&self) -> &'static str;
    fn wire_codec(&self) -> &dyn WireCodec;
    fn storage_adapter(&self) -> &dyn StorageAdapter;
    fn decode_handshake(&self, frame: &[u8]) -> Result<HandshakeIntent, ProtocolError>;
    fn decode_status(&self, frame: &[u8]) -> Result<StatusRequest, ProtocolError>;
    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError>;
    fn decode_play(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError>;
    fn encode_status_response(&self, status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError>;
    fn encode_status_pong(&self, payload: i64) -> Result<Vec<u8>, ProtocolError>;
    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_event(
        &self,
        event: &CoreEvent,
        context: &SessionEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
}

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

#[derive(Default, Debug)]
pub struct PacketWriter {
    buffer: Vec<u8>,
}

impl PacketWriter {
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.buffer
    }

    pub fn write_u8(&mut self, value: u8) {
        self.buffer.push(value);
    }

    pub fn write_i8(&mut self, value: i8) {
        self.buffer.push(value.to_be_bytes()[0]);
    }

    pub fn write_bool(&mut self, value: bool) {
        self.write_u8(u8::from(value));
    }

    pub fn write_i16(&mut self, value: i16) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    pub fn write_u16(&mut self, value: u16) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    pub fn write_i32(&mut self, value: i32) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    pub fn write_i64(&mut self, value: i64) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    pub fn write_f32(&mut self, value: f32) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    pub fn write_f64(&mut self, value: f64) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    pub fn write_varint(&mut self, value: i32) {
        let mut value = u32::from_ne_bytes(value.to_ne_bytes());
        loop {
            if value & !0x7f == 0 {
                self.write_u8(u8::try_from(value).expect("single varint byte should fit"));
                break;
            }
            let lower = u8::try_from(value & 0x7f).expect("varint lower bits should fit");
            self.write_u8(lower | 0x80);
            value >>= 7;
        }
    }

    pub fn write_string(&mut self, value: &str) -> Result<(), ProtocolError> {
        let bytes = value.as_bytes();
        let length =
            i32::try_from(bytes.len()).map_err(|_| ProtocolError::StringTooLong(bytes.len()))?;
        self.write_varint(length);
        self.write_bytes(bytes);
        Ok(())
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }
}

pub struct PacketReader<'a> {
    payload: &'a [u8],
    cursor: usize,
}

impl<'a> PacketReader<'a> {
    #[must_use]
    pub const fn new(payload: &'a [u8]) -> Self {
        Self { payload, cursor: 0 }
    }

    pub fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        let byte = *self
            .payload
            .get(self.cursor)
            .ok_or(ProtocolError::UnexpectedEof)?;
        self.cursor += 1;
        Ok(byte)
    }

    pub fn read_i8(&mut self) -> Result<i8, ProtocolError> {
        Ok(i8::from_be_bytes([self.read_u8()?]))
    }

    pub fn read_bool(&mut self) -> Result<bool, ProtocolError> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_i16(&mut self) -> Result<i16, ProtocolError> {
        Ok(i16::from_be_bytes(self.read_exact::<2>()?))
    }

    pub fn read_u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_be_bytes(self.read_exact::<2>()?))
    }

    pub fn read_i32(&mut self) -> Result<i32, ProtocolError> {
        Ok(i32::from_be_bytes(self.read_exact::<4>()?))
    }

    pub fn read_i64(&mut self) -> Result<i64, ProtocolError> {
        Ok(i64::from_be_bytes(self.read_exact::<8>()?))
    }

    pub fn read_f32(&mut self) -> Result<f32, ProtocolError> {
        Ok(f32::from_be_bytes(self.read_exact::<4>()?))
    }

    pub fn read_f64(&mut self) -> Result<f64, ProtocolError> {
        Ok(f64::from_be_bytes(self.read_exact::<8>()?))
    }

    pub fn read_varint(&mut self) -> Result<i32, ProtocolError> {
        let mut num_read = 0;
        let mut result = 0_u32;
        loop {
            let byte = self.read_u8()?;
            let value = u32::from(byte & 0x7f);
            result |= value << (7 * num_read);
            num_read += 1;
            if num_read > 5 {
                return Err(ProtocolError::InvalidVarInt);
            }
            if byte & 0x80 == 0 {
                break;
            }
        }
        Ok(i32::from_ne_bytes(result.to_ne_bytes()))
    }

    pub fn read_string(&mut self, max_len: usize) -> Result<String, ProtocolError> {
        let length = usize::try_from(self.read_varint()?)
            .map_err(|_| ProtocolError::InvalidPacket("negative string length"))?;
        if length > max_len.saturating_mul(4) {
            return Err(ProtocolError::StringTooLong(length));
        }
        let bytes = self.read_bytes(length)?;
        let value = std::str::from_utf8(bytes).map_err(|_| ProtocolError::InvalidUtf8)?;
        if value.chars().count() > max_len {
            return Err(ProtocolError::StringTooLong(value.len()));
        }
        Ok(value.to_string())
    }

    pub fn read_bytes(&mut self, length: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self.cursor.saturating_add(length);
        let slice = self
            .payload
            .get(self.cursor..end)
            .ok_or(ProtocolError::UnexpectedEof)?;
        self.cursor = end;
        Ok(slice)
    }

    pub fn is_exhausted(&self) -> bool {
        self.cursor == self.payload.len()
    }

    fn read_exact<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let bytes = self.read_bytes(N)?;
        let mut array = [0_u8; N];
        array.copy_from_slice(bytes);
        Ok(array)
    }
}

fn peek_varint(buffer: &BytesMut) -> Option<(i32, usize)> {
    let mut num_read = 0;
    let mut result = 0_u32;
    for byte in buffer.iter().copied() {
        let value = u32::from(byte & 0x7f);
        result |= value << (7 * num_read);
        num_read += 1;
        if num_read > 5 {
            return None;
        }
        if byte & 0x80 == 0 {
            return Some((i32::from_ne_bytes(result.to_ne_bytes()), num_read));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{MinecraftWireCodec, PacketReader, PacketWriter, WireCodec};
    use bytes::BytesMut;

    #[test]
    fn wire_codec_round_trip_frame() {
        let codec = MinecraftWireCodec;
        let payload = vec![0x01, 0x02, 0x03];
        let frame = codec.encode_frame(&payload).expect("frame should encode");
        let mut buffer = BytesMut::from(frame.as_slice());
        let decoded = codec
            .try_decode_frame(&mut buffer)
            .expect("frame should decode")
            .expect("complete frame should be present");
        assert_eq!(decoded, payload);
        assert!(buffer.is_empty());
    }

    #[test]
    fn packet_primitives_round_trip() {
        let mut writer = PacketWriter::default();
        writer.write_varint(0x2a);
        writer.write_string("hello").expect("string should encode");
        writer.write_f64(12.5);
        let bytes = writer.into_inner();

        let mut reader = PacketReader::new(&bytes);
        assert_eq!(reader.read_varint().expect("varint should decode"), 0x2a);
        assert_eq!(
            reader.read_string(16).expect("string should decode"),
            "hello"
        );
        assert_eq!(reader.read_f64().expect("double should decode"), 12.5);
        assert!(reader.is_exhausted());
    }
}
