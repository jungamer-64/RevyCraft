#![allow(clippy::multiple_crate_versions)]
mod codec;
mod decoding;
mod encoding;
mod runtime_ids;

#[cfg(test)]
mod tests;

use bedrockrs_proto::ProtoVersion;
use mc_core::{BlockState, CoreCommand, EntityId, PlayerId, PlayerSnapshot, WorldMeta};
use mc_proto_be_common::{BedrockAdapter, BedrockProfile};
use mc_proto_common::{
    BedrockListenerDescriptor, ConnectionPhase, Edition, LoginRequest, ProtocolDescriptor,
    ProtocolError, TransportKind, WireFormatKind,
};

pub const BE_26_3_ADAPTER_ID: &str = "be-26_3";
pub const BE_26_3_VERSION_NAME: &str = "bedrock-26.3";
pub const BE_26_3_PROTOCOL_NUMBER: i32 = 924;

#[derive(Default)]
pub struct Bedrock263Profile;

pub type Bedrock263Adapter = BedrockAdapter<Bedrock263Profile>;

impl BedrockProfile for Bedrock263Profile {
    fn adapter_id(&self) -> &'static str {
        BE_26_3_ADAPTER_ID
    }

    fn descriptor(&self) -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: BE_26_3_ADAPTER_ID.to_string(),
            transport: TransportKind::Udp,
            wire_format: WireFormatKind::RawPacketStream,
            edition: Edition::Be,
            version_name: BE_26_3_VERSION_NAME.to_string(),
            protocol_number: BE_26_3_PROTOCOL_NUMBER,
        }
    }

    fn listener_descriptor(&self) -> BedrockListenerDescriptor {
        BedrockListenerDescriptor {
            game_version: bedrockrs_proto::V924::GAME_VERSION.to_string(),
            raknet_version: bedrockrs_proto::V924::RAKNET_VERSION,
        }
    }

    fn decode_login_request(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        decoding::decode_login_request(frame)
    }

    fn encode_disconnect_packet(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        encoding::encode_disconnect_packet(phase, reason)
    }

    fn encode_network_settings_packet(
        &self,
        compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        encoding::encode_network_settings_packet(compression_threshold)
    }

    fn encode_login_success_packet(
        &self,
        player: &PlayerSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        encoding::encode_login_success_packet(player)
    }

    fn decode_play_packet(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError> {
        decoding::decode_play_packet(player_id, frame)
    }

    fn encode_play_bootstrap_packets(
        &self,
        player: &PlayerSnapshot,
        entity_id: EntityId,
        world_meta: &WorldMeta,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_play_bootstrap_packets(player, entity_id, world_meta)
    }

    fn encode_entity_moved_packets(
        &self,
        entity_id: EntityId,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_entity_moved_packets(entity_id, player)
    }

    fn encode_block_changed_packets(
        &self,
        position: mc_core::BlockPos,
        block: &BlockState,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_block_changed_packets(position, block)
    }
}
