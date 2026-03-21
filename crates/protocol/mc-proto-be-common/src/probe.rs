use bedrockrs_proto::info::MAGIC as BEDROCK_MAGIC;
use mc_proto_common::{Edition, HandshakeIntent, HandshakeNextState};

pub(crate) const RAKNET_UNCONNECTED_PING: u8 = 0x01;
pub(crate) const RAKNET_OPEN_CONNECTION_REQUEST_1: u8 = 0x05;
pub(crate) const RAKNET_OPEN_CONNECTION_REQUEST_2: u8 = 0x07;

fn has_magic_at(frame: &[u8], offset: usize) -> bool {
    frame
        .get(offset..offset + BEDROCK_MAGIC.len())
        .is_some_and(|slice| slice == BEDROCK_MAGIC)
}

#[must_use]
pub(crate) fn detects_bedrock_datagram(frame: &[u8]) -> bool {
    let Some(packet_id) = frame.first().copied() else {
        return false;
    };
    match packet_id {
        RAKNET_UNCONNECTED_PING => frame.len() >= 25 && has_magic_at(frame, 9),
        RAKNET_OPEN_CONNECTION_REQUEST_1 | RAKNET_OPEN_CONNECTION_REQUEST_2 => {
            frame.len() >= 17 && has_magic_at(frame, 1)
        }
        _ => false,
    }
}

#[must_use]
pub(crate) const fn bedrock_probe_intent() -> HandshakeIntent {
    HandshakeIntent {
        edition: Edition::Be,
        protocol_number: 0,
        server_host: String::new(),
        server_port: 0,
        next_state: HandshakeNextState::Login,
    }
}
