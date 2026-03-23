use crate::probe::{bedrock_probe_intent, detects_bedrock_datagram};
use mc_core::{
    BlockPos, BlockState, CoreCommand, CoreEvent, EntityId, PlayerId, PlayerSnapshot, WorldMeta,
};
use mc_proto_common::{
    BedrockListenerDescriptor, ConnectionPhase, HandshakeIntent, HandshakeProbe, LoginRequest,
    PlayEncodingContext, PlaySyncAdapter, ProtocolAdapter, ProtocolDescriptor, ProtocolError,
    RawPacketStreamWireCodec, ServerListStatus, SessionAdapter, StatusRequest, TransportKind,
    WireCodec,
};

pub trait BedrockProfile: Default + Send + Sync {
    fn adapter_id(&self) -> &'static str;
    fn descriptor(&self) -> ProtocolDescriptor;
    fn listener_descriptor(&self) -> BedrockListenerDescriptor;
    fn decode_login_request(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError>;
    fn encode_disconnect_packet(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_network_settings_packet(
        &self,
        compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn encode_login_success_packet(
        &self,
        player: &PlayerSnapshot,
    ) -> Result<Vec<u8>, ProtocolError>;
    fn decode_play_packet(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError>;
    fn encode_play_bootstrap_packets(
        &self,
        player: &PlayerSnapshot,
        entity_id: EntityId,
        world_meta: &WorldMeta,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_entity_moved_packets(
        &self,
        entity_id: EntityId,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
    fn encode_block_changed_packets(
        &self,
        position: BlockPos,
        block: &BlockState,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
}

#[derive(Default)]
pub struct BedrockAdapter<P> {
    codec: RawPacketStreamWireCodec,
    profile: P,
}

impl<P: Default> BedrockAdapter<P> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl<P: BedrockProfile> HandshakeProbe for BedrockAdapter<P> {
    fn transport_kind(&self) -> TransportKind {
        TransportKind::Udp
    }

    fn adapter_id(&self) -> Option<&'static str> {
        Some(self.profile.adapter_id())
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        if detects_bedrock_datagram(frame) {
            Ok(Some(bedrock_probe_intent()))
        } else {
            Ok(None)
        }
    }
}

impl<P: BedrockProfile> SessionAdapter for BedrockAdapter<P> {
    fn wire_codec(&self) -> &dyn WireCodec {
        &self.codec
    }

    fn decode_status(&self, _frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        Err(crate::world::protocol_error(
            "bedrock status requests are handled by the raknet listener",
        ))
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        self.profile.decode_login_request(frame)
    }

    fn encode_status_response(&self, _status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        Err(crate::world::protocol_error(
            "bedrock status responses are handled by the raknet listener",
        ))
    }

    fn encode_status_pong(&self, _payload: i64) -> Result<Vec<u8>, ProtocolError> {
        Err(crate::world::protocol_error(
            "bedrock status pong is handled by the raknet listener",
        ))
    }

    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.profile.encode_disconnect_packet(phase, reason)
    }

    fn encode_encryption_request(
        &self,
        _server_id: &str,
        _public_key_der: &[u8],
        _verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(crate::world::protocol_error(
            "bedrock adapters do not use java edition encryption requests",
        ))
    }

    fn encode_network_settings(
        &self,
        compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.profile
            .encode_network_settings_packet(compression_threshold)
    }

    fn encode_login_success(&self, player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
        self.profile.encode_login_success_packet(player)
    }
}

impl<P: BedrockProfile> PlaySyncAdapter for BedrockAdapter<P> {
    fn decode_play(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError> {
        self.profile.decode_play_packet(player_id, frame)
    }

    fn encode_play_event(
        &self,
        event: &CoreEvent,
        _context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        match event {
            CoreEvent::PlayBootstrap {
                player,
                entity_id,
                world_meta,
                ..
            } => self
                .profile
                .encode_play_bootstrap_packets(player, *entity_id, world_meta),
            CoreEvent::EntityMoved { entity_id, player } => {
                self.profile.encode_entity_moved_packets(*entity_id, player)
            }
            CoreEvent::BlockChanged { position, block } => {
                self.profile.encode_block_changed_packets(*position, block)
            }
            CoreEvent::KeepAliveRequested { .. }
            | CoreEvent::ChunkBatch { .. }
            | CoreEvent::EntitySpawned { .. }
            | CoreEvent::EntityDespawned { .. }
            | CoreEvent::InventoryContents { .. }
            | CoreEvent::InventorySlotChanged { .. }
            | CoreEvent::CursorChanged { .. }
            | CoreEvent::SelectedHotbarSlotChanged { .. }
            | CoreEvent::LoginAccepted { .. }
            | CoreEvent::Disconnect { .. } => Ok(Vec::new()),
        }
    }
}

impl<P: BedrockProfile> ProtocolAdapter for BedrockAdapter<P> {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.profile.descriptor()
    }

    fn bedrock_listener_descriptor(&self) -> Option<BedrockListenerDescriptor> {
        Some(self.profile.listener_descriptor())
    }
}
