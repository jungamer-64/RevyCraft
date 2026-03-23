#![allow(clippy::multiple_crate_versions)]
mod decoding;
mod encoding;
mod storage;

#[cfg(test)]
mod tests;

use decoding::decode_play_packet;
use encoding::{
    encode_block_change, encode_chunk, encode_chunk_bulk, encode_confirm_transaction,
    encode_destroy_entities, encode_entity_head_rotation, encode_entity_teleport,
    encode_held_item_change, encode_join_game, encode_keep_alive, encode_named_entity_spawn,
    encode_player_abilities, encode_position_and_look, encode_set_slot, encode_spawn_position,
    encode_time_update, encode_update_health, encode_window_items,
};
use mc_core::{
    BlockPos, BlockState, ChunkColumn, CoreCommand, EntityId, InventoryContainer, InventorySlot,
    InventoryTransactionContext, ItemStack, PlayerId, PlayerInventory, PlayerSnapshot, WorldMeta,
};
use mc_proto_common::{Edition, ProtocolDescriptor, ProtocolError, TransportKind, WireFormatKind};
use mc_proto_je_common::{
    __version_support::inventory::{
        CURSOR_SLOT_ID, CURSOR_WINDOW_ID, legacy_window_slot, player_window_id,
        player_window_id_signed,
    },
    JavaEditionAdapter, JavaEditionProfile,
};

pub use self::storage::Je1710StorageAdapter;

const PROTOCOL_VERSION_1_7_10: i32 = 5;
const VERSION_NAME_1_7_10: &str = "1.7.10";
pub const JE_1_7_10_ADAPTER_ID: &str = "je-1_7_10";
pub const JE_1_7_10_STORAGE_PROFILE_ID: &str = "je-anvil-1_7_10";

const PACKET_CB_KEEP_ALIVE: i32 = 0x00;
const PACKET_CB_JOIN_GAME: i32 = 0x01;
const PACKET_CB_TIME_UPDATE: i32 = 0x03;
const PACKET_CB_SPAWN_POSITION: i32 = 0x05;
const PACKET_CB_UPDATE_HEALTH: i32 = 0x06;
const PACKET_CB_PLAYER_POSITION_AND_LOOK: i32 = 0x08;
const PACKET_CB_HELD_ITEM_CHANGE: i32 = 0x09;
const PACKET_CB_NAMED_ENTITY_SPAWN: i32 = 0x0c;
const PACKET_CB_DESTROY_ENTITIES: i32 = 0x13;
const PACKET_CB_ENTITY_TELEPORT: i32 = 0x18;
const PACKET_CB_ENTITY_HEAD_ROTATION: i32 = 0x19;
const PACKET_CB_MAP_CHUNK: i32 = 0x21;
const PACKET_CB_BLOCK_CHANGE: i32 = 0x23;
const PACKET_CB_MAP_CHUNK_BULK: i32 = 0x26;
const PACKET_CB_SET_SLOT: i32 = 0x2f;
const PACKET_CB_WINDOW_ITEMS: i32 = 0x30;
const PACKET_CB_TRANSACTION: i32 = 0x32;
const PACKET_CB_PLAYER_ABILITIES: i32 = 0x39;
const PACKET_CB_PLAY_DISCONNECT: i32 = 0x40;

const PACKET_SB_KEEP_ALIVE: i32 = 0x00;
const PACKET_SB_FLYING: i32 = 0x03;
const PACKET_SB_POSITION: i32 = 0x04;
const PACKET_SB_LOOK: i32 = 0x05;
const PACKET_SB_POSITION_LOOK: i32 = 0x06;
const PACKET_SB_PLAYER_DIGGING: i32 = 0x07;
const PACKET_SB_PLAYER_BLOCK_PLACEMENT: i32 = 0x08;
const PACKET_SB_HELD_ITEM_CHANGE: i32 = 0x09;
const PACKET_SB_CLICK_WINDOW: i32 = 0x0e;
const PACKET_SB_CONFIRM_TRANSACTION: i32 = 0x0f;
const PACKET_SB_CREATIVE_INVENTORY_ACTION: i32 = 0x10;
const PACKET_SB_SETTINGS: i32 = 0x15;
const PACKET_SB_CLIENT_COMMAND: i32 = 0x16;

#[derive(Default)]
pub struct Je1710Profile;

pub type Je1710Adapter = JavaEditionAdapter<Je1710Profile>;

impl JavaEditionProfile for Je1710Profile {
    fn adapter_id(&self) -> &'static str {
        JE_1_7_10_ADAPTER_ID
    }

    fn descriptor(&self) -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: JE_1_7_10_ADAPTER_ID.to_string(),
            transport: TransportKind::Tcp,
            wire_format: WireFormatKind::MinecraftFramed,
            edition: Edition::Je,
            version_name: VERSION_NAME_1_7_10.to_string(),
            protocol_number: PROTOCOL_VERSION_1_7_10,
        }
    }

    fn play_disconnect_packet_id(&self) -> i32 {
        PACKET_CB_PLAY_DISCONNECT
    }

    fn format_disconnect_reason(&self, reason: &str) -> String {
        reason.to_string()
    }

    fn encode_play_bootstrap(
        &self,
        entity_id: EntityId,
        world_meta: &WorldMeta,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        Ok(vec![
            encode_join_game(entity_id, world_meta, player),
            encode_spawn_position(world_meta.spawn),
            encode_time_update(world_meta.age, world_meta.time),
            encode_update_health(player),
            encode_player_abilities(world_meta.game_mode == 1),
            encode_position_and_look(player),
        ])
    }

    fn encode_chunk_batch(&self, chunks: &[ChunkColumn]) -> Result<Vec<Vec<u8>>, ProtocolError> {
        match chunks.len() {
            0 => Ok(Vec::new()),
            1 => Ok(vec![encode_chunk(&chunks[0])?]),
            _ => Ok(vec![encode_chunk_bulk(chunks)?]),
        }
    }

    fn encode_entity_spawn(
        &self,
        entity_id: EntityId,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        Ok(vec![
            encode_named_entity_spawn(entity_id, player)?,
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

    fn encode_entity_despawn(&self, entity_ids: &[EntityId]) -> Result<Vec<u8>, ProtocolError> {
        encode_destroy_entities(entity_ids)
    }

    fn encode_inventory_contents(
        &self,
        container: InventoryContainer,
        inventory: &PlayerInventory,
    ) -> Result<Vec<u8>, ProtocolError> {
        encode_window_items(player_window_id(container), inventory)
    }

    fn encode_inventory_slot_changed(
        &self,
        container: InventoryContainer,
        slot: InventorySlot,
        stack: Option<&ItemStack>,
    ) -> Result<Option<Vec<u8>>, ProtocolError> {
        let Some(protocol_slot) = legacy_window_slot(slot) else {
            return Ok(None);
        };
        Ok(Some(encode_set_slot(
            player_window_id_signed(container),
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

    fn encode_keep_alive_requested(&self, keep_alive_id: i32) -> Result<Vec<u8>, ProtocolError> {
        Ok(encode_keep_alive(keep_alive_id))
    }

    fn decode_play(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError> {
        decode_play_packet(player_id, frame)
    }
}
