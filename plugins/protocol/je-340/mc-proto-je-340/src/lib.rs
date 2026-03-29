#![allow(clippy::multiple_crate_versions)]
mod decoding;
mod encoding;

#[cfg(test)]
mod tests;

use decoding::decode_play_packet;
use encoding::{
    encode_block_break_animation, encode_block_change, encode_chunk, encode_close_window,
    encode_confirm_transaction, encode_destroy_entities, encode_dropped_item_metadata,
    encode_dropped_item_spawn, encode_entity_head_rotation, encode_entity_teleport,
    encode_held_item_change, encode_join_game, encode_keep_alive, encode_named_entity_spawn,
    encode_open_window, encode_player_abilities, encode_player_info_add, encode_position_and_look,
    encode_set_slot, encode_spawn_position, encode_time_update, encode_update_health,
    encode_window_items, encode_window_property,
};
use mc_proto_common::{
    ProtocolDescriptor, ProtocolError, ProtocolSessionSnapshot, TransportKind, WireFormatKind,
};
use mc_proto_je_common::{
    __version_support::inventory::{
        CURSOR_SLOT_ID, CURSOR_WINDOW_ID, InventoryProtocolSpec, JE_1_12_2_INVENTORY_SPEC,
        protocol_slot, signed_window_id,
    },
    JavaEditionAdapter, JavaEditionProfile, JavaProtocolSessionStore, format_text_component,
};
use revy_voxel_core::{EntityId, PlayerSnapshot, RuntimeCommand};
use revy_voxel_model::{
    BlockPos, BlockState, ChunkColumn, DroppedItemSnapshot, InventorySlot,
    InventoryTransactionContext, InventoryWindowContents, ItemStack, WorldMeta,
};
use revy_voxel_rules::{ContainerKindId, ContainerPropertyKey};

const PROTOCOL_VERSION_1_12_2: i32 = 340;
const VERSION_NAME_1_12_2: &str = "1.12.2";
pub const JE_340_ADAPTER_ID: &str = "je-340";
pub(crate) const INVENTORY_SPEC: InventoryProtocolSpec = JE_1_12_2_INVENTORY_SPEC;

fn container_property_id(property: &ContainerPropertyKey) -> Option<u8> {
    match property.as_str() {
        "canonical:furnace.burn_left" => Some(0),
        "canonical:furnace.burn_max" => Some(1),
        "canonical:furnace.cook_progress" => Some(2),
        "canonical:furnace.cook_total" => Some(3),
        _ => None,
    }
}

const PACKET_CB_NAMED_ENTITY_SPAWN: i32 = 0x05;
const PACKET_CB_SPAWN_OBJECT: i32 = 0x00;
const PACKET_CB_BLOCK_BREAK_ANIMATION: i32 = 0x08;
const PACKET_CB_BLOCK_CHANGE: i32 = 0x0b;
const PACKET_CB_TRANSACTION: i32 = 0x11;
const PACKET_CB_CLOSE_WINDOW: i32 = 0x12;
const PACKET_CB_OPEN_WINDOW: i32 = 0x13;
const PACKET_CB_WINDOW_ITEMS: i32 = 0x14;
const PACKET_CB_WINDOW_PROPERTY: i32 = 0x15;
const PACKET_CB_SET_SLOT: i32 = 0x16;
const PACKET_CB_PLAY_DISCONNECT: i32 = 0x1a;
const PACKET_CB_KEEP_ALIVE: i32 = 0x1f;
const PACKET_CB_MAP_CHUNK: i32 = 0x20;
const PACKET_CB_JOIN_GAME: i32 = 0x23;
const PACKET_CB_PLAYER_INFO: i32 = 0x2d;
const PACKET_CB_PLAYER_ABILITIES: i32 = 0x2c;
const PACKET_CB_PLAYER_POSITION_AND_LOOK: i32 = 0x2f;
const PACKET_CB_DESTROY_ENTITIES: i32 = 0x32;
const PACKET_CB_ENTITY_HEAD_ROTATION: i32 = 0x36;
const PACKET_CB_ENTITY_METADATA: i32 = 0x3c;
const PACKET_CB_HELD_ITEM_CHANGE: i32 = 0x3a;
const PACKET_CB_SPAWN_POSITION: i32 = 0x46;
const PACKET_CB_TIME_UPDATE: i32 = 0x47;
const PACKET_CB_ENTITY_TELEPORT: i32 = 0x4c;
const PACKET_CB_UPDATE_HEALTH: i32 = 0x41;

const PACKET_SB_KEEP_ALIVE: i32 = 0x0b;
const PACKET_SB_FLYING: i32 = 0x0c;
const PACKET_SB_POSITION: i32 = 0x0d;
const PACKET_SB_POSITION_LOOK: i32 = 0x0e;
const PACKET_SB_LOOK: i32 = 0x0f;
const PACKET_SB_PLAYER_DIGGING: i32 = 0x14;
const PACKET_SB_HELD_ITEM_CHANGE: i32 = 0x1a;
const PACKET_SB_CREATIVE_INVENTORY_ACTION: i32 = 0x1b;
const PACKET_SB_PLAYER_BLOCK_PLACEMENT: i32 = 0x1f;
const PACKET_SB_USE_ITEM: i32 = 0x20;
const PACKET_SB_CLIENT_COMMAND: i32 = 0x03;
const PACKET_SB_SETTINGS: i32 = 0x04;
const PACKET_SB_CONFIRM_TRANSACTION: i32 = 0x05;
const PACKET_SB_CLOSE_WINDOW: i32 = 0x08;
const PACKET_SB_CLICK_WINDOW: i32 = 0x07;

#[derive(Default)]
pub struct Je340Profile {
    sessions: JavaProtocolSessionStore,
}

pub type Je340Adapter = JavaEditionAdapter<Je340Profile>;

impl JavaEditionProfile for Je340Profile {
    fn adapter_id(&self) -> &'static str {
        JE_340_ADAPTER_ID
    }

    fn descriptor(&self) -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: JE_340_ADAPTER_ID.to_string(),
            transport: TransportKind::Tcp,
            wire_format: WireFormatKind::MinecraftFramed,
            edition: mc_proto_common::Edition::Je,
            version_name: VERSION_NAME_1_12_2.to_string(),
            protocol_number: PROTOCOL_VERSION_1_12_2,
        }
    }

    fn play_disconnect_packet_id(&self) -> i32 {
        PACKET_CB_PLAY_DISCONNECT
    }

    fn format_disconnect_reason(&self, reason: &str) -> String {
        format_text_component(reason)
    }

    fn encode_play_bootstrap(
        &self,
        entity_id: EntityId,
        world_meta: &WorldMeta,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        Ok(vec![
            encode_join_game(entity_id, world_meta, player)?,
            encode_spawn_position(world_meta.spawn),
            encode_time_update(world_meta.age, world_meta.time),
            encode_update_health(player),
            encode_player_abilities(world_meta.game_mode == 1),
            encode_position_and_look(player),
        ])
    }

    fn encode_chunk_batch(&self, chunks: &[ChunkColumn]) -> Result<Vec<Vec<u8>>, ProtocolError> {
        chunks.iter().map(encode_chunk).collect()
    }

    fn encode_entity_spawn(
        &self,
        entity_id: EntityId,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        Ok(vec![
            encode_player_info_add(player)?,
            encode_named_entity_spawn(entity_id, player),
            encode_entity_head_rotation(entity_id, player.yaw),
        ])
    }

    fn encode_entity_moved(
        &self,
        entity_id: EntityId,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        Ok(vec![
            encode_entity_teleport(entity_id, player),
            encode_entity_head_rotation(entity_id, player.yaw),
        ])
    }

    fn encode_dropped_item_spawn(
        &self,
        entity_id: EntityId,
        item: &DroppedItemSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        Ok(vec![
            encode_dropped_item_spawn(entity_id, item),
            encode_dropped_item_metadata(entity_id, item)?,
        ])
    }

    fn encode_entity_despawn(&self, entity_ids: &[EntityId]) -> Result<Vec<u8>, ProtocolError> {
        encode_destroy_entities(entity_ids)
    }

    fn encode_container_opened(
        &self,
        window_id: u8,
        container: &ContainerKindId,
        title: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        encode_open_window(window_id, container, title)
    }

    fn encode_container_closed(&self, window_id: u8) -> Result<Vec<u8>, ProtocolError> {
        Ok(encode_close_window(window_id))
    }

    fn encode_inventory_contents(
        &self,
        window_id: u8,
        container: &ContainerKindId,
        contents: &InventoryWindowContents,
    ) -> Result<Vec<u8>, ProtocolError> {
        encode_window_items(window_id, container, contents)
    }

    fn encode_container_property_changed(
        &self,
        window_id: u8,
        property_id: &ContainerPropertyKey,
        value: i16,
    ) -> Result<Vec<u8>, ProtocolError> {
        let Some(property_id) = container_property_id(property_id) else {
            return Ok(Vec::new());
        };
        Ok(encode_window_property(window_id, property_id, value))
    }

    fn encode_inventory_slot_changed(
        &self,
        window_id: u8,
        container: &ContainerKindId,
        slot: InventorySlot,
        stack: Option<&ItemStack>,
    ) -> Result<Option<Vec<u8>>, ProtocolError> {
        let Some(protocol_slot) = protocol_slot(container, INVENTORY_SPEC.layout, slot) else {
            return Ok(None);
        };
        Ok(Some(encode_set_slot(
            signed_window_id(window_id),
            protocol_slot,
            stack,
        )?))
    }

    fn encode_inventory_transaction_processed(
        &self,
        transaction: InventoryTransactionContext,
        accepted: bool,
    ) -> Result<Vec<u8>, ProtocolError> {
        Ok(encode_confirm_transaction(
            transaction.window_id,
            transaction.action_number,
            accepted,
        ))
    }

    fn encode_cursor_changed(&self, stack: Option<&ItemStack>) -> Result<Vec<u8>, ProtocolError> {
        encode_set_slot(CURSOR_WINDOW_ID, CURSOR_SLOT_ID, stack)
    }

    fn encode_selected_hotbar_slot_changed(&self, slot: u8) -> Result<Vec<u8>, ProtocolError> {
        Ok(encode_held_item_change(slot))
    }

    fn encode_block_changed(
        &self,
        position: BlockPos,
        block: &BlockState,
    ) -> Result<Vec<u8>, ProtocolError> {
        Ok(encode_block_change(position, block))
    }

    fn encode_block_breaking_progress(
        &self,
        breaker_entity_id: EntityId,
        position: BlockPos,
        stage: Option<u8>,
    ) -> Result<Vec<u8>, ProtocolError> {
        Ok(encode_block_break_animation(
            breaker_entity_id,
            position,
            stage,
        ))
    }

    fn encode_keep_alive_requested(&self, keep_alive_id: i32) -> Result<Vec<u8>, ProtocolError> {
        Ok(encode_keep_alive(keep_alive_id))
    }

    fn decode_play(
        &self,
        session: &ProtocolSessionSnapshot,
        frame: &[u8],
    ) -> Result<Option<RuntimeCommand>, ProtocolError> {
        decode_play_packet(session, &self.sessions, frame)
    }

    fn observe_event(
        &self,
        session: &ProtocolSessionSnapshot,
        event: &revy_voxel_core::CoreEvent,
    ) -> Result<(), ProtocolError> {
        self.sessions.observe_event(session, event);
        Ok(())
    }

    fn session_closed(&self, session: &ProtocolSessionSnapshot) -> Result<(), ProtocolError> {
        self.sessions.remove_session(session);
        Ok(())
    }

    fn export_session_state(
        &self,
        session: &ProtocolSessionSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.sessions.export_session_state(session)
    }

    fn import_session_state(
        &self,
        session: &ProtocolSessionSnapshot,
        blob: &[u8],
    ) -> Result<(), ProtocolError> {
        self.sessions.import_session_state(session, blob)
    }
}
