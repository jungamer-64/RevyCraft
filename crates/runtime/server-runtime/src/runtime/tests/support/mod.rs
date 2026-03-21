mod build;
mod network;
mod packets;
mod plugins;

use super::*;

pub(crate) use self::build::*;
pub(crate) use self::network::*;
pub(crate) use self::packets::*;
pub(crate) use self::plugins::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UdpDatagramAction {
    Ignore,
    UnsupportedBedrock,
}

pub(crate) fn classify_udp_datagram(
    protocol_registry: &ProtocolRegistry,
    datagram: &[u8],
) -> Result<UdpDatagramAction, ProtocolError> {
    match protocol_registry.route_handshake(TransportKind::Udp, datagram)? {
        Some(intent) if intent.edition == Edition::Be => Ok(UdpDatagramAction::UnsupportedBedrock),
        Some(_) | None => Ok(UdpDatagramAction::Ignore),
    }
}
