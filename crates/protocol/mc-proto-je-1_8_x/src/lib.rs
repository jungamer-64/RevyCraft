#![allow(clippy::multiple_crate_versions)]
use mc_core::{
    BlockFace, BlockPos, ChunkColumn, CoreCommand, DimensionId, EntityId, InteractionHand,
    InventoryContainer, InventorySlot, ItemStack, PlayerId, PlayerInventory, PlayerSnapshot,
    Vec3, WorldMeta,
};
use mc_proto_common::{Edition, PacketReader, PacketWriter, ProtocolDescriptor, ProtocolError, TransportKind, WireFormatKind};
use mc_proto_je_common::{
    JavaEditionAdapter, JavaEditionProfile, build_chunk_data_1_8, legacy_block_state_id,
    legacy_inventory_slot, legacy_window_items, legacy_window_slot, pack_block_position,
    player_window_id, read_legacy_slot, to_angle_byte, to_fixed_point, unpack_block_position,
    write_empty_metadata_1_8, write_legacy_slot,
};
use serde_json::json;

const PROTOCOL_VERSION_1_8_X: i32 = 47;
const VERSION_NAME_1_8_X: &str = "1.8.x";
pub const JE_1_8_X_ADAPTER_ID: &str = "je-1_8_x";

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
pub struct Je18xProfile;

pub type Je18xAdapter = JavaEditionAdapter<Je18xProfile>;

impl JavaEditionProfile for Je18xProfile {
    fn adapter_id(&self) -> &'static str {
        JE_1_8_X_ADAPTER_ID
    }

    fn descriptor(&self) -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: JE_1_8_X_ADAPTER_ID.to_string(),
            transport: TransportKind::Tcp,
            wire_format: WireFormatKind::MinecraftFramed,
            edition: Edition::Je,
            version_name: VERSION_NAME_1_8_X.to_string(),
            protocol_number: PROTOCOL_VERSION_1_8_X,
        }
    }

    fn play_disconnect_packet_id(&self) -> i32 {
        PACKET_CB_PLAY_DISCONNECT
    }

    fn format_disconnect_reason(&self, reason: &str) -> String {
        json!({ "text": reason }).to_string()
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
        chunks
            .iter()
            .map(encode_chunk)
            .map(|packet| packet.map(|packet| vec![packet]))
            .collect::<Result<Vec<_>, _>>()
            .map(|packets| packets.into_iter().flatten().collect())
    }

    fn encode_entity_spawn(
        &self,
        entity_id: EntityId,
        player: &PlayerSnapshot,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        Ok(vec![
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
            protocol_slot,
            stack,
        )?))
    }

    fn encode_selected_hotbar_slot_changed(&self, slot: u8) -> Result<Vec<u8>, ProtocolError> {
        Ok(encode_held_item_change(slot))
    }

    fn encode_block_changed(
        &self,
        position: BlockPos,
        block: &mc_core::BlockState,
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

fn encode_named_entity_spawn(entity_id: EntityId, player: &PlayerSnapshot) -> Vec<u8> {
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
    writer.into_inner()
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
    if value.is_negative() {
        0
    } else {
        value.cast_unsigned()
    }
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
    fn encodes_status_and_inventory_events() {
        let adapter = Je18xAdapter::new();
        let status_packet = adapter
            .encode_status_response(&ServerListStatus {
                version: ProtocolDescriptor {
                    adapter_id: JE_1_8_X_ADAPTER_ID.to_string(),
                    transport: TransportKind::Tcp,
                    wire_format: WireFormatKind::MinecraftFramed,
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

        let encryption_request = adapter
            .encode_encryption_request("", &[1, 2, 3], &[4, 5])
            .expect("encryption request should encode");
        assert_eq!(encryption_request[0], 0x01);

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
