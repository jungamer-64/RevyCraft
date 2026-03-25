#![allow(clippy::multiple_crate_versions)]
mod chunk;
mod codec;
mod decoding;
mod encoding;
mod inventory;
mod runtime_ids;

#[cfg(test)]
mod tests;

use bedrockrs_proto::ProtoVersion;
use mc_core::{
    BlockState, ChunkColumn, CoreCommand, DroppedItemSnapshot, EntityId, InventoryContainer,
    InventorySlot, InventoryWindowContents, ItemStack, PlayerId, PlayerSnapshot, WorldMeta,
};
use mc_proto_be_common::{BedrockAdapter, BedrockProfile};
use mc_proto_common::{
    BedrockListenerDescriptor, ConnectionPhase, Edition, LoginRequest, ProtocolDescriptor,
    ProtocolError, TransportKind, WireFormatKind,
};

pub const BE_924_ADAPTER_ID: &str = "be-924";
pub const BE_924_VERSION_NAME: &str = "bedrock-26.3";
pub const BE_924_PROTOCOL_NUMBER: i32 = 924;

#[derive(Default)]
pub struct Bedrock924Profile;

pub type Bedrock924Adapter = BedrockAdapter<Bedrock924Profile>;

impl BedrockProfile for Bedrock924Profile {
    fn adapter_id(&self) -> &'static str {
        BE_924_ADAPTER_ID
    }

    fn descriptor(&self) -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: BE_924_ADAPTER_ID.to_string(),
            transport: TransportKind::Udp,
            wire_format: WireFormatKind::RawPacketStream,
            edition: Edition::Be,
            version_name: BE_924_VERSION_NAME.to_string(),
            protocol_number: BE_924_PROTOCOL_NUMBER,
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

    fn encode_dropped_item_spawn_packets(
        &self,
        entity_id: EntityId,
        item: &DroppedItemSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_dropped_item_spawn_packets(entity_id, item)
    }

    fn encode_entity_despawn_packets(
        &self,
        entity_ids: &[EntityId],
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_entity_despawn_packets(entity_ids)
    }

    fn encode_chunk_batch_packets(
        &self,
        chunks: &[ChunkColumn],
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_chunk_batch_packets(chunks)
    }

    fn encode_block_changed_packets(
        &self,
        position: mc_core::BlockPos,
        block: &BlockState,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_block_changed_packets(position, block)
    }

    fn encode_block_breaking_progress_packets(
        &self,
        breaker_entity_id: EntityId,
        position: mc_core::BlockPos,
        stage: Option<u8>,
        duration_ms: u64,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_block_breaking_progress_packets(
            breaker_entity_id,
            position,
            stage,
            duration_ms,
        )
    }

    fn encode_inventory_contents_packets(
        &self,
        window_id: u8,
        container: InventoryContainer,
        contents: &InventoryWindowContents,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_inventory_contents_packets(window_id, container, contents)
    }

    fn encode_container_opened_packets(
        &self,
        window_id: u8,
        container: InventoryContainer,
        title: &str,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_container_opened_packets(window_id, container, title)
    }

    fn encode_container_closed_packets(
        &self,
        window_id: u8,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_container_closed_packets(window_id)
    }

    fn encode_container_property_changed_packets(
        &self,
        window_id: u8,
        property_id: u8,
        value: i16,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_container_property_changed_packets(window_id, property_id, value)
    }

    fn encode_inventory_slot_changed_packets(
        &self,
        window_id: u8,
        container: InventoryContainer,
        slot: InventorySlot,
        stack: Option<&ItemStack>,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_inventory_slot_changed_packets(window_id, container, slot, stack)
    }

    fn encode_selected_hotbar_slot_changed_packets(
        &self,
        slot: u8,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        encoding::encode_selected_hotbar_slot_changed_packets(slot)
    }
}
