#![allow(clippy::multiple_crate_versions)]
mod decoding;
mod encoding;

#[cfg(test)]
mod tests;

use decoding::{decode_play_packet, read_login_byte_array};
use encoding::{
    encode_block_change, encode_chunk, encode_destroy_entities, encode_entity_head_rotation,
    encode_entity_teleport, encode_held_item_change, encode_join_game, encode_keep_alive,
    encode_named_entity_spawn, encode_player_abilities, encode_position_and_look, encode_set_slot,
    encode_spawn_position, encode_time_update, encode_update_health, encode_window_items,
    window_id, write_login_byte_array,
};
use mc_core::{CoreCommand, CoreEvent, PlayerId, PlayerSnapshot};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeIntent, HandshakeProbe, LoginRequest, MinecraftWireCodec,
    PacketReader, PacketWriter, PlayEncodingContext, PlaySyncAdapter, ProtocolAdapter,
    ProtocolDescriptor, ProtocolError, ServerListStatus, SessionAdapter, StatusRequest,
    TransportKind, WireCodec, WireFormatKind,
};
use mc_proto_je_common::{decode_handshake_frame, modern_window_slot};
use serde_json::json;

const PROTOCOL_VERSION_1_12_2: i32 = 340;
const VERSION_NAME_1_12_2: &str = "1.12.2";
pub const JE_1_12_2_ADAPTER_ID: &str = "je-1_12_2";

const PACKET_STATUS_REQUEST: i32 = 0x00;
const PACKET_STATUS_PING: i32 = 0x01;
const PACKET_LOGIN_START: i32 = 0x00;
const PACKET_LOGIN_ENCRYPTION_RESPONSE: i32 = 0x01;

const PACKET_CB_STATUS_RESPONSE: i32 = 0x00;
const PACKET_CB_STATUS_PONG: i32 = 0x01;
const PACKET_CB_LOGIN_DISCONNECT: i32 = 0x00;
const PACKET_CB_LOGIN_ENCRYPTION_REQUEST: i32 = 0x01;
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
            PACKET_LOGIN_ENCRYPTION_RESPONSE => Ok(LoginRequest::EncryptionResponse {
                shared_secret_encrypted: read_login_byte_array(&mut reader)?,
                verify_token_encrypted: read_login_byte_array(&mut reader)?,
            }),
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

    fn encode_encryption_request(
        &self,
        server_id: &str,
        public_key_der: &[u8],
        verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = PacketWriter::default();
        writer.write_varint(PACKET_CB_LOGIN_ENCRYPTION_REQUEST);
        writer.write_string(server_id)?;
        write_login_byte_array(&mut writer, public_key_der)?;
        write_login_byte_array(&mut writer, verify_token)?;
        Ok(writer.into_inner())
    }

    fn encode_network_settings(
        &self,
        _compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(ProtocolError::InvalidPacket(
            "java edition adapters do not support bedrock network settings",
        ))
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
        decode_play_packet(player_id, frame)
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
                encode_named_entity_spawn(*entity_id, player),
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
            wire_format: WireFormatKind::MinecraftFramed,
            edition: Edition::Je,
            version_name: VERSION_NAME_1_12_2.to_string(),
            protocol_number: PROTOCOL_VERSION_1_12_2,
        }
    }
}
