use bedrockrs_proto::V924;
use bedrockrs_proto::codec::{decode_packets, encode_packets};
use mc_proto_common::ProtocolError;

pub(crate) fn encode_v924(packets: &[V924]) -> Result<Vec<u8>, ProtocolError> {
    encode_packets(packets, None, None)
        .map_err(|error| ProtocolError::Plugin(format!("bedrock encode failed: {error}")))
}

pub(crate) fn decode_v924(frame: &[u8]) -> Result<Vec<V924>, ProtocolError> {
    decode_packets::<V924>(frame.to_vec(), None, None).map_err(|error| {
        ProtocolError::InvalidPacket(Box::leak(
            format!("bedrock decode failed: {error}").into_boxed_str(),
        ))
    })
}
