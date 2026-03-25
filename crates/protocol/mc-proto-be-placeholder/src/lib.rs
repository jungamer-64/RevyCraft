#![allow(clippy::multiple_crate_versions)]
use mc_core::{CoreCommand, CoreEvent, PlayerSnapshot};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeIntent, HandshakeNextState, HandshakeProbe, LoginRequest,
    PlayEncodingContext, PlaySyncAdapter, ProtocolAdapter, ProtocolDescriptor, ProtocolError,
    ProtocolSessionSnapshot, RawPacketStreamWireCodec, ServerListStatus, SessionAdapter,
    StatusRequest, TransportKind, WireCodec, WireFormatKind,
};

const VERSION_NAME_PLACEHOLDER: &str = "bedrock-placeholder";
pub const BE_PLACEHOLDER_ADAPTER_ID: &str = "be-placeholder";
const RAKNET_UNCONNECTED_PING: u8 = 0x01;
const RAKNET_OPEN_CONNECTION_REQUEST_1: u8 = 0x05;
const RAKNET_OPEN_CONNECTION_REQUEST_2: u8 = 0x07;
const UNCONNECTED_MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

#[derive(Default)]
pub struct BePlaceholderAdapter {
    codec: RawPacketStreamWireCodec,
}

impl BePlaceholderAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn has_magic_at(frame: &[u8], offset: usize) -> bool {
    frame
        .get(offset..offset + UNCONNECTED_MAGIC.len())
        .is_some_and(|slice| slice == UNCONNECTED_MAGIC)
}

fn detects_bedrock_datagram(frame: &[u8]) -> bool {
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

impl HandshakeProbe for BePlaceholderAdapter {
    fn transport_kind(&self) -> TransportKind {
        TransportKind::Udp
    }

    fn adapter_id(&self) -> Option<&'static str> {
        Some(BE_PLACEHOLDER_ADAPTER_ID)
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        if !detects_bedrock_datagram(frame) {
            return Ok(None);
        }
        Ok(Some(HandshakeIntent {
            edition: Edition::Be,
            protocol_number: 0,
            server_host: String::new(),
            server_port: 0,
            next_state: HandshakeNextState::Login,
        }))
    }
}

const fn unsupported() -> ProtocolError {
    ProtocolError::InvalidPacket("bedrock placeholder adapter does not support sessions")
}

impl SessionAdapter for BePlaceholderAdapter {
    fn wire_codec(&self) -> &dyn WireCodec {
        &self.codec
    }

    fn decode_status(&self, _frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        Err(unsupported())
    }

    fn decode_login(&self, _frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        Err(unsupported())
    }

    fn encode_status_response(&self, _status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        Err(unsupported())
    }

    fn encode_status_pong(&self, _payload: i64) -> Result<Vec<u8>, ProtocolError> {
        Err(unsupported())
    }

    fn encode_disconnect(
        &self,
        _phase: ConnectionPhase,
        _reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(unsupported())
    }

    fn encode_encryption_request(
        &self,
        _server_id: &str,
        _public_key_der: &[u8],
        _verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(unsupported())
    }

    fn encode_network_settings(
        &self,
        _compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(unsupported())
    }

    fn encode_login_success(&self, _player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
        Err(unsupported())
    }
}

impl PlaySyncAdapter for BePlaceholderAdapter {
    fn decode_play(
        &self,
        _session: &ProtocolSessionSnapshot,
        _frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError> {
        Err(unsupported())
    }

    fn encode_play_event(
        &self,
        _event: &CoreEvent,
        _session: &ProtocolSessionSnapshot,
        _context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        Err(unsupported())
    }
}

impl ProtocolAdapter for BePlaceholderAdapter {
    fn descriptor(&self) -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: BE_PLACEHOLDER_ADAPTER_ID.to_string(),
            transport: TransportKind::Udp,
            wire_format: WireFormatKind::RawPacketStream,
            edition: Edition::Be,
            version_name: VERSION_NAME_PLACEHOLDER.to_string(),
            protocol_number: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BePlaceholderAdapter, RAKNET_OPEN_CONNECTION_REQUEST_1, RAKNET_UNCONNECTED_PING};
    use mc_proto_common::{Edition, HandshakeProbe, TransportKind};

    const MAGIC: [u8; 16] = [
        0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56,
        0x78,
    ];

    fn raknet_unconnected_ping() -> Vec<u8> {
        let mut frame = Vec::with_capacity(33);
        frame.push(RAKNET_UNCONNECTED_PING);
        frame.extend_from_slice(&123_i64.to_be_bytes());
        frame.extend_from_slice(&MAGIC);
        frame.extend_from_slice(&456_i64.to_be_bytes());
        frame
    }

    fn raknet_open_connection_request_1() -> Vec<u8> {
        let mut frame = Vec::with_capacity(18);
        frame.push(RAKNET_OPEN_CONNECTION_REQUEST_1);
        frame.extend_from_slice(&MAGIC);
        frame.push(11);
        frame
    }

    #[test]
    fn bedrock_probe_uses_udp_transport() {
        let adapter = BePlaceholderAdapter::new();
        assert_eq!(adapter.transport_kind(), TransportKind::Udp);
    }

    #[test]
    fn unconnected_ping_routes_to_bedrock() {
        let adapter = BePlaceholderAdapter::new();
        let intent = adapter
            .try_route(&raknet_unconnected_ping())
            .expect("probe should not fail")
            .expect("unconnected ping should match");
        assert_eq!(intent.edition, Edition::Be);
    }

    #[test]
    fn open_connection_request_routes_to_bedrock() {
        let adapter = BePlaceholderAdapter::new();
        let intent = adapter
            .try_route(&raknet_open_connection_request_1())
            .expect("probe should not fail")
            .expect("open connection request should match");
        assert_eq!(intent.edition, Edition::Be);
    }

    #[test]
    fn non_bedrock_udp_datagram_is_ignored() {
        let adapter = BePlaceholderAdapter::new();
        let intent = adapter
            .try_route(&[0xfe, 0xed, 0xfa, 0xce])
            .expect("probe should not fail");
        assert!(intent.is_none());
    }
}
