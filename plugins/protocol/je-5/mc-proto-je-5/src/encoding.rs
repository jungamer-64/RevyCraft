use crate::{
    PACKET_CB_BLOCK_BREAK_ANIMATION, PACKET_CB_BLOCK_CHANGE, PACKET_CB_CLOSE_WINDOW,
    PACKET_CB_DESTROY_ENTITIES, PACKET_CB_ENTITY_HEAD_ROTATION, PACKET_CB_ENTITY_METADATA,
    PACKET_CB_ENTITY_TELEPORT, PACKET_CB_HELD_ITEM_CHANGE, PACKET_CB_JOIN_GAME,
    PACKET_CB_KEEP_ALIVE, PACKET_CB_MAP_CHUNK, PACKET_CB_MAP_CHUNK_BULK,
    PACKET_CB_NAMED_ENTITY_SPAWN, PACKET_CB_OPEN_WINDOW, PACKET_CB_PLAYER_ABILITIES,
    PACKET_CB_PLAYER_POSITION_AND_LOOK, PACKET_CB_SET_SLOT, PACKET_CB_SPAWN_OBJECT,
    PACKET_CB_SPAWN_POSITION, PACKET_CB_TIME_UPDATE, PACKET_CB_TRANSACTION,
    PACKET_CB_UPDATE_HEALTH, PACKET_CB_WINDOW_ITEMS, PACKET_CB_WINDOW_PROPERTY,
};
use mc_content_api::ContainerKindId;
use mc_core::{EntityId, PlayerSnapshot};
use mc_model::{
    BlockPos, BlockState, ChunkColumn, DimensionId, DroppedItemSnapshot, InventoryWindowContents,
    ItemStack, WorldMeta,
};
use mc_proto_common::{PacketWriter, ProtocolError};
use mc_proto_je_common::__version_support::{
    blocks::legacy_block,
    chunks::{build_chunk_data_1_7, zlib_compress},
    inventory::{signed_window_id, unique_slot_count, window_items, window_type, write_slot},
    metadata::{write_empty_metadata_1_8, write_item_stack_metadata_1_8},
    positions::{to_angle_byte, to_fixed_point},
};

pub(crate) fn encode_join_game(
    entity_id: EntityId,
    world_meta: &WorldMeta,
    player: &PlayerSnapshot,
) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_JOIN_GAME);
    writer.write_i32(entity_id.0);
    writer.write_u8(world_meta.game_mode);
    writer.write_i8(dimension_to_i8(player.dimension));
    writer.write_u8(world_meta.difficulty);
    writer.write_u8(world_meta.max_players);
    let level_type = world_meta.level_type.to_ascii_lowercase();
    let _ = writer.write_string(&level_type);
    writer.into_inner()
}

pub(crate) fn encode_spawn_position(spawn: BlockPos) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_SPAWN_POSITION);
    writer.write_i32(spawn.x);
    writer.write_i32(spawn.y);
    writer.write_i32(spawn.z);
    writer.into_inner()
}

pub(crate) fn encode_time_update(age: i64, time: i64) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_TIME_UPDATE);
    writer.write_i64(age);
    writer.write_i64(time);
    writer.into_inner()
}

pub(crate) fn encode_update_health(player: &PlayerSnapshot) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_UPDATE_HEALTH);
    writer.write_f32(player.health);
    writer.write_i16(player.food);
    writer.write_f32(player.food_saturation);
    writer.into_inner()
}

pub(crate) fn encode_position_and_look(player: &PlayerSnapshot) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_PLAYER_POSITION_AND_LOOK);
    writer.write_f64(player.position.x);
    writer.write_f64(player.position.y);
    writer.write_f64(player.position.z);
    writer.write_f32(player.yaw);
    writer.write_f32(player.pitch);
    writer.write_bool(player.on_ground);
    writer.into_inner()
}

pub(crate) fn encode_held_item_change(slot: u8) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_HELD_ITEM_CHANGE);
    writer.write_i8(i8::try_from(slot).expect("held slot should fit into i8"));
    writer.into_inner()
}

pub(crate) fn encode_player_abilities(creative_mode: bool) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_PLAYER_ABILITIES);
    let flags = if creative_mode { 0x0d } else { 0x00 };
    writer.write_u8(flags);
    writer.write_f32(0.05);
    writer.write_f32(0.1);
    writer.into_inner()
}

pub(crate) fn encode_keep_alive(keep_alive_id: i32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_KEEP_ALIVE);
    writer.write_i32(keep_alive_id);
    writer.into_inner()
}

pub(crate) fn encode_named_entity_spawn(
    entity_id: EntityId,
    player: &PlayerSnapshot,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_NAMED_ENTITY_SPAWN);
    writer.write_varint(entity_id.0);
    writer.write_string(&player.id.0.hyphenated().to_string())?;
    writer.write_string(&player.username)?;
    writer.write_varint(0);
    writer.write_i32(to_fixed_point(player.position.x));
    writer.write_i32(to_fixed_point(player.position.y));
    writer.write_i32(to_fixed_point(player.position.z));
    writer.write_i8(to_angle_byte(player.yaw));
    writer.write_i8(to_angle_byte(player.pitch));
    writer.write_i16(0);
    write_empty_metadata_1_8(&mut writer);
    Ok(writer.into_inner())
}

pub(crate) fn encode_entity_teleport(entity_id: EntityId, player: &PlayerSnapshot) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_ENTITY_TELEPORT);
    writer.write_i32(entity_id.0);
    writer.write_i32(to_fixed_point(player.position.x));
    writer.write_i32(to_fixed_point(player.position.y));
    writer.write_i32(to_fixed_point(player.position.z));
    writer.write_i8(to_angle_byte(player.yaw));
    writer.write_i8(to_angle_byte(player.pitch));
    writer.into_inner()
}

pub(crate) fn encode_entity_head_rotation(entity_id: EntityId, yaw: f32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_ENTITY_HEAD_ROTATION);
    writer.write_i32(entity_id.0);
    writer.write_i8(to_angle_byte(yaw));
    writer.into_inner()
}

pub(crate) fn encode_dropped_item_spawn(
    entity_id: EntityId,
    item: &DroppedItemSnapshot,
) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_SPAWN_OBJECT);
    writer.write_i32(entity_id.0);
    writer.write_u8(2);
    writer.write_i32(to_fixed_point(item.position.x));
    writer.write_i32(to_fixed_point(item.position.y));
    writer.write_i32(to_fixed_point(item.position.z));
    writer.write_i8(0);
    writer.write_i8(0);
    writer.write_i32(1);
    writer.write_i16(0);
    writer.write_i16(0);
    writer.write_i16(0);
    writer.into_inner()
}

pub(crate) fn encode_dropped_item_metadata(
    entity_id: EntityId,
    item: &DroppedItemSnapshot,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_ENTITY_METADATA);
    writer.write_i32(entity_id.0);
    write_item_stack_metadata_1_8(&mut writer, 10, &item.item, crate::INVENTORY_SPEC.slot)?;
    Ok(writer.into_inner())
}

pub(crate) fn encode_destroy_entities(entity_ids: &[EntityId]) -> Result<Vec<u8>, ProtocolError> {
    let count = i8::try_from(entity_ids.len())
        .map_err(|_| ProtocolError::InvalidPacket("too many entities to destroy in one packet"))?;
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_DESTROY_ENTITIES);
    writer.write_i8(count);
    for entity_id in entity_ids {
        writer.write_i32(entity_id.0);
    }
    Ok(writer.into_inner())
}

pub(crate) fn encode_block_change(position: BlockPos, block: &BlockState) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    let (block_id, metadata) = legacy_block(block);
    writer.write_varint(PACKET_CB_BLOCK_CHANGE);
    writer.write_i32(position.x);
    writer.write_u8(u8::try_from(position.y).expect("block change y should fit into u8"));
    writer.write_i32(position.z);
    writer.write_varint(i32::from(block_id));
    writer.write_u8(metadata);
    writer.into_inner()
}

pub(crate) fn encode_block_break_animation(
    entity_id: EntityId,
    position: BlockPos,
    stage: Option<u8>,
) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_BLOCK_BREAK_ANIMATION);
    writer.write_i32(entity_id.0);
    writer.write_i32(position.x);
    writer.write_i32(position.y);
    writer.write_i32(position.z);
    writer.write_i8(stage.map_or(-1, |stage| i8::try_from(stage).unwrap_or(9)));
    writer.into_inner()
}

pub(crate) fn encode_set_slot(
    window_id: i8,
    slot: i16,
    stack: Option<&ItemStack>,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_SET_SLOT);
    writer.write_i8(window_id);
    writer.write_i16(slot);
    write_slot(&mut writer, stack, crate::INVENTORY_SPEC.slot)?;
    Ok(writer.into_inner())
}

pub(crate) fn encode_open_window(
    window_id: u8,
    container: &ContainerKindId,
    title: &str,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_OPEN_WINDOW);
    writer.write_u8(window_id);
    writer.write_string(window_type(container))?;
    writer.write_string(title)?;
    writer.write_u8(unique_slot_count(container));
    writer.write_bool(true);
    Ok(writer.into_inner())
}

pub(crate) fn encode_close_window(window_id: u8) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_CLOSE_WINDOW);
    writer.write_u8(window_id);
    writer.into_inner()
}

pub(crate) fn encode_confirm_transaction(
    window_id: u8,
    action_number: i16,
    accepted: bool,
) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_TRANSACTION);
    writer.write_u8(window_id);
    writer.write_i16(action_number);
    writer.write_bool(accepted);
    writer.into_inner()
}

pub(crate) fn encode_window_property(window_id: u8, property_id: u8, value: i16) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_WINDOW_PROPERTY);
    writer.write_u8(window_id);
    writer.write_i16(i16::from(property_id));
    writer.write_i16(value);
    writer.into_inner()
}

pub(crate) fn encode_window_items(
    window_id: u8,
    container: &ContainerKindId,
    contents: &InventoryWindowContents,
) -> Result<Vec<u8>, ProtocolError> {
    let items = window_items(container, crate::INVENTORY_SPEC.layout, contents);
    let slot_count = i16::try_from(items.len())
        .map_err(|_| ProtocolError::InvalidPacket("too many inventory slots"))?;
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_WINDOW_ITEMS);
    writer.write_i8(signed_window_id(window_id));
    writer.write_i16(slot_count);
    for slot in &items {
        write_slot(&mut writer, slot.as_ref(), crate::INVENTORY_SPEC.slot)?;
    }
    Ok(writer.into_inner())
}

pub(crate) fn encode_chunk(chunk: &ChunkColumn) -> Result<Vec<u8>, ProtocolError> {
    let (bit_map, chunk_data) = build_chunk_data_1_7(chunk, true);
    let compressed = zlib_compress(&chunk_data)?;
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_MAP_CHUNK);
    writer.write_i32(chunk.pos.x);
    writer.write_i32(chunk.pos.z);
    writer.write_bool(true);
    writer.write_u16(bit_map);
    writer.write_u16(0);
    writer.write_i32(
        i32::try_from(compressed.len())
            .map_err(|_| ProtocolError::InvalidPacket("compressed chunk too large"))?,
    );
    writer.write_bytes(&compressed);
    Ok(writer.into_inner())
}

pub(crate) fn encode_chunk_bulk(chunks: &[ChunkColumn]) -> Result<Vec<u8>, ProtocolError> {
    let mut uncompressed = Vec::new();
    let mut meta = Vec::new();
    for chunk in chunks {
        let (bit_map, chunk_data) = build_chunk_data_1_7(chunk, true);
        uncompressed.extend_from_slice(&chunk_data);
        meta.push((chunk.pos.x, chunk.pos.z, bit_map));
    }
    let compressed = zlib_compress(&uncompressed)?;
    let chunk_count = i16::try_from(chunks.len())
        .map_err(|_| ProtocolError::InvalidPacket("too many chunks in bulk packet"))?;
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_MAP_CHUNK_BULK);
    writer.write_i16(chunk_count);
    writer.write_i32(
        i32::try_from(compressed.len())
            .map_err(|_| ProtocolError::InvalidPacket("bulk chunk payload too large"))?,
    );
    writer.write_bool(true);
    writer.write_bytes(&compressed);
    for (x, z, bit_map) in meta {
        writer.write_i32(x);
        writer.write_i32(z);
        writer.write_u16(bit_map);
        writer.write_u16(0);
    }
    Ok(writer.into_inner())
}

const fn dimension_to_i8(dimension: DimensionId) -> i8 {
    match dimension {
        DimensionId::Overworld => 0,
    }
}
