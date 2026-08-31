use bedrock_protocol_core::error::{PacketCodecError, ProtoCodecError};
use bedrock_protocol_core::{PacketHeader, Packets, ProtoCodecVAR};
use flate2::Compression as FlateCompression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use std::io::{Cursor, Read, Write};
use thiserror::Error;

/// RakNet payload discriminator for a Bedrock packet batch.
pub const BEDROCK_GAME_PACKET_ID: u8 = 0xfe;

/// RakNet offline-message marker used by Bedrock discovery and connection setup.
pub const BEDROCK_RAKNET_MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

const ZLIB_METHOD_ID: u8 = 0;
const UNCOMPRESSED_METHOD_ID: u8 = u8::MAX;

/// Negotiated zlib compression for a Bedrock packet batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BedrockCompression {
    threshold: u16,
}

impl BedrockCompression {
    /// Constructs the zlib mode advertised by the server after login.
    #[must_use]
    pub const fn zlib(threshold: u16) -> Self {
        Self { threshold }
    }

    /// Adds the negotiated compression-method discriminator and compresses when required.
    pub fn compress(&self, payload: &[u8]) -> Result<Vec<u8>, BedrockWireError> {
        let mut encoded = Vec::with_capacity(payload.len().saturating_add(1));
        if usize::from(self.threshold) >= payload.len() {
            encoded.push(UNCOMPRESSED_METHOD_ID);
            encoded.extend_from_slice(payload);
            return Ok(encoded);
        }

        encoded.push(ZLIB_METHOD_ID);
        let mut compressor = DeflateEncoder::new(encoded, FlateCompression::default());
        compressor.write_all(payload)?;
        compressor.finish().map_err(BedrockWireError::Io)
    }

    /// Removes the negotiated compression-method discriminator and decodes the batch.
    pub fn decompress(&self, payload: &[u8]) -> Result<Vec<u8>, BedrockWireError> {
        let Some((&method, encoded)) = payload.split_first() else {
            return Err(BedrockWireError::MissingCompressionMethod);
        };

        match method {
            UNCOMPRESSED_METHOD_ID => Ok(encoded.to_vec()),
            ZLIB_METHOD_ID => {
                let mut decoded = Vec::new();
                DeflateDecoder::new(encoded).read_to_end(&mut decoded)?;
                Ok(decoded)
            }
            unsupported => Err(BedrockWireError::UnsupportedCompressionMethod(unsupported)),
        }
    }
}

/// Failures while framing or compressing a Bedrock packet batch.
#[derive(Debug, Error)]
pub enum BedrockWireError {
    #[error("packet codec failed: {0}")]
    Packet(#[from] PacketCodecError),
    #[error("primitive codec failed: {0}")]
    Primitive(#[from] ProtoCodecError),
    #[error("packet payload is too large to encode: {0} bytes")]
    PacketTooLarge(usize),
    #[error("compressed batch does not contain a compression method")]
    MissingCompressionMethod,
    #[error("unsupported compression method {0}")]
    UnsupportedCompressionMethod(u8),
    #[error("compression stream failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Encodes protocol packets as one optionally compressed Bedrock batch.
pub fn encode_packet_batch<T: Packets>(
    packets: &[T],
    compression: Option<&BedrockCompression>,
) -> Result<Vec<u8>, BedrockWireError> {
    let mut batch = Vec::new();
    for packet in packets {
        let header = PacketHeader {
            packet_id: packet.id(),
            sender_sub_client_id: 0,
            target_sub_client_id: 0,
        };
        let mut encoded_packet = Vec::with_capacity(packet.size_hint(&header));
        packet.serialize(&header, &mut encoded_packet)?;

        let packet_len = u32::try_from(encoded_packet.len())
            .map_err(|_| BedrockWireError::PacketTooLarge(encoded_packet.len()))?;
        <u32 as ProtoCodecVAR>::serialize(&packet_len, &mut batch)?;
        batch.extend_from_slice(&encoded_packet);
    }

    if let Some(mode) = compression {
        mode.compress(&batch)
    } else {
        Ok(batch)
    }
}

/// Decodes one optionally compressed Bedrock batch into its protocol packets.
pub fn decode_packet_batch<T: Packets>(
    payload: &[u8],
    compression: Option<&BedrockCompression>,
) -> Result<Vec<T>, BedrockWireError> {
    let decoded;
    let payload = if let Some(mode) = compression {
        decoded = mode.decompress(payload)?;
        decoded.as_slice()
    } else {
        payload
    };

    let mut batch = Cursor::new(payload);
    let mut packets = Vec::new();
    let batch_len = u64::try_from(payload.len())
        .map_err(|_| BedrockWireError::PacketTooLarge(payload.len()))?;
    while batch.position() < batch_len {
        let packet_len = <u32 as ProtoCodecVAR>::deserialize(&mut batch)?;
        let packet_len = usize::try_from(packet_len)
            .map_err(|_| BedrockWireError::PacketTooLarge(payload.len()))?;
        let mut encoded_packet = vec![0; packet_len];
        batch.read_exact(&mut encoded_packet)?;
        let (packet, _) = T::deserialize(&mut Cursor::new(encoded_packet))?;
        packets.push(packet);
    }
    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zlib_round_trips_payload_above_threshold() {
        let compression = BedrockCompression::zlib(1);
        let payload = b"bedrock packet batch";
        let encoded = compression
            .compress(payload)
            .expect("compression should succeed");

        assert_eq!(encoded.first(), Some(&ZLIB_METHOD_ID));
        assert_eq!(
            compression
                .decompress(&encoded)
                .expect("decompression should succeed"),
            payload
        );
    }

    #[test]
    fn payload_at_threshold_stays_uncompressed() {
        let payload = b"batch";
        let compression = BedrockCompression::zlib(5);
        let encoded = compression
            .compress(payload)
            .expect("framing should succeed");

        assert_eq!(encoded.first(), Some(&UNCOMPRESSED_METHOD_ID));
        assert_eq!(&encoded[1..], payload);
    }

    #[test]
    fn unknown_compression_method_is_rejected() {
        let error = BedrockCompression::zlib(0)
            .decompress(&[7])
            .expect_err("unknown method must fail");

        assert!(matches!(
            error,
            BedrockWireError::UnsupportedCompressionMethod(7)
        ));
    }
}
