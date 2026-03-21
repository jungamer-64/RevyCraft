#![allow(clippy::multiple_crate_versions)]
mod storage;

use mc_core::{
    BlockFace, BlockPos, BlockState, ChunkColumn, CoreCommand, DimensionId, EntityId,
    InteractionHand, InventoryContainer, InventorySlot, ItemStack, PlayerId, PlayerInventory,
    PlayerSnapshot, Vec3, WorldMeta,
};
use mc_proto_common::{
    Edition, PacketReader, PacketWriter, ProtocolDescriptor, ProtocolError, TransportKind,
    WireFormatKind,
};
use mc_proto_je_common::{
    __version_support::{
        build_chunk_data_1_7, get_nibble as common_get_nibble, legacy_block, legacy_inventory_slot,
        legacy_item, legacy_window_items, legacy_window_slot, player_window_id, read_legacy_slot,
        semantic_block, semantic_item, to_angle_byte, to_fixed_point, write_legacy_slot,
        zlib_compress,
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
            player_window_id(container),
            u8::try_from(protocol_slot).expect("legacy inventory slot should fit into u8"),
            stack,
        )?))
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
        let mut reader = PacketReader::new(frame);
        let packet_id = reader.read_varint()?;
        match packet_id {
            PACKET_SB_KEEP_ALIVE => Ok(Some(CoreCommand::KeepAliveResponse {
                player_id,
                keep_alive_id: reader.read_i32()?,
            })),
            PACKET_SB_FLYING => Ok(Some(CoreCommand::MoveIntent {
                player_id,
                position: None,
                yaw: None,
                pitch: None,
                on_ground: reader.read_bool()?,
            })),
            PACKET_SB_POSITION => Ok(Some(decode_position_packet(player_id, &mut reader)?)),
            PACKET_SB_LOOK => Ok(Some(CoreCommand::MoveIntent {
                player_id,
                position: None,
                yaw: Some(reader.read_f32()?),
                pitch: Some(reader.read_f32()?),
                on_ground: reader.read_bool()?,
            })),
            PACKET_SB_POSITION_LOOK => {
                Ok(Some(decode_position_look_packet(player_id, &mut reader)?))
            }
            PACKET_SB_PLAYER_DIGGING => Ok(Some(decode_digging_packet(player_id, &mut reader)?)),
            PACKET_SB_PLAYER_BLOCK_PLACEMENT => decode_place_block_packet(player_id, &mut reader),
            PACKET_SB_HELD_ITEM_CHANGE => Ok(Some(CoreCommand::SetHeldSlot {
                player_id,
                slot: reader.read_i16()?,
            })),
            PACKET_SB_CREATIVE_INVENTORY_ACTION => {
                let slot = reader.read_i16()?;
                let stack = read_legacy_slot(&mut reader)?;
                Ok(
                    legacy_inventory_slot(slot).map(|slot| CoreCommand::CreativeInventorySet {
                        player_id,
                        slot,
                        stack,
                    }),
                )
            }
            PACKET_SB_SETTINGS => Ok(Some(decode_client_settings_packet(player_id, &mut reader)?)),
            PACKET_SB_CLIENT_COMMAND => Ok(Some(CoreCommand::ClientStatus {
                player_id,
                action_id: reader.read_i8()?,
            })),
            _ => Ok(None),
        }
    }
}

fn encode_join_game(
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

fn encode_spawn_position(spawn: BlockPos) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_SPAWN_POSITION);
    writer.write_i32(spawn.x);
    writer.write_i32(spawn.y);
    writer.write_i32(spawn.z);
    writer.into_inner()
}

fn encode_time_update(age: i64, time: i64) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_TIME_UPDATE);
    writer.write_i64(age);
    writer.write_i64(time);
    writer.into_inner()
}

fn encode_update_health(player: &PlayerSnapshot) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_UPDATE_HEALTH);
    writer.write_f32(player.health);
    writer.write_i16(player.food);
    writer.write_f32(player.food_saturation);
    writer.into_inner()
}

fn encode_position_and_look(player: &PlayerSnapshot) -> Vec<u8> {
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

fn encode_held_item_change(slot: u8) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_HELD_ITEM_CHANGE);
    writer.write_i8(i8::try_from(slot).expect("held slot should fit into i8"));
    writer.into_inner()
}

fn encode_player_abilities(creative_mode: bool) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_PLAYER_ABILITIES);
    let flags = if creative_mode { 0x0d } else { 0x00 };
    writer.write_u8(flags);
    writer.write_f32(0.05);
    writer.write_f32(0.1);
    writer.into_inner()
}

fn encode_keep_alive(keep_alive_id: i32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_KEEP_ALIVE);
    writer.write_i32(keep_alive_id);
    writer.into_inner()
}

fn encode_named_entity_spawn(
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
    writer.write_u8(0x7f);
    Ok(writer.into_inner())
}

fn encode_entity_teleport(entity_id: EntityId, player: &PlayerSnapshot) -> Vec<u8> {
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

fn encode_entity_head_rotation(entity_id: EntityId, yaw: f32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_ENTITY_HEAD_ROTATION);
    writer.write_i32(entity_id.0);
    writer.write_i8(to_angle_byte(yaw));
    writer.into_inner()
}

fn encode_destroy_entities(entity_ids: &[EntityId]) -> Result<Vec<u8>, ProtocolError> {
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

fn encode_block_change(position: BlockPos, block: &BlockState) -> Vec<u8> {
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

fn encode_set_slot(
    window_id: u8,
    slot: u8,
    stack: Option<&ItemStack>,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_SET_SLOT);
    writer.write_i8(i8::from_be_bytes([window_id]));
    writer.write_i16(i16::from(slot));
    write_legacy_slot(&mut writer, stack)?;
    Ok(writer.into_inner())
}

fn encode_window_items(
    window_id: u8,
    inventory: &PlayerInventory,
) -> Result<Vec<u8>, ProtocolError> {
    let slot_count = i16::try_from(inventory.slots.len())
        .map_err(|_| ProtocolError::InvalidPacket("too many inventory slots"))?;
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_WINDOW_ITEMS);
    writer.write_i8(i8::from_be_bytes([window_id]));
    writer.write_i16(slot_count);
    for slot in &legacy_window_items(inventory) {
        write_legacy_slot(&mut writer, slot.as_ref())?;
    }
    Ok(writer.into_inner())
}

fn build_chunk_data(chunk: &ChunkColumn, include_biomes: bool) -> (u16, Vec<u8>) {
    build_chunk_data_1_7(chunk, include_biomes)
}

fn encode_chunk(chunk: &ChunkColumn) -> Result<Vec<u8>, ProtocolError> {
    let (bit_map, chunk_data) = build_chunk_data(chunk, true);
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

fn encode_chunk_bulk(chunks: &[ChunkColumn]) -> Result<Vec<u8>, ProtocolError> {
    let mut uncompressed = Vec::new();
    let mut meta = Vec::new();
    for chunk in chunks {
        let (bit_map, chunk_data) = build_chunk_data(chunk, true);
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

pub(crate) fn get_nibble(source: &[u8], index: usize) -> u8 {
    common_get_nibble(source, index)
}

const fn dimension_to_i8(dimension: DimensionId) -> i8 {
    match dimension {
        DimensionId::Overworld => 0,
    }
}

const fn i8_to_u8(value: i8) -> u8 {
    if value.is_negative() {
        0
    } else {
        value.cast_unsigned()
    }
}

fn decode_position_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<CoreCommand, ProtocolError> {
    let x = reader.read_f64()?;
    let _stance = reader.read_f64()?;
    let y = reader.read_f64()?;
    let z = reader.read_f64()?;
    let on_ground = reader.read_bool()?;
    Ok(CoreCommand::MoveIntent {
        player_id,
        position: Some(Vec3::new(x, y, z)),
        yaw: None,
        pitch: None,
        on_ground,
    })
}

fn decode_position_look_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<CoreCommand, ProtocolError> {
    let x = reader.read_f64()?;
    let _stance = reader.read_f64()?;
    let y = reader.read_f64()?;
    let z = reader.read_f64()?;
    let yaw = reader.read_f32()?;
    let pitch = reader.read_f32()?;
    let on_ground = reader.read_bool()?;
    Ok(CoreCommand::MoveIntent {
        player_id,
        position: Some(Vec3::new(x, y, z)),
        yaw: Some(yaw),
        pitch: Some(pitch),
        on_ground,
    })
}

fn decode_digging_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<CoreCommand, ProtocolError> {
    Ok(CoreCommand::DigBlock {
        player_id,
        status: reader.read_u8()?,
        position: BlockPos::new(
            reader.read_i32()?,
            i32::from(reader.read_u8()?),
            reader.read_i32()?,
        ),
        face: BlockFace::from_protocol_byte(reader.read_u8()?),
    })
}

fn decode_place_block_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<Option<CoreCommand>, ProtocolError> {
    let position = BlockPos::new(
        reader.read_i32()?,
        i32::from(reader.read_u8()?),
        reader.read_i32()?,
    );
    let direction = reader.read_u8()?;
    let held_item = read_legacy_slot(reader)?;
    let _cursor_x = reader.read_u8()?;
    let _cursor_y = reader.read_u8()?;
    let _cursor_z = reader.read_u8()?;
    if position.x == -1 && position.z == -1 && position.y == 255 && direction == 255 {
        return Ok(None);
    }
    Ok(Some(CoreCommand::PlaceBlock {
        player_id,
        hand: InteractionHand::Main,
        position,
        face: BlockFace::from_protocol_byte(direction),
        held_item,
    }))
}

fn decode_client_settings_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<CoreCommand, ProtocolError> {
    let _locale = reader.read_string(16)?;
    let view_distance = i8_to_u8(reader.read_i8()?);
    let _chat_flags = reader.read_i8()?;
    let _chat_colors = reader.read_bool()?;
    let _difficulty = reader.read_u8()?;
    let _show_cape = reader.read_bool()?;
    Ok(CoreCommand::UpdateClientView {
        player_id,
        view_distance: view_distance.max(1),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        JE_1_7_10_ADAPTER_ID, Je1710Adapter, PROTOCOL_VERSION_1_7_10, VERSION_NAME_1_7_10,
        get_nibble, legacy_block,
    };
    use mc_core::{
        BlockState, ChunkColumn, ChunkPos, ConnectionId, CoreCommand, CoreConfig, CoreEvent,
        InventoryContainer, InventorySlot, PlayerId, PlayerInventory, PlayerSnapshot, ServerCore,
        Vec3,
    };
    use mc_proto_common::{
        Edition, HandshakeProbe, LoginRequest, PacketWriter, PlayEncodingContext, PlaySyncAdapter,
        ProtocolAdapter, ProtocolDescriptor, ServerListStatus, SessionAdapter, StatusRequest,
        TransportKind, WireFormatKind,
    };
    use uuid::Uuid;

    fn player_snapshot(name: &str) -> PlayerSnapshot {
        PlayerSnapshot {
            id: PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, name.as_bytes())),
            username: name.to_string(),
            position: Vec3::new(0.5, 4.0, 0.5),
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            dimension: mc_core::DimensionId::Overworld,
            health: 20.0,
            food: 20,
            food_saturation: 5.0,
            inventory: PlayerInventory::creative_starter(),
            selected_hotbar_slot: 0,
        }
    }

    #[test]
    fn decodes_handshake_status_and_login_packets() {
        let adapter = Je1710Adapter::new();

        let handshake = [
            0x00, 0x05, 0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', 0x63, 0xdd,
            0x02,
        ];
        let intent = adapter
            .try_route(&handshake)
            .expect("handshake should decode");
        let intent = intent.expect("handshake should match JE");
        assert_eq!(intent.protocol_number, PROTOCOL_VERSION_1_7_10);
        assert_eq!(intent.edition, Edition::Je);

        let status = adapter
            .decode_status(&[0x00])
            .expect("status query should decode");
        assert_eq!(status, StatusRequest::Query);

        let login = adapter
            .decode_login(&[0x00, 0x04, b't', b'e', b's', b't'])
            .expect("login start should decode");
        assert_eq!(
            login,
            LoginRequest::LoginStart {
                username: "test".to_string()
            }
        );

        let encryption_response = adapter
            .decode_login(&[0x01, 0x03, 1, 2, 3, 0x02, 4, 5])
            .expect("encryption response should decode");
        assert_eq!(
            encryption_response,
            LoginRequest::EncryptionResponse {
                shared_secret_encrypted: vec![1, 2, 3],
                verify_token_encrypted: vec![4, 5],
            }
        );
    }

    #[test]
    fn encodes_status_and_login_events() {
        let adapter = Je1710Adapter::new();
        assert_eq!(adapter.descriptor().adapter_id, JE_1_7_10_ADAPTER_ID);
        assert_eq!(adapter.transport_kind(), TransportKind::Tcp);
        let status_packet = adapter
            .encode_status_response(&ServerListStatus {
                version: ProtocolDescriptor {
                    adapter_id: JE_1_7_10_ADAPTER_ID.to_string(),
                    transport: TransportKind::Tcp,
                    wire_format: WireFormatKind::MinecraftFramed,
                    edition: Edition::Je,
                    version_name: VERSION_NAME_1_7_10.to_string(),
                    protocol_number: PROTOCOL_VERSION_1_7_10,
                },
                players_online: 1,
                max_players: 20,
                description: "hello".to_string(),
            })
            .expect("status should encode");
        assert_eq!(status_packet[0], 0x00);

        let player = player_snapshot("alpha");
        let login_packet = adapter
            .encode_login_success(&player)
            .expect("login event should encode");
        assert_eq!(login_packet[0], 0x02);

        let encryption_request = adapter
            .encode_encryption_request("", &[1, 2, 3], &[4, 5])
            .expect("encryption request should encode");
        assert_eq!(encryption_request[0], 0x01);
    }

    #[test]
    fn decodes_play_packets_into_core_commands() {
        let adapter = Je1710Adapter::new();
        let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"decode-play"));
        let mut writer = PacketWriter::default();
        writer.write_varint(0x04);
        writer.write_f64(42.0);
        writer.write_f64(43.62);
        writer.write_f64(43.0);
        writer.write_f64(10.0);
        writer.write_bool(true);

        let command = adapter
            .decode_play(player_id, &writer.into_inner())
            .expect("position should decode")
            .expect("position should produce a command");
        assert!(matches!(
            command,
            CoreCommand::MoveIntent {
                position: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn decodes_inventory_and_edit_packets_into_core_commands() {
        let adapter = Je1710Adapter::new();
        let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"decode-edit"));

        let mut held_item = PacketWriter::default();
        held_item.write_varint(0x09);
        held_item.write_i16(4);
        let command = adapter
            .decode_play(player_id, &held_item.into_inner())
            .expect("held item change should decode")
            .expect("held item change should produce command");
        assert!(matches!(command, CoreCommand::SetHeldSlot { slot: 4, .. }));

        let mut settings = PacketWriter::default();
        settings.write_varint(0x15);
        let _ = settings.write_string("ja_JP");
        settings.write_i8(7);
        settings.write_i8(0);
        settings.write_bool(true);
        settings.write_u8(1);
        settings.write_bool(true);
        let command = adapter
            .decode_play(player_id, &settings.into_inner())
            .expect("settings should decode")
            .expect("settings should produce command");
        assert!(matches!(
            command,
            CoreCommand::UpdateClientView {
                view_distance: 7,
                ..
            }
        ));

        let mut creative_inventory = PacketWriter::default();
        creative_inventory.write_varint(0x10);
        creative_inventory.write_i16(36);
        creative_inventory.write_i16(20);
        creative_inventory.write_u8(64);
        creative_inventory.write_i16(0);
        creative_inventory.write_i16(-1);
        let command = adapter
            .decode_play(player_id, &creative_inventory.into_inner())
            .expect("creative inventory should decode")
            .expect("creative inventory should produce command");
        assert!(matches!(
            command,
            CoreCommand::CreativeInventorySet {
                slot: InventorySlot::Hotbar(0),
                stack: Some(ref stack),
                ..
            }
                if stack.key.as_str() == "minecraft:glass"
        ));

        let mut placement = PacketWriter::default();
        placement.write_varint(0x08);
        placement.write_i32(2);
        placement.write_u8(3);
        placement.write_i32(0);
        placement.write_u8(1);
        placement.write_i16(1);
        placement.write_u8(64);
        placement.write_i16(0);
        placement.write_i16(-1);
        placement.write_u8(8);
        placement.write_u8(8);
        placement.write_u8(8);
        let command = adapter
            .decode_play(player_id, &placement.into_inner())
            .expect("placement should decode")
            .expect("placement should produce command");
        assert!(matches!(
            command,
            CoreCommand::PlaceBlock {
                position: mc_core::BlockPos { x: 2, y: 3, z: 0 },
                face: Some(mc_core::BlockFace::Top),
                held_item: Some(ref stack),
                ..
            } if stack.key.as_str() == "minecraft:stone"
        ));
    }

    #[test]
    fn chunk_encoding_uses_legacy_block_layout() {
        let mut chunk = ChunkColumn::new(ChunkPos::new(0, 0));
        chunk.set_block(0, 0, 0, BlockState::bedrock());
        chunk.set_block(1, 0, 0, BlockState::stone());
        let (_, data) = super::build_chunk_data(&chunk, true);
        assert_eq!(data[0], 7);
        assert_eq!(data[1], 1);
        assert_eq!(get_nibble(&data[4096..6144], 0), 0);
        assert_eq!(legacy_block(&BlockState::grass_block()), (2, 0));
    }

    #[test]
    fn play_bootstrap_and_chunk_batch_emit_join_game_and_chunks() {
        let adapter = Je1710Adapter::new();
        let mut core = ServerCore::new(CoreConfig::default());
        let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"initial-world"));
        let events = core.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(1),
                username: "alpha".to_string(),
                player_id,
            },
            0,
        );

        let mut play_bootstrap = None;
        let mut chunk_batch = None;
        for event in events {
            let core_event = event.event;
            match core_event {
                CoreEvent::PlayBootstrap { .. } if play_bootstrap.is_none() => {
                    play_bootstrap = Some(core_event);
                }
                CoreEvent::ChunkBatch { .. } if chunk_batch.is_none() => {
                    chunk_batch = Some(core_event);
                }
                _ => {}
            }
        }
        let play_bootstrap = play_bootstrap.expect("play bootstrap event should exist");
        let chunk_batch = chunk_batch.expect("chunk batch event should exist");

        let context = PlayEncodingContext {
            player_id,
            entity_id: mc_core::EntityId(1),
        };
        let bootstrap_packets = adapter
            .encode_play_event(&play_bootstrap, &context)
            .expect("play bootstrap should encode");
        let chunk_packets = adapter
            .encode_play_event(&chunk_batch, &context)
            .expect("chunk batch should encode");

        assert!(bootstrap_packets.iter().any(|packet| packet[0] == 0x01));
        assert!(chunk_packets.iter().any(|packet| packet[0] == 0x26));
        assert!(bootstrap_packets.iter().any(|packet| packet[0] == 0x39));
    }

    #[test]
    fn encodes_inventory_and_block_events() {
        let adapter = Je1710Adapter::new();
        let context = PlayEncodingContext {
            player_id: PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"encode-play-events")),
            entity_id: mc_core::EntityId(1),
        };
        let inventory = PlayerInventory::creative_starter();
        let packets = adapter
            .encode_play_event(
                &CoreEvent::InventoryContents {
                    container: InventoryContainer::Player,
                    inventory,
                },
                &context,
            )
            .expect("inventory snapshot should encode");
        assert_eq!(packets[0][0], 0x30);

        let packets = adapter
            .encode_play_event(&CoreEvent::SelectedHotbarSlotChanged { slot: 4 }, &context)
            .expect("held slot change should encode");
        assert_eq!(packets[0][0], 0x09);

        let packets = adapter
            .encode_play_event(
                &CoreEvent::BlockChanged {
                    position: mc_core::BlockPos::new(2, 4, 0),
                    block: BlockState::glass(),
                },
                &context,
            )
            .expect("block change should encode");
        assert_eq!(packets[0][0], 0x23);
    }
}
