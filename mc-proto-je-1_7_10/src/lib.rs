mod storage;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use mc_core::catalog::{
    BEDROCK, BRICKS, COBBLESTONE, DIRT, GLASS, GRASS_BLOCK, OAK_PLANKS, SAND, SANDSTONE, STONE,
};
use mc_core::{
    BlockFace, BlockPos, BlockState, ChunkColumn, CoreCommand, CoreEvent, DimensionId, EntityId,
    InventoryContainer, ItemStack, PlayerId, PlayerInventory, PlayerSnapshot, ProtocolVersion,
    Vec3, WorldMeta,
};
use mc_proto_common::{
    ConnectionPhase, HandshakeIntent, HandshakeNextState, LoginRequest, MinecraftWireCodec,
    PacketReader, PacketWriter, PlayEncodingContext, PlaySyncAdapter, ProtocolAdapter,
    ProtocolError, ServerListStatus, SessionAdapter, StatusRequest, StorageAdapter, WireCodec,
};
use num_traits::ToPrimitive;
use serde_json::json;
use std::io::Write;

pub use self::storage::Je1710StorageAdapter;

const PROTOCOL_VERSION_1_7_10: i32 = 5;
const VERSION_NAME_1_7_10: &str = "1.7.10";

const PACKET_HANDSHAKE: i32 = 0x00;
const PACKET_STATUS_REQUEST: i32 = 0x00;
const PACKET_STATUS_PING: i32 = 0x01;
const PACKET_LOGIN_START: i32 = 0x00;
const PACKET_LOGIN_ENCRYPTION_RESPONSE: i32 = 0x01;

const PACKET_CB_STATUS_RESPONSE: i32 = 0x00;
const PACKET_CB_STATUS_PONG: i32 = 0x01;
const PACKET_CB_LOGIN_DISCONNECT: i32 = 0x00;
const PACKET_CB_LOGIN_SUCCESS: i32 = 0x02;

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
pub struct Je1710Adapter {
    codec: MinecraftWireCodec,
    storage: Je1710StorageAdapter,
}

impl Je1710Adapter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionAdapter for Je1710Adapter {
    fn wire_codec(&self) -> &dyn WireCodec {
        &self.codec
    }

    fn decode_handshake(&self, frame: &[u8]) -> Result<HandshakeIntent, ProtocolError> {
        let mut reader = PacketReader::new(frame);
        let packet_id = reader.read_varint()?;
        if packet_id != PACKET_HANDSHAKE {
            return Err(ProtocolError::UnsupportedPacket(packet_id));
        }
        let protocol_version = ProtocolVersion(reader.read_varint()?);
        let server_host = reader.read_string(255)?;
        let server_port = reader.read_u16()?;
        let next_state = match reader.read_varint()? {
            1 => HandshakeNextState::Status,
            2 => HandshakeNextState::Login,
            _ => {
                return Err(ProtocolError::InvalidPacket(
                    "unsupported handshake next state",
                ));
            }
        };
        Ok(HandshakeIntent {
            protocol_version,
            server_host,
            server_port,
            next_state,
        })
    }

    fn decode_status(&self, frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        let mut reader = PacketReader::new(frame);
        match reader.read_varint()? {
            PACKET_STATUS_REQUEST => Ok(StatusRequest::Query),
            PACKET_STATUS_PING => Ok(StatusRequest::Ping {
                payload: reader.read_i64()?,
            }),
            packet_id => Err(ProtocolError::UnsupportedPacket(packet_id)),
        }
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        let mut reader = PacketReader::new(frame);
        match reader.read_varint()? {
            PACKET_LOGIN_START => Ok(LoginRequest::LoginStart {
                username: reader.read_string(16)?,
            }),
            PACKET_LOGIN_ENCRYPTION_RESPONSE => Ok(LoginRequest::EncryptionResponse),
            packet_id => Err(ProtocolError::UnsupportedPacket(packet_id)),
        }
    }

    fn encode_status_response(&self, status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        let payload = json!({
            "version": {
                "name": status.version_name,
                "protocol": status.protocol.0,
            },
            "players": {
                "max": status.max_players,
                "online": status.players_online,
                "sample": [],
            },
            "description": {
                "text": status.description,
            }
        });
        let mut writer = PacketWriter::default();
        writer.write_varint(PACKET_CB_STATUS_RESPONSE);
        writer.write_string(&payload.to_string())?;
        Ok(writer.into_inner())
    }

    fn encode_status_pong(&self, payload: i64) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = PacketWriter::default();
        writer.write_varint(PACKET_CB_STATUS_PONG);
        writer.write_i64(payload);
        Ok(writer.into_inner())
    }

    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = PacketWriter::default();
        let packet_id = match phase {
            ConnectionPhase::Login => PACKET_CB_LOGIN_DISCONNECT,
            ConnectionPhase::Play => PACKET_CB_PLAY_DISCONNECT,
            _ => {
                return Err(ProtocolError::InvalidPacket(
                    "disconnect only valid in login/play",
                ));
            }
        };
        writer.write_varint(packet_id);
        writer.write_string(reason)?;
        Ok(writer.into_inner())
    }

    fn encode_login_success(&self, player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
        encode_login_success(player)
    }
}

impl PlaySyncAdapter for Je1710Adapter {
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
            PACKET_SB_CREATIVE_INVENTORY_ACTION => Ok(Some(CoreCommand::CreativeInventorySet {
                player_id,
                slot: reader.read_i16()?,
                stack: read_slot(&mut reader)?,
            })),
            PACKET_SB_SETTINGS => Ok(Some(decode_client_settings_packet(player_id, &mut reader)?)),
            PACKET_SB_CLIENT_COMMAND => Ok(Some(CoreCommand::ClientStatus {
                player_id,
                action_id: reader.read_i8()?,
            })),
            _ => Ok(None),
        }
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
            } => Ok(vec![
                encode_join_game(*entity_id, world_meta, player),
                encode_spawn_position(world_meta.spawn),
                encode_time_update(world_meta.age, world_meta.time),
                encode_update_health(player),
                encode_player_abilities(world_meta.game_mode == 1),
                encode_position_and_look(player),
            ]),
            CoreEvent::ChunkBatch { chunks } => match chunks.len() {
                0 => Ok(Vec::new()),
                1 => Ok(vec![encode_chunk(&chunks[0])?]),
                _ => Ok(vec![encode_chunk_bulk(chunks)?]),
            },
            CoreEvent::EntitySpawned { entity_id, player } => Ok(vec![
                encode_named_entity_spawn(*entity_id, player)?,
                encode_entity_head_rotation(*entity_id, player.yaw),
            ]),
            CoreEvent::EntityMoved { entity_id, player } => Ok(vec![
                encode_entity_teleport(*entity_id, player),
                encode_entity_head_rotation(*entity_id, player.yaw),
            ]),
            CoreEvent::EntityDespawned { entity_ids } => {
                Ok(vec![encode_destroy_entities(entity_ids)?])
            }
            CoreEvent::InventoryContents {
                container,
                inventory,
            } => Ok(vec![encode_window_items(window_id(*container), inventory)?]),
            CoreEvent::InventorySlotChanged {
                container,
                slot,
                stack,
            } => Ok(vec![encode_set_slot(
                window_id(*container),
                *slot,
                stack.as_ref(),
            )?]),
            CoreEvent::SelectedHotbarSlotChanged { slot } => {
                Ok(vec![encode_held_item_change(*slot)])
            }
            CoreEvent::BlockChanged { position, block } => {
                Ok(vec![encode_block_change(*position, block)])
            }
            CoreEvent::KeepAliveRequested { keep_alive_id } => {
                Ok(vec![encode_keep_alive(*keep_alive_id)])
            }
            CoreEvent::LoginAccepted { .. } | CoreEvent::Disconnect { .. } => Err(
                ProtocolError::InvalidPacket("session event cannot be encoded as play sync"),
            ),
        }
    }
}

impl ProtocolAdapter for Je1710Adapter {
    fn protocol_version(&self) -> ProtocolVersion {
        ProtocolVersion(PROTOCOL_VERSION_1_7_10)
    }

    fn version_name(&self) -> &'static str {
        VERSION_NAME_1_7_10
    }

    fn storage_adapter(&self) -> &dyn StorageAdapter {
        &self.storage
    }
}

pub(crate) fn legacy_block(state: &BlockState) -> (u16, u8) {
    match state.key.as_str() {
        STONE => (1, 0),
        GRASS_BLOCK => (2, 0),
        DIRT => (3, 0),
        COBBLESTONE => (4, 0),
        OAK_PLANKS => (5, 0),
        BEDROCK => (7, 0),
        SAND => (12, 0),
        GLASS => (20, 0),
        SANDSTONE => (24, 0),
        BRICKS => (45, 0),
        _ => (0, 0),
    }
}

pub(crate) fn semantic_block(block_id: u16, metadata: u8) -> BlockState {
    match block_id {
        1 => BlockState::stone(),
        2 => BlockState::grass_block(),
        3 => BlockState::dirt(),
        4 => BlockState::cobblestone(),
        5 if metadata == 0 => BlockState::oak_planks(),
        7 => BlockState::bedrock(),
        12 if metadata == 0 => BlockState::sand(),
        20 => BlockState::glass(),
        24 if metadata == 0 => BlockState::sandstone(),
        45 => BlockState::bricks(),
        _ => BlockState::air(),
    }
}

fn legacy_item(stack: &ItemStack) -> Option<(i16, u16)> {
    let damage = stack.damage;
    match stack.key.as_str() {
        STONE => Some((1, damage)),
        GRASS_BLOCK => Some((2, damage)),
        DIRT => Some((3, damage)),
        COBBLESTONE => Some((4, damage)),
        OAK_PLANKS => Some((5, damage)),
        SAND => Some((12, damage)),
        GLASS => Some((20, damage)),
        SANDSTONE => Some((24, damage)),
        BRICKS => Some((45, damage)),
        _ => None,
    }
}

fn semantic_item(item_id: i16, damage: u16, count: u8) -> ItemStack {
    let key = match item_id {
        1 => STONE,
        2 => GRASS_BLOCK,
        3 => DIRT,
        4 => COBBLESTONE,
        5 if damage == 0 => OAK_PLANKS,
        12 if damage == 0 => SAND,
        20 => GLASS,
        24 if damage == 0 => SANDSTONE,
        45 => BRICKS,
        _ => return ItemStack::unsupported(count, damage),
    };
    ItemStack::new(key, count, damage)
}

const fn window_id(container: InventoryContainer) -> u8 {
    match container {
        InventoryContainer::Player => 0,
    }
}

fn encode_login_success(player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_LOGIN_SUCCESS);
    writer.write_string(&player.id.0.hyphenated().to_string())?;
    writer.write_string(&player.username)?;
    Ok(writer.into_inner())
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
    write_slot(&mut writer, stack)?;
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
    for slot in &inventory.slots {
        write_slot(&mut writer, slot.as_ref())?;
    }
    Ok(writer.into_inner())
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

fn build_chunk_data(chunk: &ChunkColumn, include_biomes: bool) -> (u16, Vec<u8>) {
    let mut bit_map = 0_u16;
    let mut sections = Vec::new();
    for (section_y, section) in &chunk.sections {
        if !(0..16).contains(section_y) || section.is_empty() {
            continue;
        }
        bit_map |= 1_u16 << u16::try_from(*section_y).expect("section index should fit into u16");
        let mut blocks = vec![0_u8; 4096];
        let mut metadata = vec![0_u8; 2048];
        let block_light = vec![0_u8; 2048];
        let sky_light = vec![0xff_u8; 2048];
        for (index, state) in section.iter_blocks() {
            let (block_id, block_meta) = legacy_block(state);
            let index_usize = usize::from(index);
            blocks[index_usize] =
                u8::try_from(block_id).expect("legacy block id should fit into byte");
            set_nibble(&mut metadata, index_usize, block_meta);
        }
        sections.extend_from_slice(&blocks);
        sections.extend_from_slice(&metadata);
        sections.extend_from_slice(&block_light);
        sections.extend_from_slice(&sky_light);
    }
    if include_biomes {
        sections.extend_from_slice(&chunk.biomes);
    }
    (bit_map, sections)
}

fn set_nibble(target: &mut [u8], index: usize, value: u8) {
    let byte_index = index / 2;
    if index.is_multiple_of(2) {
        target[byte_index] = (target[byte_index] & 0xf0) | (value & 0x0f);
    } else {
        target[byte_index] = (target[byte_index] & 0x0f) | ((value & 0x0f) << 4);
    }
}

pub(crate) fn get_nibble(source: &[u8], index: usize) -> u8 {
    let byte = source[index / 2];
    if index.is_multiple_of(2) {
        byte & 0x0f
    } else {
        (byte >> 4) & 0x0f
    }
}

fn zlib_compress(data: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|_| ProtocolError::InvalidPacket("failed to compress payload"))?;
    encoder
        .finish()
        .map_err(|_| ProtocolError::InvalidPacket("failed to finalize compressed payload"))
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

fn to_fixed_point(value: f64) -> i32 {
    rounded_f64_to_i32(value * 32.0)
}

fn to_angle_byte(value: f32) -> i8 {
    let wrapped = value.rem_euclid(360.0);
    let scaled = rounded_f32_to_i32(wrapped * 256.0 / 360.0);
    let narrowed =
        u8::try_from(scaled.rem_euclid(256)).expect("wrapped angle should fit into byte");
    i8::from_be_bytes([narrowed])
}

fn rounded_f64_to_i32(value: f64) -> i32 {
    value
        .round()
        .to_i32()
        .expect("fixed-point value should fit into i32")
}

fn rounded_f32_to_i32(value: f32) -> i32 {
    value
        .round()
        .to_i32()
        .expect("angle byte intermediate should fit into i32")
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
    let held_item = read_slot(reader)?;
    let _cursor_x = reader.read_u8()?;
    let _cursor_y = reader.read_u8()?;
    let _cursor_z = reader.read_u8()?;
    if position.x == -1 && position.z == -1 && position.y == 255 && direction == 255 {
        return Ok(None);
    }
    Ok(Some(CoreCommand::PlaceBlock {
        player_id,
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

fn read_slot(reader: &mut PacketReader<'_>) -> Result<Option<ItemStack>, ProtocolError> {
    let item_id = reader.read_i16()?;
    if item_id < 0 {
        return Ok(None);
    }
    let count = reader.read_u8()?;
    let damage = u16::from_be_bytes(reader.read_i16()?.to_be_bytes());
    skip_slot_nbt(reader)?;
    Ok(Some(semantic_item(item_id, damage, count)))
}

fn write_slot(writer: &mut PacketWriter, stack: Option<&ItemStack>) -> Result<(), ProtocolError> {
    let Some(stack) = stack else {
        writer.write_i16(-1);
        return Ok(());
    };
    let Some((item_id, damage)) = legacy_item(stack) else {
        return Err(ProtocolError::InvalidPacket("unsupported inventory item"));
    };
    writer.write_i16(item_id);
    writer.write_u8(stack.count);
    writer.write_i16(i16::from_be_bytes(damage.to_be_bytes()));
    writer.write_i16(-1);
    Ok(())
}

fn skip_slot_nbt(reader: &mut PacketReader<'_>) -> Result<(), ProtocolError> {
    let length = reader.read_i16()?;
    if length < 0 {
        return Ok(());
    }
    let length = usize::try_from(length)
        .map_err(|_| ProtocolError::InvalidPacket("negative slot nbt length"))?;
    let _ = reader.read_bytes(length)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Je1710Adapter, PROTOCOL_VERSION_1_7_10, get_nibble, legacy_block};
    use mc_core::{
        BlockState, ChunkColumn, ChunkPos, ConnectionId, CoreCommand, CoreConfig, CoreEvent,
        InventoryContainer, PlayerId, PlayerInventory, PlayerSnapshot, ProtocolVersion, ServerCore,
        Vec3,
    };
    use mc_proto_common::{
        LoginRequest, PacketWriter, PlayEncodingContext, PlaySyncAdapter, ServerListStatus,
        SessionAdapter, StatusRequest,
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
            .decode_handshake(&handshake)
            .expect("handshake should decode");
        assert_eq!(
            intent.protocol_version,
            ProtocolVersion(PROTOCOL_VERSION_1_7_10)
        );

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
    }

    #[test]
    fn encodes_status_and_login_events() {
        let adapter = Je1710Adapter::new();
        let status_packet = adapter
            .encode_status_response(&ServerListStatus {
                version_name: "1.7.10".to_string(),
                protocol: ProtocolVersion(5),
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
            CoreCommand::CreativeInventorySet { slot: 36, stack: Some(ref stack), .. }
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
                protocol_version: ProtocolVersion(5),
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
