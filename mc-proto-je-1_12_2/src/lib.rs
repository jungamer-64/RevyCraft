use mc_core::{
    BlockFace, BlockPos, ChunkColumn, CoreCommand, CoreEvent, DimensionId, EntityId,
    InteractionHand, InventoryContainer, ItemStack, PlayerId, PlayerInventory, PlayerSnapshot,
    Vec3, WorldMeta,
};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeIntent, HandshakeNextState, HandshakeProbe, LoginRequest,
    MinecraftWireCodec, PacketReader, PacketWriter, PlayEncodingContext, PlaySyncAdapter,
    ProtocolAdapter, ProtocolDescriptor, ProtocolError, ServerListStatus, SessionAdapter,
    StatusRequest, TransportKind, WireCodec,
};
use mc_proto_je_common::{
    build_chunk_data_1_12, legacy_block_state_id, modern_inventory_slot, modern_window_items,
    modern_window_slot, pack_block_position, read_legacy_slot, to_angle_byte,
    unpack_block_position, write_empty_metadata_1_12, write_legacy_slot,
};
use serde_json::json;

const PROTOCOL_VERSION_1_12_2: i32 = 340;
const VERSION_NAME_1_12_2: &str = "1.12.2";
pub const JE_1_12_2_ADAPTER_ID: &str = "je-1_12_2";

const PACKET_HANDSHAKE: i32 = 0x00;
const PACKET_STATUS_REQUEST: i32 = 0x00;
const PACKET_STATUS_PING: i32 = 0x01;
const PACKET_LOGIN_START: i32 = 0x00;
const PACKET_LOGIN_ENCRYPTION_RESPONSE: i32 = 0x01;

const PACKET_CB_STATUS_RESPONSE: i32 = 0x00;
const PACKET_CB_STATUS_PONG: i32 = 0x01;
const PACKET_CB_LOGIN_DISCONNECT: i32 = 0x00;
const PACKET_CB_LOGIN_SUCCESS: i32 = 0x02;

const PACKET_CB_NAMED_ENTITY_SPAWN: i32 = 0x05;
const PACKET_CB_BLOCK_CHANGE: i32 = 0x0b;
const PACKET_CB_WINDOW_ITEMS: i32 = 0x14;
const PACKET_CB_SET_SLOT: i32 = 0x16;
const PACKET_CB_PLAY_DISCONNECT: i32 = 0x1a;
const PACKET_CB_KEEP_ALIVE: i32 = 0x1f;
const PACKET_CB_MAP_CHUNK: i32 = 0x20;
const PACKET_CB_JOIN_GAME: i32 = 0x23;
const PACKET_CB_PLAYER_ABILITIES: i32 = 0x2c;
const PACKET_CB_PLAYER_POSITION_AND_LOOK: i32 = 0x2f;
const PACKET_CB_DESTROY_ENTITIES: i32 = 0x32;
const PACKET_CB_ENTITY_HEAD_ROTATION: i32 = 0x36;
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

#[derive(Default)]
pub struct Je1122Adapter {
    codec: MinecraftWireCodec,
}

impl Je1122Adapter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn decode_handshake_frame(frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
    let mut reader = PacketReader::new(frame);
    let packet_id = reader.read_varint()?;
    if packet_id != PACKET_HANDSHAKE {
        return Ok(None);
    }
    let protocol_number = reader.read_varint()?;
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
    Ok(Some(HandshakeIntent {
        edition: Edition::Je,
        protocol_number,
        server_host,
        server_port,
        next_state,
    }))
}

impl HandshakeProbe for Je1122Adapter {
    fn transport_kind(&self) -> TransportKind {
        TransportKind::Tcp
    }

    fn adapter_id(&self) -> Option<&'static str> {
        Some(JE_1_12_2_ADAPTER_ID)
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        decode_handshake_frame(frame)
    }
}

impl SessionAdapter for Je1122Adapter {
    fn wire_codec(&self) -> &dyn WireCodec {
        &self.codec
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
                "name": status.version.version_name,
                "protocol": status.version.protocol_number,
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
        writer.write_string(&json!({ "text": reason }).to_string())?;
        Ok(writer.into_inner())
    }

    fn encode_login_success(&self, player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = PacketWriter::default();
        writer.write_varint(PACKET_CB_LOGIN_SUCCESS);
        writer.write_string(&player.id.0.hyphenated().to_string())?;
        writer.write_string(&player.username)?;
        Ok(writer.into_inner())
    }
}

impl PlaySyncAdapter for Je1122Adapter {
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
                keep_alive_id: i32::try_from(reader.read_i64()?)
                    .map_err(|_| ProtocolError::InvalidPacket("keepalive id out of range"))?,
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
            PACKET_SB_HELD_ITEM_CHANGE => Ok(Some(CoreCommand::SetHeldSlot {
                player_id,
                slot: reader.read_i16()?,
            })),
            PACKET_SB_CREATIVE_INVENTORY_ACTION => {
                let slot = reader.read_i16()?;
                let stack = read_legacy_slot(&mut reader)?;
                Ok(
                    modern_inventory_slot(slot).map(|slot| CoreCommand::CreativeInventorySet {
                        player_id,
                        slot,
                        stack,
                    }),
                )
            }
            PACKET_SB_PLAYER_BLOCK_PLACEMENT => decode_place_block_packet(player_id, &mut reader),
            PACKET_SB_USE_ITEM => {
                let _hand = decode_interaction_hand(reader.read_varint()?)?;
                Ok(None)
            }
            PACKET_SB_SETTINGS => Ok(Some(decode_client_settings_packet(player_id, &mut reader)?)),
            PACKET_SB_CLIENT_COMMAND => Ok(Some(CoreCommand::ClientStatus {
                player_id,
                action_id: i8::try_from(reader.read_varint()?)
                    .map_err(|_| ProtocolError::InvalidPacket("client command out of range"))?,
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
                encode_join_game(*entity_id, world_meta, player)?,
                encode_spawn_position(world_meta.spawn),
                encode_time_update(world_meta.age, world_meta.time),
                encode_update_health(player),
                encode_player_abilities(world_meta.game_mode == 1),
                encode_position_and_look(player),
            ]),
            CoreEvent::ChunkBatch { chunks } => chunks
                .iter()
                .map(encode_chunk)
                .map(|packet| packet.map(|packet| vec![packet]))
                .collect::<Result<Vec<_>, _>>()
                .map(|packets| packets.into_iter().flatten().collect()),
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
            } => {
                let Some(protocol_slot) = modern_window_slot(*slot) else {
                    return Ok(Vec::new());
                };
                Ok(vec![encode_set_slot(
                    window_id(*container),
                    protocol_slot,
                    stack.as_ref(),
                )?])
            }
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

impl ProtocolAdapter for Je1122Adapter {
    fn descriptor(&self) -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: JE_1_12_2_ADAPTER_ID.to_string(),
            transport: TransportKind::Tcp,
            edition: Edition::Je,
            version_name: VERSION_NAME_1_12_2.to_string(),
            protocol_number: PROTOCOL_VERSION_1_12_2,
        }
    }
}

const fn window_id(container: InventoryContainer) -> u8 {
    match container {
        InventoryContainer::Player => 0,
    }
}

fn encode_join_game(
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

fn encode_spawn_position(spawn: BlockPos) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_SPAWN_POSITION);
    writer.write_i64(pack_block_position(spawn));
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
    writer.write_varint(i32::from(player.food));
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
    writer.write_i8(0);
    writer.write_varint(0);
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
    writer.write_i64(i64::from(keep_alive_id));
    writer.into_inner()
}

fn encode_named_entity_spawn(
    entity_id: EntityId,
    player: &PlayerSnapshot,
) -> Result<Vec<u8>, ProtocolError> {
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
    Ok(writer.into_inner())
}

fn encode_entity_teleport(entity_id: EntityId, player: &PlayerSnapshot) -> Vec<u8> {
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

fn encode_entity_head_rotation(entity_id: EntityId, yaw: f32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_ENTITY_HEAD_ROTATION);
    writer.write_varint(entity_id.0);
    writer.write_i8(to_angle_byte(yaw));
    writer.into_inner()
}

fn encode_destroy_entities(entity_ids: &[EntityId]) -> Result<Vec<u8>, ProtocolError> {
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

fn encode_block_change(position: BlockPos, block: &mc_core::BlockState) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_BLOCK_CHANGE);
    writer.write_i64(pack_block_position(position));
    writer.write_varint(legacy_block_state_id(block));
    writer.into_inner()
}

fn encode_set_slot(
    window_id: u8,
    slot: i16,
    stack: Option<&ItemStack>,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_SET_SLOT);
    writer.write_i8(i8::from_be_bytes([window_id]));
    writer.write_i16(slot);
    write_legacy_slot(&mut writer, stack)?;
    Ok(writer.into_inner())
}

fn encode_window_items(
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

fn encode_chunk(chunk: &ChunkColumn) -> Result<Vec<u8>, ProtocolError> {
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

fn decode_position_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<CoreCommand, ProtocolError> {
    let x = reader.read_f64()?;
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
        status: u8::try_from(reader.read_varint()?)
            .map_err(|_| ProtocolError::InvalidPacket("dig status out of range"))?,
        position: unpack_block_position(reader.read_i64()?),
        face: BlockFace::from_protocol_byte(reader.read_i8()?.to_be_bytes()[0]),
    })
}

fn decode_place_block_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<Option<CoreCommand>, ProtocolError> {
    let position = unpack_block_position(reader.read_i64()?);
    let face = u8::try_from(reader.read_varint()?)
        .map_err(|_| ProtocolError::InvalidPacket("face out of range"))?;
    let hand = decode_interaction_hand(reader.read_varint()?)?;
    let _cursor_x = reader.read_f32()?;
    let _cursor_y = reader.read_f32()?;
    let _cursor_z = reader.read_f32()?;
    Ok(Some(CoreCommand::PlaceBlock {
        player_id,
        hand,
        position,
        face: BlockFace::from_protocol_byte(face),
        held_item: None,
    }))
}

fn decode_client_settings_packet(
    player_id: PlayerId,
    reader: &mut PacketReader<'_>,
) -> Result<CoreCommand, ProtocolError> {
    let _locale = reader.read_string(16)?;
    let view_distance = i8_to_u8(reader.read_i8()?);
    let _chat_flags = reader.read_varint()?;
    let _chat_colors = reader.read_bool()?;
    let _skin_parts = reader.read_u8()?;
    let _main_hand = reader.read_varint()?;
    Ok(CoreCommand::UpdateClientView {
        player_id,
        view_distance: view_distance.max(1),
    })
}

fn decode_interaction_hand(hand: i32) -> Result<InteractionHand, ProtocolError> {
    match hand {
        0 => Ok(InteractionHand::Main),
        1 => Ok(InteractionHand::Offhand),
        _ => Err(ProtocolError::InvalidPacket("invalid interaction hand")),
    }
}

const fn i8_to_u8(value: i8) -> u8 {
    if value.is_negative() { 0 } else { value as u8 }
}

#[cfg(test)]
mod tests {
    use super::{
        JE_1_12_2_ADAPTER_ID, Je1122Adapter, PROTOCOL_VERSION_1_12_2, VERSION_NAME_1_12_2,
    };
    use mc_core::{
        CoreCommand, CoreEvent, DimensionId, InteractionHand, InventoryContainer, InventorySlot,
        ItemStack, PlayerId, PlayerInventory, PlayerSnapshot, Vec3,
    };
    use mc_proto_common::{
        Edition, HandshakeProbe, LoginRequest, PacketReader, PacketWriter, PlayEncodingContext,
        PlaySyncAdapter, ProtocolDescriptor, ServerListStatus, SessionAdapter, StatusRequest,
        TransportKind,
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
            dimension: DimensionId::Overworld,
            health: 20.0,
            food: 20,
            food_saturation: 5.0,
            inventory: PlayerInventory::creative_starter(),
            selected_hotbar_slot: 0,
        }
    }

    #[test]
    fn decodes_handshake_status_and_login_packets() {
        let adapter = Je1122Adapter::new();

        let handshake = [
            0x00, 0xd4, 0x02, 0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', 0x63,
            0xdd, 0x02,
        ];
        let intent = adapter
            .try_route(&handshake)
            .expect("handshake should decode")
            .expect("handshake should match JE");
        assert_eq!(intent.protocol_number, PROTOCOL_VERSION_1_12_2);
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
    }

    #[test]
    fn encodes_status_and_offhand_inventory_events() {
        let adapter = Je1122Adapter::new();
        let status_packet = adapter
            .encode_status_response(&ServerListStatus {
                version: ProtocolDescriptor {
                    adapter_id: JE_1_12_2_ADAPTER_ID.to_string(),
                    transport: TransportKind::Tcp,
                    edition: Edition::Je,
                    version_name: VERSION_NAME_1_12_2.to_string(),
                    protocol_number: PROTOCOL_VERSION_1_12_2,
                },
                players_online: 1,
                max_players: 20,
                description: "hello".to_string(),
            })
            .expect("status should encode");
        assert_eq!(status_packet[0], 0x00);

        let player = player_snapshot("alpha");
        let packets = adapter
            .encode_play_event(
                &CoreEvent::InventorySlotChanged {
                    container: InventoryContainer::Player,
                    slot: InventorySlot::Offhand,
                    stack: Some(ItemStack::new("minecraft:glass", 1, 0)),
                },
                &PlayEncodingContext {
                    player_id: player.id,
                    entity_id: mc_core::EntityId(1),
                },
            )
            .expect("offhand update should encode");
        let mut reader = PacketReader::new(&packets[0]);
        assert_eq!(reader.read_varint().expect("packet id should decode"), 0x16);
        assert_eq!(reader.read_i8().expect("window id should decode"), 0);
        assert_eq!(reader.read_i16().expect("slot should decode"), 45);
    }

    #[test]
    fn decodes_offhand_block_place() {
        let adapter = Je1122Adapter::new();
        let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"offhand-1122"));
        let mut writer = PacketWriter::default();
        writer.write_varint(0x1f);
        writer.write_i64(mc_proto_je_common::pack_block_position(
            mc_core::BlockPos::new(2, 3, 4),
        ));
        writer.write_varint(1);
        writer.write_varint(1);
        writer.write_f32(0.5);
        writer.write_f32(0.5);
        writer.write_f32(0.5);
        let command = adapter
            .decode_play(player_id, &writer.into_inner())
            .expect("block place should decode")
            .expect("block place should produce a command");
        assert!(matches!(
            command,
            CoreCommand::PlaceBlock {
                hand: InteractionHand::Offhand,
                ..
            }
        ));
    }
}
