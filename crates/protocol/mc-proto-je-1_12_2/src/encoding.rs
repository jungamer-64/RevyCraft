use crate::{
    PACKET_CB_BLOCK_CHANGE, PACKET_CB_DESTROY_ENTITIES, PACKET_CB_ENTITY_HEAD_ROTATION,
    PACKET_CB_ENTITY_TELEPORT, PACKET_CB_HELD_ITEM_CHANGE, PACKET_CB_JOIN_GAME,
    PACKET_CB_KEEP_ALIVE, PACKET_CB_MAP_CHUNK, PACKET_CB_NAMED_ENTITY_SPAWN,
    PACKET_CB_PLAYER_ABILITIES, PACKET_CB_PLAYER_POSITION_AND_LOOK, PACKET_CB_SET_SLOT,
    PACKET_CB_SPAWN_POSITION, PACKET_CB_TIME_UPDATE, PACKET_CB_UPDATE_HEALTH,
    PACKET_CB_WINDOW_ITEMS,
};
use mc_core::{
    BlockPos, ChunkColumn, DimensionId, EntityId, ItemStack, PlayerInventory, PlayerSnapshot,
    WorldMeta,
};
use mc_proto_common::{PacketWriter, ProtocolError};
use mc_proto_je_common::__version_support::{
    blocks::legacy_block_state_id,
    chunks::build_chunk_data_1_12,
    inventory::{modern_window_items, write_legacy_slot},
    metadata::write_empty_metadata_1_12,
    positions::{pack_block_position, to_angle_byte},
};

pub fn encode_join_game(
    entity_id: EntityId,
    world_meta: &WorldMeta,
    player: &PlayerSnapshot,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_JOIN_GAME);
    writer.write_i32(entity_id.0);
    writer.write_u8(world_meta.game_mode);
    writer.write_i32(dimension_to_i32(player.dimension));
    writer.write_u8(world_meta.difficulty);
    writer.write_u8(world_meta.max_players);
    writer.write_string(&world_meta.level_type.to_ascii_lowercase())?;
    writer.write_bool(false);
    Ok(writer.into_inner())
}

pub fn encode_spawn_position(spawn: BlockPos) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_SPAWN_POSITION);
    writer.write_i64(pack_block_position(spawn));
    writer.into_inner()
}

pub fn encode_time_update(age: i64, time: i64) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_TIME_UPDATE);
    writer.write_i64(age);
    writer.write_i64(time);
    writer.into_inner()
}

pub fn encode_update_health(player: &PlayerSnapshot) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_UPDATE_HEALTH);
    writer.write_f32(player.health);
    writer.write_varint(i32::from(player.food));
    writer.write_f32(player.food_saturation);
    writer.into_inner()
}

pub fn encode_position_and_look(player: &PlayerSnapshot) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_PLAYER_POSITION_AND_LOOK);
    writer.write_f64(player.position.x);
    writer.write_f64(player.position.y);
    writer.write_f64(player.position.z);
    writer.write_f32(player.yaw);
    writer.write_f32(player.pitch);
    writer.write_i8(0);
    writer.write_varint(0);
    writer.into_inner()
}

pub fn encode_held_item_change(slot: u8) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_HELD_ITEM_CHANGE);
    writer.write_i8(i8::try_from(slot).expect("held slot should fit into i8"));
    writer.into_inner()
}

pub fn encode_player_abilities(creative_mode: bool) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_PLAYER_ABILITIES);
    let flags = if creative_mode { 0x0d } else { 0x00 };
    writer.write_u8(flags);
    writer.write_f32(0.05);
    writer.write_f32(0.1);
    writer.into_inner()
}

pub fn encode_keep_alive(keep_alive_id: i32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_KEEP_ALIVE);
    writer.write_i64(i64::from(keep_alive_id));
    writer.into_inner()
}

pub fn encode_named_entity_spawn(entity_id: EntityId, player: &PlayerSnapshot) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_NAMED_ENTITY_SPAWN);
    writer.write_varint(entity_id.0);
    writer.write_bytes(player.id.0.as_bytes());
    writer.write_f64(player.position.x);
    writer.write_f64(player.position.y);
    writer.write_f64(player.position.z);
    writer.write_i8(to_angle_byte(player.yaw));
    writer.write_i8(to_angle_byte(player.pitch));
    write_empty_metadata_1_12(&mut writer);
    writer.into_inner()
}

pub fn encode_entity_teleport(entity_id: EntityId, player: &PlayerSnapshot) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_ENTITY_TELEPORT);
    writer.write_varint(entity_id.0);
    writer.write_f64(player.position.x);
    writer.write_f64(player.position.y);
    writer.write_f64(player.position.z);
    writer.write_i8(to_angle_byte(player.yaw));
    writer.write_i8(to_angle_byte(player.pitch));
    writer.write_bool(player.on_ground);
    writer.into_inner()
}

pub fn encode_entity_head_rotation(entity_id: EntityId, yaw: f32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_ENTITY_HEAD_ROTATION);
    writer.write_varint(entity_id.0);
    writer.write_i8(to_angle_byte(yaw));
    writer.into_inner()
}

pub fn encode_destroy_entities(entity_ids: &[EntityId]) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_DESTROY_ENTITIES);
    writer.write_varint(
        i32::try_from(entity_ids.len()).map_err(|_| {
            ProtocolError::InvalidPacket("too many entities to destroy in one packet")
        })?,
    );
    for entity_id in entity_ids {
        writer.write_varint(entity_id.0);
    }
    Ok(writer.into_inner())
}

pub fn encode_block_change(position: BlockPos, block: &mc_core::BlockState) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_BLOCK_CHANGE);
    writer.write_i64(pack_block_position(position));
    writer.write_varint(legacy_block_state_id(block));
    writer.into_inner()
}

pub fn encode_set_slot(
    window_id: i8,
    slot: i16,
    stack: Option<&ItemStack>,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_SET_SLOT);
    writer.write_i8(window_id);
    writer.write_i16(slot);
    write_legacy_slot(&mut writer, stack)?;
    Ok(writer.into_inner())
}

pub fn encode_window_items(
    window_id: u8,
    inventory: &PlayerInventory,
) -> Result<Vec<u8>, ProtocolError> {
    let items = modern_window_items(inventory);
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_WINDOW_ITEMS);
    writer.write_u8(window_id);
    writer.write_i16(
        i16::try_from(items.len())
            .map_err(|_| ProtocolError::InvalidPacket("too many inventory slots"))?,
    );
    for item in &items {
        write_legacy_slot(&mut writer, item.as_ref())?;
    }
    Ok(writer.into_inner())
}

pub fn encode_chunk(chunk: &ChunkColumn) -> Result<Vec<u8>, ProtocolError> {
    let (bit_map, chunk_data) = build_chunk_data_1_12(chunk, true);
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_MAP_CHUNK);
    writer.write_i32(chunk.pos.x);
    writer.write_i32(chunk.pos.z);
    writer.write_bool(true);
    writer.write_varint(i32::from(bit_map));
    writer.write_varint(
        i32::try_from(chunk_data.len())
            .map_err(|_| ProtocolError::InvalidPacket("chunk payload too large"))?,
    );
    writer.write_bytes(&chunk_data);
    writer.write_varint(0);
    Ok(writer.into_inner())
}
const fn dimension_to_i32(dimension: DimensionId) -> i32 {
    match dimension {
        DimensionId::Overworld => 0,
    }
}
