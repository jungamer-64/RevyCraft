use thiserror::Error;

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
    #[error("plugin error: {0}")]
    Plugin(String),
}
