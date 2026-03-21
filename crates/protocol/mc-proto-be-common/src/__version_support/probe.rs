use crate::probe;
use mc_proto_common::HandshakeIntent;

pub const RAKNET_OPEN_CONNECTION_REQUEST_1: u8 = probe::RAKNET_OPEN_CONNECTION_REQUEST_1;
pub const RAKNET_OPEN_CONNECTION_REQUEST_2: u8 = probe::RAKNET_OPEN_CONNECTION_REQUEST_2;
pub const RAKNET_UNCONNECTED_PING: u8 = probe::RAKNET_UNCONNECTED_PING;

pub fn detects_bedrock_datagram(frame: &[u8]) -> bool {
    probe::detects_bedrock_datagram(frame)
}

pub const fn bedrock_probe_intent() -> HandshakeIntent {
    probe::bedrock_probe_intent()
}
