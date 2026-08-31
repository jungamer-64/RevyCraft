use bedrock_protocol::V924;
use mc_proto_be_common::{decode_packet_batch, encode_packet_batch};
use mc_proto_common::ProtocolError;

pub(crate) fn encode_v924(packets: &[V924]) -> Result<Vec<u8>, ProtocolError> {
    encode_packet_batch(packets, None)
        .map_err(|error| ProtocolError::Plugin(format!("bedrock encode failed: {error}")))
}

pub(crate) fn decode_v924(frame: &[u8]) -> Result<Vec<V924>, ProtocolError> {
    decode_packet_batch::<V924>(frame, None)
        .map_err(|error| ProtocolError::Plugin(format!("bedrock decode failed: {error}")))
}
