use mc_core::{
    BlockFace, BlockPos, ChunkColumn, CoreCommand, CoreEvent, DimensionId, EntityId,
    InteractionHand, InventoryContainer, PlayerId, PlayerInventory, PlayerSnapshot, Vec3,
    WorldMeta,
};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeIntent, HandshakeNextState, HandshakeProbe, LoginRequest,
    MinecraftWireCodec, PacketReader, PacketWriter, PlayEncodingContext, PlaySyncAdapter,
    ProtocolAdapter, ProtocolDescriptor, ProtocolError, ServerListStatus, SessionAdapter,
    StatusRequest, TransportKind, WireCodec,
};
use mc_proto_je_common::{
    build_chunk_data_1_8, legacy_block_state_id, legacy_inventory_slot, legacy_window_items,
    legacy_window_slot, pack_block_position, read_legacy_slot, to_angle_byte, to_fixed_point,
    unpack_block_position, write_empty_metadata_1_8, write_legacy_slot,
};
use serde_json::json;

const PROTOCOL_VERSION_1_8_X: i32 = 47;
const VERSION_NAME_1_8_X: &str = "1.8.x";
pub const JE_1_8_X_ADAPTER_ID: &str = "je-1_8_x";

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
pub struct Je18xAdapter {
    codec: MinecraftWireCodec,
}

impl Je18xAdapter {
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

impl HandshakeProbe for Je18xAdapter {
    fn transport_kind(&self) -> TransportKind {
        TransportKind::Tcp
    }

    fn adapter_id(&self) -> Option<&'static str> {
        Some(JE_1_8_X_ADAPTER_ID)
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        decode_handshake_frame(frame)
    }
}

impl SessionAdapter for Je18xAdapter {
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

impl PlaySyncAdapter for Je18xAdapter {
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
                keep_alive_id: reader.read_varint()?,
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
                let Some(protocol_slot) = legacy_window_slot(*slot) else {
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

impl ProtocolAdapter for Je18xAdapter {
    fn descriptor(&self) -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: JE_1_8_X_ADAPTER_ID.to_string(),
            transport: TransportKind::Tcp,
            edition: Edition::Je,
            version_name: VERSION_NAME_1_8_X.to_string(),
            protocol_number: PROTOCOL_VERSION_1_8_X,
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
    writer.write_i8(dimension_to_i8(player.dimension));
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
    writer.write_varint(keep_alive_id);
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
    writer.write_i32(to_fixed_point(player.position.x));
    writer.write_i32(to_fixed_point(player.position.y));
    writer.write_i32(to_fixed_point(player.position.z));
    writer.write_i8(to_angle_byte(player.yaw));
    writer.write_i8(to_angle_byte(player.pitch));
    writer.write_i16(0);
    write_empty_metadata_1_8(&mut writer);
    Ok(writer.into_inner())
}

fn encode_entity_teleport(entity_id: EntityId, player: &PlayerSnapshot) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_ENTITY_TELEPORT);
    writer.write_varint(entity_id.0);
    writer.write_i32(to_fixed_point(player.position.x));
    writer.write_i32(to_fixed_point(player.position.y));
    writer.write_i32(to_fixed_point(player.position.z));
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
    stack: Option<&mc_core::ItemStack>,
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
    let items = legacy_window_items(inventory);
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
    let (bit_map, chunk_data) = build_chunk_data_1_8(chunk, true);
    let mut writer = PacketWriter::default();
    writer.write_varint(PACKET_CB_MAP_CHUNK);
    writer.write_i32(chunk.pos.x);
    writer.write_i32(chunk.pos.z);
    writer.write_bool(true);
    writer.write_u16(bit_map);
    writer.write_varint(
        i32::try_from(chunk_data.len())
            .map_err(|_| ProtocolError::InvalidPacket("chunk payload too large"))?,
    );
    writer.write_bytes(&chunk_data);
    Ok(writer.into_inner())
}

const fn dimension_to_i8(dimension: DimensionId) -> i8 {
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
    let direction = reader.read_i8()?;
    let held_item = read_legacy_slot(reader)?;
    let _cursor_x = reader.read_i8()?;
    let _cursor_y = reader.read_i8()?;
    let _cursor_z = reader.read_i8()?;
    if position.x == -1 && position.z == -1 && position.y == 255 && direction == -1 {
        return Ok(None);
    }
    Ok(Some(CoreCommand::PlaceBlock {
        player_id,
        hand: InteractionHand::Main,
        position,
        face: u8::try_from(direction)
            .ok()
            .and_then(BlockFace::from_protocol_byte),
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
    let _skin_parts = reader.read_u8()?;
    Ok(CoreCommand::UpdateClientView {
        player_id,
        view_distance: view_distance.max(1),
    })
}

const fn i8_to_u8(value: i8) -> u8 {
    if value.is_negative() { 0 } else { value as u8 }
}

#[cfg(test)]
mod tests {
    use super::{JE_1_8_X_ADAPTER_ID, Je18xAdapter, PROTOCOL_VERSION_1_8_X, VERSION_NAME_1_8_X};
    use mc_core::{
        ChunkColumn, ChunkPos, CoreCommand, CoreEvent, DimensionId, InventoryContainer,
        InventorySlot, ItemStack, PlayerId, PlayerInventory, PlayerSnapshot, Vec3,
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
        let adapter = Je18xAdapter::new();

        let handshake = [
            0x00, 0x2f, 0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', 0x63, 0xdd,
            0x02,
        ];
        let intent = adapter
            .try_route(&handshake)
            .expect("handshake should decode")
            .expect("handshake should match JE");
        assert_eq!(intent.protocol_number, PROTOCOL_VERSION_1_8_X);
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
    fn encodes_status_and_inventory_events() {
        let adapter = Je18xAdapter::new();
        let status_packet = adapter
            .encode_status_response(&ServerListStatus {
                version: ProtocolDescriptor {
                    adapter_id: JE_1_8_X_ADAPTER_ID.to_string(),
                    transport: TransportKind::Tcp,
                    edition: Edition::Je,
                    version_name: VERSION_NAME_1_8_X.to_string(),
                    protocol_number: PROTOCOL_VERSION_1_8_X,
                },
                players_online: 1,
                max_players: 20,
                description: "hello".to_string(),
            })
            .expect("status should encode");
        assert_eq!(status_packet[0], 0x00);

        let packet = adapter
            .encode_play_event(
                &CoreEvent::InventorySlotChanged {
                    container: InventoryContainer::Player,
                    slot: InventorySlot::Offhand,
                    stack: Some(ItemStack::new("minecraft:glass", 1, 0)),
                },
                &PlayEncodingContext {
                    player_id: player_snapshot("alpha").id,
                    entity_id: mc_core::EntityId(1),
                },
            )
            .expect("inventory update should encode");
        assert!(packet.is_empty());
    }

    #[test]
    fn decodes_play_packets_into_core_commands() {
        let adapter = Je18xAdapter::new();
        let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"decode-play-18"));

        let mut writer = PacketWriter::default();
        writer.write_varint(0x06);
        writer.write_f64(42.0);
        writer.write_f64(43.0);
        writer.write_f64(10.0);
        writer.write_f32(90.0);
        writer.write_f32(0.0);
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
    fn encodes_chunk_and_spawn_packets() {
        let adapter = Je18xAdapter::new();
        let player = player_snapshot("alpha");
        let packet = adapter
            .encode_play_event(
                &CoreEvent::EntitySpawned {
                    entity_id: mc_core::EntityId(7),
                    player: player.clone(),
                },
                &PlayEncodingContext {
                    player_id: player.id,
                    entity_id: mc_core::EntityId(7),
                },
            )
            .expect("spawn should encode");
        assert_eq!(packet.len(), 2);

        let chunk = ChunkColumn::new(ChunkPos::new(0, 0));
        let packets = adapter
            .encode_play_event(
                &CoreEvent::ChunkBatch {
                    chunks: vec![chunk],
                },
                &PlayEncodingContext {
                    player_id: player.id,
                    entity_id: mc_core::EntityId(7),
                },
            )
            .expect("chunk batch should encode");
        let mut reader = PacketReader::new(&packets[0]);
        assert_eq!(reader.read_varint().expect("packet id should decode"), 0x21);
    }

    #[test]
    fn decodes_creative_inventory_slot_mapping() {
        let adapter = Je18xAdapter::new();
        let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"creative-18"));
        let mut writer = PacketWriter::default();
        writer.write_varint(0x10);
        writer.write_i16(36);
        writer.write_i16(20);
        writer.write_u8(64);
        writer.write_i16(0);
        writer.write_i16(-1);

        let command = adapter
            .decode_play(player_id, &writer.into_inner())
            .expect("creative inventory action should decode")
            .expect("creative inventory action should produce a command");
        assert!(matches!(
            command,
            CoreCommand::CreativeInventorySet {
                slot: InventorySlot::Hotbar(0),
                ..
            }
        ));
    }
}
