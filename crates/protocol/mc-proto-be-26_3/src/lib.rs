#![allow(clippy::multiple_crate_versions)]
use bedrockrs_proto::ProtoVersion;
use bedrockrs_proto::V924;
use bedrockrs_proto::codec::{decode_packets, encode_packets};
use bedrockrs_proto::v662::enums::{
    ChatRestrictionLevel, ConnectionFailReason, Difficulty, EditorWorldType, EducationEditionOffer,
    GamePublishSetting, GameType, PacketCompressionAlgorithm, PlayStatus, PlayerActionType,
    PlayerPermissionLevel, SpawnBiomeType,
};
use bedrockrs_proto::v662::packets::{
    ClientCacheStatusPacket, LoginPacket, MobEquipmentPacket, MovePlayerPacket,
    NetworkSettingsPacket, PlayStatusPacket, PlayerActionPacket, RequestNetworkSettingsPacket,
    ResourcePackClientResponsePacket, UpdateBlockPacket,
};
use bedrockrs_proto::v662::types::{
    ActorRuntimeID, ActorUniqueID, BaseGameVersion, EduSharedUriResource, Experiments,
    NetworkPermissions, SpawnSettings,
};
use bedrockrs_proto::v712::packets::{DisconnectMessage, DisconnectPacket};
use bedrockrs_proto::v818::packets::ResourcePacksInfoPacket;
use bedrockrs_proto::v818::types::SyncedPlayerMovementSettings;
use bedrockrs_proto::v898::packets::ResourcePackStackPacket;
use bedrockrs_proto::v924::packets::StartGamePacket;
use bedrockrs_proto::v924::types::LevelSettings;
use mc_core::catalog::{BEDROCK, BRICKS, COBBLESTONE, DIRT, GLASS, GRASS_BLOCK, OAK_PLANKS, SAND};
use mc_core::{BlockFace, BlockState, CoreCommand, CoreEvent, PlayerId, PlayerSnapshot};
use mc_proto_be_common::{
    bedrock_probe_intent, block_pos_from_network, block_pos_to_network, detects_bedrock_datagram,
    parse_bedrock_login_payload, protocol_error, vec3_to_bedrock,
};
use mc_proto_common::{
    BedrockListenerDescriptor, ConnectionPhase, Edition, HandshakeIntent, HandshakeProbe,
    LoginRequest, PlayEncodingContext, PlaySyncAdapter, ProtocolAdapter, ProtocolDescriptor,
    ProtocolError, RawPacketStreamWireCodec, ServerListStatus, SessionAdapter, StatusRequest,
    TransportKind, WireCodec, WireFormatKind,
};
use std::collections::HashMap;
use vek::Vec2;

pub const BE_26_3_ADAPTER_ID: &str = "be-26_3";
pub const BE_26_3_VERSION_NAME: &str = "bedrock-26.3";
pub const BE_26_3_PROTOCOL_NUMBER: i32 = 924;
const BEDROCK_26_3_RUNTIME_ID_STONE: u32 = 2_532;
const BEDROCK_26_3_RUNTIME_ID_COBBLESTONE: u32 = 5_088;
const BEDROCK_26_3_RUNTIME_ID_SAND: u32 = 6_234;
const BEDROCK_26_3_RUNTIME_ID_BRICKS: u32 = 7_455;
const BEDROCK_26_3_RUNTIME_ID_DIRT: u32 = 9_852;
const BEDROCK_26_3_RUNTIME_ID_GRASS_BLOCK: u32 = 11_062;
const BEDROCK_26_3_RUNTIME_ID_GLASS: u32 = 11_998;
const BEDROCK_26_3_RUNTIME_ID_AIR: u32 = 12_530;
const BEDROCK_26_3_RUNTIME_ID_BEDROCK: u32 = 13_079;
const BEDROCK_26_3_RUNTIME_ID_OAK_PLANKS: u32 = 14_388;

#[derive(Default)]
pub struct Bedrock263Adapter {
    codec: RawPacketStreamWireCodec,
}

impl Bedrock263Adapter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn encode_v924(packets: &[V924]) -> Result<Vec<u8>, ProtocolError> {
        encode_packets(packets, None, None)
            .map_err(|error| ProtocolError::Plugin(format!("bedrock encode failed: {error}")))
    }

    fn decode_v924(frame: &[u8]) -> Result<Vec<V924>, ProtocolError> {
        decode_packets::<V924>(frame.to_vec(), None, None).map_err(|error| {
            ProtocolError::InvalidPacket(Box::leak(
                format!("bedrock decode failed: {error}").into_boxed_str(),
            ))
        })
    }

    fn play_status(status: PlayStatus) -> Result<Vec<u8>, ProtocolError> {
        Self::encode_v924(&[V924::PlayStatusPacket(PlayStatusPacket { status })])
    }

    fn start_game_packets(
        player: &PlayerSnapshot,
        entity_id: mc_core::EntityId,
        world_meta: &mc_core::WorldMeta,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let start_game = StartGamePacket {
            target_actor_id: ActorUniqueID(bedrock_actor_id(entity_id)),
            target_runtime_id: ActorRuntimeID(bedrock_actor_id(entity_id)),
            actor_game_type: GameType::Creative,
            position: vec3_to_bedrock(player.position),
            rotation: Vec2::new(player.yaw, player.pitch),
            settings: LevelSettings {
                seed: world_meta.seed,
                spawn_settings: SpawnSettings {
                    spawn_type: SpawnBiomeType::Default,
                    user_defined_biome_name: String::new(),
                    dimension: 0,
                },
                generator_type: bedrockrs_proto::v662::enums::GeneratorType::Overworld,
                game_type: GameType::Creative,
                is_hardcore_enabled: false,
                game_difficulty: Difficulty::Peaceful,
                default_spawn_block_position: block_pos_to_network(world_meta.spawn),
                achievements_disabled: true,
                editor_world_type: EditorWorldType::NonEditor,
                is_created_in_editor: false,
                is_exported_from_editor: false,
                day_cycle_stop_time: i32::try_from(world_meta.time).unwrap_or_default(),
                education_edition_offer: EducationEditionOffer::None,
                education_features_enabled: false,
                education_product_id: String::new(),
                rain_level: 0.0,
                lightning_level: 0.0,
                has_confirmed_platform_locked_content: false,
                multiplayer_enabled: true,
                lan_broadcasting_enabled: true,
                xbox_live_broadcast_setting: GamePublishSetting::FriendsOnly,
                platform_broadcast_setting: GamePublishSetting::FriendsOnly,
                commands_enabled: true,
                texture_packs_required: false,
                rule_data: bedrockrs_proto::v924::types::GameRuleLegacyData { rules_list: vec![] },
                experiments: Experiments {
                    experiments: vec![],
                    ever_toggled: false,
                },
                bonus_chest_enabled: false,
                starting_map_enabled: false,
                player_permissions: PlayerPermissionLevel::Custom,
                server_chunk_tick_range: 4,
                locked_behaviour_pack: false,
                locked_resource_pack: false,
                from_locked_template: false,
                use_msa_gamer_tags: false,
                from_template: false,
                has_locked_template_settings: false,
                only_spawn_v1_villagers: false,
                persona_disabled: false,
                custom_skins_disabled: false,
                emote_chat_muted: false,
                base_game_version: BaseGameVersion(V924::GAME_VERSION.to_string()),
                limited_world_width: 16,
                limited_world_depth: 16,
                nether_type: true,
                edu_shared_uri_resource: EduSharedUriResource {
                    button_name: String::new(),
                    link_uri: String::new(),
                },
                override_force_experimental_gameplay: Some(true),
                chat_restriction_level: ChatRestrictionLevel::None,
                disable_player_interactions: false,
            },
            level_id: "RevyCraft".to_string(),
            level_name: world_meta.level_name.clone(),
            template_content_identity: String::new(),
            is_trial: false,
            movement_settings: SyncedPlayerMovementSettings {
                rewind_history_size: 3200,
                server_authoritative_block_breaking: false,
            },
            current_level_time: u64::try_from(world_meta.time).unwrap_or_default(),
            enchantment_seed: 0,
            block_properties: vec![],
            multiplayer_correlation_id: player.id.0.to_string(),
            enable_item_stack_net_manager: false,
            server_version: V924::GAME_VERSION.to_string(),
            player_property_data: nbtx::Value::Compound(HashMap::new()),
            server_block_type_registry_checksum: 0,
            world_template_id: uuid::Uuid::nil(),
            server_enabled_client_side_generation: false,
            block_network_ids_are_hashes: false,
            network_permissions: NetworkPermissions {
                server_auth_sound_enabled: false,
            },
            server_join_information: None,
            server_id: String::new(),
            world_id: String::new(),
            scenario_id: String::new(),
            owner_id: String::new(),
        };
        Ok(vec![
            Self::encode_v924(&[V924::StartGamePacket(start_game)])?,
            Self::encode_v924(&[V924::PlayStatusPacket(PlayStatusPacket {
                status: PlayStatus::PlayerSpawn,
            })])?,
        ])
    }
}

impl HandshakeProbe for Bedrock263Adapter {
    fn transport_kind(&self) -> TransportKind {
        TransportKind::Udp
    }

    fn adapter_id(&self) -> Option<&'static str> {
        Some(BE_26_3_ADAPTER_ID)
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        if detects_bedrock_datagram(frame) {
            Ok(Some(bedrock_probe_intent()))
        } else {
            Ok(None)
        }
    }
}

impl SessionAdapter for Bedrock263Adapter {
    fn wire_codec(&self) -> &dyn WireCodec {
        &self.codec
    }

    fn decode_status(&self, _frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        Err(protocol_error(
            "bedrock status requests are handled by the raknet listener",
        ))
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        let packets = Self::decode_v924(frame)?;
        let packet = packets
            .into_iter()
            .next()
            .ok_or_else(|| protocol_error("bedrock login frame was empty"))?;
        match packet {
            V924::RequestNetworkSettingsPacket(RequestNetworkSettingsPacket {
                client_network_version,
            }) => Ok(LoginRequest::BedrockNetworkSettingsRequest {
                protocol_number: client_network_version,
            }),
            V924::LoginPacket(LoginPacket {
                client_network_version,
                connection_request,
            }) => {
                let login = parse_bedrock_login_payload(&connection_request)
                    .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
                Ok(LoginRequest::BedrockLogin {
                    protocol_number: client_network_version,
                    display_name: login.display_name,
                    chain_jwts: login.chain_jwts,
                    client_data_jwt: login.client_data_jwt,
                })
            }
            V924::ClientCacheStatusPacket(ClientCacheStatusPacket { .. })
            | V924::ResourcePackClientResponsePacket(ResourcePackClientResponsePacket { .. }) => {
                Err(protocol_error(
                    "bedrock login control packet arrived in login phase",
                ))
            }
            _ => Err(protocol_error("unsupported bedrock login packet")),
        }
    }

    fn encode_status_response(&self, _status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        Err(protocol_error(
            "bedrock status responses are handled by the raknet listener",
        ))
    }

    fn encode_status_pong(&self, _payload: i64) -> Result<Vec<u8>, ProtocolError> {
        Err(protocol_error(
            "bedrock status pong is handled by the raknet listener",
        ))
    }

    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        if matches!(phase, ConnectionPhase::Login) {
            return Self::play_status(PlayStatus::LoginFailedServerOld);
        }
        Self::encode_v924(&[V924::DisconnectPacket(DisconnectPacket {
            reason: ConnectionFailReason::Disconnected,
            message: Some(DisconnectMessage {
                kick_message: reason.to_string(),
                filtered_message: reason.to_string(),
            }),
        })])
    }

    fn encode_encryption_request(
        &self,
        _server_id: &str,
        _public_key_der: &[u8],
        _verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(protocol_error(
            "bedrock adapters do not use java edition encryption requests",
        ))
    }

    fn encode_network_settings(
        &self,
        compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        Self::encode_v924(&[V924::NetworkSettingsPacket(NetworkSettingsPacket {
            compression_threshold,
            compression_algorithm: PacketCompressionAlgorithm::ZLib,
            client_throttle_enabled: false,
            client_throttle_threshold: 0,
            client_throttle_scalar: 0.0,
        })])
    }

    fn encode_login_success(&self, _player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
        Self::encode_v924(&[
            V924::PlayStatusPacket(PlayStatusPacket {
                status: PlayStatus::LoginSuccess,
            }),
            V924::ResourcePacksInfoPacket(ResourcePacksInfoPacket {
                resource_pack_required: false,
                has_addon_packs: false,
                has_scripts: false,
                force_disable_vibrant_visuals: false,
                world_template_uuid: uuid::Uuid::nil(),
                world_template_version: String::new(),
                resource_packs: vec![],
            }),
            V924::ResourcePackStackPacket(ResourcePackStackPacket {
                texture_pack_required: false,
                addon_list: vec![],
                base_game_version: BaseGameVersion(V924::GAME_VERSION.to_string()),
                experiments: Experiments {
                    experiments: vec![],
                    ever_toggled: false,
                },
                include_editor_packs: false,
            }),
        ])
    }
}

impl PlaySyncAdapter for Bedrock263Adapter {
    fn decode_play(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError> {
        let packets = Self::decode_v924(frame)?;
        let Some(packet) = packets.into_iter().next() else {
            return Ok(None);
        };
        match packet {
            V924::MovePlayerPacket(MovePlayerPacket {
                position,
                rotation,
                on_ground,
                ..
            }) => Ok(Some(CoreCommand::MoveIntent {
                player_id,
                position: Some(mc_core::Vec3::new(
                    f64::from(position.x),
                    f64::from(position.y),
                    f64::from(position.z),
                )),
                yaw: Some(rotation.x),
                pitch: Some(rotation.y),
                on_ground,
            })),
            V924::MobEquipmentPacket(MobEquipmentPacket { slot, .. }) => {
                Ok(Some(CoreCommand::SetHeldSlot {
                    player_id,
                    slot: i16::from(slot),
                }))
            }
            V924::PlayerActionPacket(PlayerActionPacket {
                action:
                    PlayerActionType::StartDestroyBlock { .. }
                    | PlayerActionType::StopDestroyBlock { .. }
                    | PlayerActionType::CreativeDestroyBlock
                    | PlayerActionType::PredictDestroyBlock { .. },
                block_position,
                face,
                ..
            }) => Ok(Some(CoreCommand::DigBlock {
                player_id,
                position: block_pos_from_network(&block_position),
                status: 2,
                face: block_face_from_i32(face),
            })),
            V924::ClientCacheStatusPacket(_) | V924::ResourcePackClientResponsePacket(_) => {
                Ok(None)
            }
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
            } => Self::start_game_packets(player, *entity_id, world_meta),
            CoreEvent::EntityMoved { entity_id, player } => {
                Ok(vec![Self::encode_v924(&[V924::MovePlayerPacket(
                    MovePlayerPacket {
                        player_runtime_id: ActorRuntimeID(bedrock_actor_id(*entity_id)),
                        position: vec3_to_bedrock(player.position),
                        rotation: Vec2::new(player.yaw, player.pitch),
                        y_head_rotation: player.yaw,
                        position_mode: bedrockrs_proto::v662::enums::PlayerPositionMode::Normal,
                        on_ground: player.on_ground,
                        riding_runtime_id: ActorRuntimeID(0),
                        tick: 0,
                    },
                )])?])
            }
            CoreEvent::BlockChanged { position, block } => {
                Ok(vec![Self::encode_v924(&[V924::UpdateBlockPacket(
                    UpdateBlockPacket {
                        block_position: block_pos_to_network(*position),
                        block_runtime_id: block_runtime_id(block),
                        flags: 0,
                        layer: 0,
                    },
                )])?])
            }
            CoreEvent::KeepAliveRequested { .. }
            | CoreEvent::ChunkBatch { .. }
            | CoreEvent::EntitySpawned { .. }
            | CoreEvent::EntityDespawned { .. }
            | CoreEvent::InventoryContents { .. }
            | CoreEvent::InventorySlotChanged { .. }
            | CoreEvent::SelectedHotbarSlotChanged { .. }
            | CoreEvent::LoginAccepted { .. }
            | CoreEvent::Disconnect { .. } => Ok(Vec::new()),
        }
    }
}

impl ProtocolAdapter for Bedrock263Adapter {
    fn descriptor(&self) -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: BE_26_3_ADAPTER_ID.to_string(),
            transport: TransportKind::Udp,
            wire_format: WireFormatKind::RawPacketStream,
            edition: Edition::Be,
            version_name: BE_26_3_VERSION_NAME.to_string(),
            protocol_number: BE_26_3_PROTOCOL_NUMBER,
        }
    }

    fn bedrock_listener_descriptor(&self) -> Option<BedrockListenerDescriptor> {
        Some(BedrockListenerDescriptor {
            game_version: V924::GAME_VERSION.to_string(),
            raknet_version: V924::RAKNET_VERSION,
        })
    }
}

const fn block_face_from_i32(face: i32) -> Option<BlockFace> {
    match face {
        0 => Some(BlockFace::Bottom),
        1 => Some(BlockFace::Top),
        2 => Some(BlockFace::North),
        3 => Some(BlockFace::South),
        4 => Some(BlockFace::West),
        5 => Some(BlockFace::East),
        _ => None,
    }
}

fn bedrock_actor_id(entity_id: mc_core::EntityId) -> u64 {
    u64::try_from(entity_id.0).expect("bedrock entity id should be non-negative")
}

fn block_runtime_id(block: &BlockState) -> u32 {
    match block.key.as_str() {
        COBBLESTONE => BEDROCK_26_3_RUNTIME_ID_COBBLESTONE,
        SAND => BEDROCK_26_3_RUNTIME_ID_SAND,
        BRICKS => BEDROCK_26_3_RUNTIME_ID_BRICKS,
        DIRT => BEDROCK_26_3_RUNTIME_ID_DIRT,
        GRASS_BLOCK => BEDROCK_26_3_RUNTIME_ID_GRASS_BLOCK,
        GLASS => BEDROCK_26_3_RUNTIME_ID_GLASS,
        "minecraft:air" => BEDROCK_26_3_RUNTIME_ID_AIR,
        BEDROCK => BEDROCK_26_3_RUNTIME_ID_BEDROCK,
        OAK_PLANKS => BEDROCK_26_3_RUNTIME_ID_OAK_PLANKS,
        _ => BEDROCK_26_3_RUNTIME_ID_STONE,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BE_26_3_PROTOCOL_NUMBER, BEDROCK_26_3_RUNTIME_ID_AIR, BEDROCK_26_3_RUNTIME_ID_BEDROCK,
        BEDROCK_26_3_RUNTIME_ID_BRICKS, BEDROCK_26_3_RUNTIME_ID_COBBLESTONE,
        BEDROCK_26_3_RUNTIME_ID_DIRT, BEDROCK_26_3_RUNTIME_ID_GLASS,
        BEDROCK_26_3_RUNTIME_ID_GRASS_BLOCK, BEDROCK_26_3_RUNTIME_ID_OAK_PLANKS,
        BEDROCK_26_3_RUNTIME_ID_SAND, BEDROCK_26_3_RUNTIME_ID_STONE, Bedrock263Adapter,
        block_runtime_id,
    };
    use base64::Engine;
    use bedrockrs_proto::V924;
    use bedrockrs_proto::codec::encode_packets;
    use bedrockrs_proto::v662::packets::{LoginPacket, RequestNetworkSettingsPacket};
    use mc_core::BlockState;
    use mc_proto_common::{HandshakeProbe, LoginRequest, SessionAdapter};
    use serde_json::json;

    fn test_jwt(payload: &serde_json::Value) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        format!("{header}.{payload}.")
    }

    #[test]
    fn request_network_settings_maps_to_login_request() {
        let adapter = Bedrock263Adapter::new();
        let frame = encode_packets(
            &[V924::RequestNetworkSettingsPacket(
                RequestNetworkSettingsPacket {
                    client_network_version: BE_26_3_PROTOCOL_NUMBER,
                },
            )],
            None,
            None,
        )
        .expect("request should encode");
        let request = adapter.decode_login(&frame).expect("request should decode");
        assert_eq!(
            request,
            LoginRequest::BedrockNetworkSettingsRequest {
                protocol_number: BE_26_3_PROTOCOL_NUMBER
            }
        );
    }

    #[test]
    fn login_packet_maps_to_bedrock_login_request() {
        let adapter = Bedrock263Adapter::new();
        let chain_entry = test_jwt(&json!({"extraData":{"displayName":"Builder"}}));
        let chain = json!({ "chain": [chain_entry] }).to_string();
        let client_jwt = test_jwt(&json!({"DisplayName":"Builder"}));
        let mut connection_request = Vec::new();
        let chain_len = u32::try_from(chain.len()).expect("test chain jwt should fit in u32");
        connection_request.extend_from_slice(&chain_len.to_le_bytes());
        connection_request.extend_from_slice(chain.as_bytes());
        let client_jwt_len =
            u32::try_from(client_jwt.len()).expect("test client jwt should fit in u32");
        connection_request.extend_from_slice(&client_jwt_len.to_le_bytes());
        connection_request.extend_from_slice(client_jwt.as_bytes());
        let frame = encode_packets(
            &[V924::LoginPacket(LoginPacket {
                client_network_version: BE_26_3_PROTOCOL_NUMBER,
                connection_request,
            })],
            None,
            None,
        )
        .expect("login packet should encode");
        let request = adapter.decode_login(&frame).expect("login should decode");
        match request {
            LoginRequest::BedrockLogin {
                protocol_number,
                display_name,
                ..
            } => {
                assert_eq!(protocol_number, BE_26_3_PROTOCOL_NUMBER);
                assert_eq!(display_name, "Builder");
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn probe_matches_raknet_datagram() {
        let adapter = Bedrock263Adapter::new();
        let mut datagram = Vec::new();
        datagram.push(0x01);
        datagram.extend_from_slice(&123_i64.to_be_bytes());
        datagram.extend_from_slice(&bedrockrs_proto::info::MAGIC);
        datagram.extend_from_slice(&456_i64.to_be_bytes());
        assert!(
            adapter
                .try_route(&datagram)
                .expect("probe should succeed")
                .is_some()
        );
    }

    #[test]
    fn supported_block_runtime_ids_match_bedrock_1_26_0_palette() {
        assert_eq!(
            block_runtime_id(&BlockState::stone()),
            BEDROCK_26_3_RUNTIME_ID_STONE
        );
        assert_eq!(
            block_runtime_id(&BlockState::cobblestone()),
            BEDROCK_26_3_RUNTIME_ID_COBBLESTONE
        );
        assert_eq!(
            block_runtime_id(&BlockState::sand()),
            BEDROCK_26_3_RUNTIME_ID_SAND
        );
        assert_eq!(
            block_runtime_id(&BlockState::bricks()),
            BEDROCK_26_3_RUNTIME_ID_BRICKS
        );
        assert_eq!(
            block_runtime_id(&BlockState::dirt()),
            BEDROCK_26_3_RUNTIME_ID_DIRT
        );
        assert_eq!(
            block_runtime_id(&BlockState::grass_block()),
            BEDROCK_26_3_RUNTIME_ID_GRASS_BLOCK
        );
        assert_eq!(
            block_runtime_id(&BlockState::glass()),
            BEDROCK_26_3_RUNTIME_ID_GLASS
        );
        assert_eq!(
            block_runtime_id(&BlockState::air()),
            BEDROCK_26_3_RUNTIME_ID_AIR
        );
        assert_eq!(
            block_runtime_id(&BlockState::bedrock()),
            BEDROCK_26_3_RUNTIME_ID_BEDROCK
        );
        assert_eq!(
            block_runtime_id(&BlockState::oak_planks()),
            BEDROCK_26_3_RUNTIME_ID_OAK_PLANKS
        );
    }
}
