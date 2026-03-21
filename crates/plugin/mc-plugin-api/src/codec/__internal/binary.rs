use crate::abi::{CURRENT_PLUGIN_ABI, PluginAbiVersion, PluginKind};
use thiserror::Error;

pub(crate) const PROTOCOL_FLAG_RESPONSE: u16 = 0x0001;
pub(crate) const PLUGIN_ENVELOPE_HEADER_LEN: usize = 12;

#[derive(Debug, Error)]
pub enum ProtocolCodecError {
    #[error("unexpected end of plugin payload")]
    UnexpectedEof,
    #[error("invalid utf-8 in plugin payload")]
    InvalidUtf8,
    #[error("plugin envelope length overflow")]
    LengthOverflow,
    #[error("invalid plugin envelope: {0}")]
    InvalidEnvelope(&'static str),
    #[error("invalid plugin kind {0}")]
    InvalidPluginKind(u8),
    #[error("invalid protocol op code {0}")]
    InvalidProtocolOpCode(u8),
    #[error("invalid plugin payload value: {0}")]
    InvalidValue(&'static str),
    #[error("plugin payload had trailing bytes")]
    TrailingBytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EnvelopeHeader {
    pub abi: PluginAbiVersion,
    pub plugin_kind: PluginKind,
    pub op_code: u8,
    pub flags: u16,
    pub payload_len: u32,
}

#[derive(Default)]
pub(crate) struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    pub fn with_header(header: EnvelopeHeader) -> Self {
        let mut encoder = Self::default();
        encoder.write_u16(header.abi.major);
        encoder.write_u16(header.abi.minor);
        encoder.write_u8(match header.plugin_kind {
            PluginKind::Protocol => 1,
            PluginKind::Storage => 2,
            PluginKind::Auth => 3,
            PluginKind::Gameplay => 4,
        });
        encoder.write_u8(header.op_code);
        encoder.write_u16(header.flags);
        encoder.write_u32(header.payload_len);
        encoder
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.bytes
    }

    pub fn write_u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub fn write_i8(&mut self, value: i8) {
        self.bytes.push(value.to_le_bytes()[0]);
    }

    pub fn write_bool(&mut self, value: bool) {
        self.write_u8(u8::from(value));
    }

    pub fn write_u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_i16(&mut self, value: i16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_f32(&mut self, value: f32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_f64(&mut self, value: f64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_len(&mut self, value: usize) -> Result<(), ProtocolCodecError> {
        let value = u32::try_from(value).map_err(|_| ProtocolCodecError::LengthOverflow)?;
        self.write_u32(value);
        Ok(())
    }

    pub fn write_string(&mut self, value: &str) -> Result<(), ProtocolCodecError> {
        self.write_bytes(value.as_bytes())
    }

    pub fn write_bytes(&mut self, value: &[u8]) -> Result<(), ProtocolCodecError> {
        self.write_len(value.len())?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    pub fn write_raw(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }
}

pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Decoder<'a> {
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    pub const fn finish(&self) -> Result<(), ProtocolCodecError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(ProtocolCodecError::TrailingBytes)
        }
    }

    pub fn read_u8(&mut self) -> Result<u8, ProtocolCodecError> {
        let byte = *self
            .bytes
            .get(self.cursor)
            .ok_or(ProtocolCodecError::UnexpectedEof)?;
        self.cursor = self.cursor.saturating_add(1);
        Ok(byte)
    }

    pub fn read_i8(&mut self) -> Result<i8, ProtocolCodecError> {
        Ok(i8::from_le_bytes([self.read_u8()?]))
    }

    pub fn read_bool(&mut self) -> Result<bool, ProtocolCodecError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ProtocolCodecError::InvalidValue("invalid bool tag")),
        }
    }

    pub fn read_u16(&mut self) -> Result<u16, ProtocolCodecError> {
        Ok(u16::from_le_bytes(self.read_exact::<2>()?))
    }

    pub fn read_i16(&mut self) -> Result<i16, ProtocolCodecError> {
        Ok(i16::from_le_bytes(self.read_exact::<2>()?))
    }

    pub fn read_u32(&mut self) -> Result<u32, ProtocolCodecError> {
        Ok(u32::from_le_bytes(self.read_exact::<4>()?))
    }

    pub fn read_i32(&mut self) -> Result<i32, ProtocolCodecError> {
        Ok(i32::from_le_bytes(self.read_exact::<4>()?))
    }

    pub fn read_u64(&mut self) -> Result<u64, ProtocolCodecError> {
        Ok(u64::from_le_bytes(self.read_exact::<8>()?))
    }

    pub fn read_i64(&mut self) -> Result<i64, ProtocolCodecError> {
        Ok(i64::from_le_bytes(self.read_exact::<8>()?))
    }

    pub fn read_f32(&mut self) -> Result<f32, ProtocolCodecError> {
        Ok(f32::from_le_bytes(self.read_exact::<4>()?))
    }

    pub fn read_f64(&mut self) -> Result<f64, ProtocolCodecError> {
        Ok(f64::from_le_bytes(self.read_exact::<8>()?))
    }

    pub fn read_len(&mut self) -> Result<usize, ProtocolCodecError> {
        usize::try_from(self.read_u32()?).map_err(|_| ProtocolCodecError::LengthOverflow)
    }

    pub fn read_string(&mut self) -> Result<String, ProtocolCodecError> {
        let bytes = self.read_bytes()?;
        String::from_utf8(bytes).map_err(|_| ProtocolCodecError::InvalidUtf8)
    }

    pub fn read_bytes(&mut self) -> Result<Vec<u8>, ProtocolCodecError> {
        let len = self.read_len()?;
        let bytes = self.read_raw(len)?;
        Ok(bytes.to_vec())
    }

    pub fn read_raw(&mut self, len: usize) -> Result<&'a [u8], ProtocolCodecError> {
        let end = self.cursor.saturating_add(len);
        let slice = self
            .bytes
            .get(self.cursor..end)
            .ok_or(ProtocolCodecError::UnexpectedEof)?;
        self.cursor = end;
        Ok(slice)
    }

    pub fn read_exact<const N: usize>(&mut self) -> Result<[u8; N], ProtocolCodecError> {
        let bytes = self.read_raw(N)?;
        let mut array = [0_u8; N];
        array.copy_from_slice(bytes);
        Ok(array)
    }
}

pub(crate) fn encode_envelope(
    header: EnvelopeHeader,
    payload: &[u8],
) -> Result<Vec<u8>, ProtocolCodecError> {
    if usize::try_from(header.payload_len).map_err(|_| ProtocolCodecError::LengthOverflow)?
        != payload.len()
    {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "payload length did not match header",
        ));
    }
    let mut encoder = Encoder::with_header(header);
    encoder.write_raw(payload);
    Ok(encoder.into_inner())
}

pub(crate) fn decode_envelope(bytes: &[u8]) -> Result<(EnvelopeHeader, &[u8]), ProtocolCodecError> {
    if bytes.len() < PLUGIN_ENVELOPE_HEADER_LEN {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "message shorter than header",
        ));
    }
    let mut decoder = Decoder::new(bytes);
    let abi = PluginAbiVersion {
        major: decoder.read_u16()?,
        minor: decoder.read_u16()?,
    };
    let plugin_kind = PluginKind::try_from(decoder.read_u8()?)?;
    let op_code = decoder.read_u8()?;
    let flags = decoder.read_u16()?;
    let payload_len = decoder.read_u32()?;
    if abi != CURRENT_PLUGIN_ABI {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "plugin ABI version mismatch",
        ));
    }
    let payload_len_usize =
        usize::try_from(payload_len).map_err(|_| ProtocolCodecError::LengthOverflow)?;
    if bytes.len() != PLUGIN_ENVELOPE_HEADER_LEN.saturating_add(payload_len_usize) {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "payload length did not match message size",
        ));
    }
    Ok((
        EnvelopeHeader {
            abi,
            plugin_kind,
            op_code,
            flags,
            payload_len,
        },
        &bytes[PLUGIN_ENVELOPE_HEADER_LEN..],
    ))
}
