use crate::errors::ProtocolError;
use bytes::BytesMut;

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

    /// # Panics
    ///
    /// Panics if the final single-byte branch of the varint encoder contains a
    /// value that does not fit into `u8`. The preceding bitmask check makes
    /// that unreachable for valid `u32` state.
    pub fn write_varint(&mut self, value: i32) {
        let mut value = u32::from_ne_bytes(value.to_ne_bytes());
        loop {
            if value & !0x7f == 0 {
                self.write_u8(u8::try_from(value).expect("single varint byte should fit"));
                break;
            }
            let lower = (value & 0x7f) as u8;
            self.write_u8(lower | 0x80);
            value >>= 7;
        }
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::StringTooLong`] when the UTF-8 byte length does
    /// not fit into a protocol string length prefix.
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

    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedEof`] when the payload ends before
    /// the next byte is available.
    pub fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        let byte = *self
            .payload
            .get(self.cursor)
            .ok_or(ProtocolError::UnexpectedEof)?;
        self.cursor += 1;
        Ok(byte)
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedEof`] when the payload ends before
    /// the next byte is available.
    pub fn read_i8(&mut self) -> Result<i8, ProtocolError> {
        Ok(i8::from_be_bytes([self.read_u8()?]))
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedEof`] when the payload ends before
    /// the boolean byte is available.
    pub fn read_bool(&mut self) -> Result<bool, ProtocolError> {
        Ok(self.read_u8()? != 0)
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedEof`] when fewer than two bytes
    /// remain in the payload.
    pub fn read_i16(&mut self) -> Result<i16, ProtocolError> {
        Ok(i16::from_be_bytes(self.read_exact::<2>()?))
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedEof`] when fewer than two bytes
    /// remain in the payload.
    pub fn read_u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_be_bytes(self.read_exact::<2>()?))
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedEof`] when fewer than four bytes
    /// remain in the payload.
    pub fn read_i32(&mut self) -> Result<i32, ProtocolError> {
        Ok(i32::from_be_bytes(self.read_exact::<4>()?))
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedEof`] when fewer than eight bytes
    /// remain in the payload.
    pub fn read_i64(&mut self) -> Result<i64, ProtocolError> {
        Ok(i64::from_be_bytes(self.read_exact::<8>()?))
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedEof`] when fewer than four bytes
    /// remain in the payload.
    pub fn read_f32(&mut self) -> Result<f32, ProtocolError> {
        Ok(f32::from_be_bytes(self.read_exact::<4>()?))
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedEof`] when fewer than eight bytes
    /// remain in the payload.
    pub fn read_f64(&mut self) -> Result<f64, ProtocolError> {
        Ok(f64::from_be_bytes(self.read_exact::<8>()?))
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedEof`] when the varint is incomplete,
    /// or [`ProtocolError::InvalidVarInt`] when it exceeds the protocol's
    /// 5-byte representation.
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

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the length prefix is invalid, the payload
    /// ends before the string bytes are available, the bytes are not UTF-8, or
    /// the decoded string exceeds `max_len`.
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

    /// # Errors
    ///
    /// Returns [`ProtocolError::UnexpectedEof`] when fewer than `length` bytes
    /// remain in the payload.
    pub fn read_bytes(&mut self, length: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self.cursor.saturating_add(length);
        let slice = self
            .payload
            .get(self.cursor..end)
            .ok_or(ProtocolError::UnexpectedEof)?;
        self.cursor = end;
        Ok(slice)
    }

    #[must_use]
    pub const fn is_exhausted(&self) -> bool {
        self.cursor == self.payload.len()
    }

    fn read_exact<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let bytes = self.read_bytes(N)?;
        let mut array = [0_u8; N];
        array.copy_from_slice(bytes);
        Ok(array)
    }
}

pub fn peek_varint(buffer: &BytesMut) -> Option<(i32, usize)> {
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
