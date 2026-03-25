use crate::chunk::level_chunk_packet;
use crate::codec::encode_v924;
use crate::inventory::{
    encode_container_closed_packets as encode_bedrock_container_closed_packets,
    encode_container_opened_packets as encode_bedrock_container_opened_packets,
    encode_container_property_changed_packets as encode_bedrock_container_property_changed_packets,
    encode_creative_content_packet,
    encode_inventory_contents_packets as encode_bedrock_inventory_contents_packets,
    encode_inventory_slot_changed_packets as encode_bedrock_inventory_slot_changed_packets,
    encode_selected_hotbar_slot_changed_packets as encode_bedrock_selected_hotbar_slot_changed_packets,
};
use crate::runtime_ids::block_runtime_id;
use bedrockrs_proto::ProtoVersion;
use bedrockrs_proto::V924;
use bedrockrs_proto::v662::enums::{
    ConnectionFailReason, Difficulty, EditorWorldType, EducationEditionOffer, GamePublishSetting,
    GameType, PacketCompressionAlgorithm, PlayStatus, PlayerPermissionLevel, SpawnBiomeType,
};
use bedrockrs_proto::v662::packets::{
    MovePlayerPacket, NetworkSettingsPacket, PlayStatusPacket, UpdateBlockPacket,
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
use mc_core::{
    BlockPos, BlockState, ChunkColumn, EntityId, InventoryContainer, InventorySlot,
    InventoryWindowContents, ItemStack, PlayerSnapshot, WorldMeta,
};
use mc_proto_be_common::__version_support::world::{
    bedrock_actor_id, block_pos_to_network, vec3_to_bedrock,
};
use mc_proto_common::{ConnectionPhase, ProtocolError};
use std::collections::HashMap;
use vek::Vec2;

pub(crate) fn encode_disconnect_packet(
    phase: ConnectionPhase,
    reason: &str,
) -> Result<Vec<u8>, ProtocolError> {
    if matches!(phase, ConnectionPhase::Login) {
        return play_status(PlayStatus::LoginFailedServerOld);
    }
    encode_v924(&[V924::DisconnectPacket(DisconnectPacket {
        reason: ConnectionFailReason::Disconnected,
        message: Some(DisconnectMessage {
            kick_message: reason.to_string(),
            filtered_message: reason.to_string(),
        }),
    })])
}

pub(crate) fn encode_network_settings_packet(
    compression_threshold: u16,
) -> Result<Vec<u8>, ProtocolError> {
    encode_v924(&[V924::NetworkSettingsPacket(NetworkSettingsPacket {
        compression_threshold,
        compression_algorithm: PacketCompressionAlgorithm::ZLib,
        client_throttle_enabled: false,
        client_throttle_threshold: 0,
        client_throttle_scalar: 0.0,
    })])
}

pub(crate) fn encode_login_success_packet(
    _player: &PlayerSnapshot,
) -> Result<Vec<u8>, ProtocolError> {
    encode_v924(&[
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

pub(crate) fn encode_play_bootstrap_packets(
    player: &PlayerSnapshot,
    entity_id: EntityId,
    world_meta: &WorldMeta,
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
            chat_restriction_level: bedrockrs_proto::v662::enums::ChatRestrictionLevel::None,
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
        player_property_data: HashMap::new(),
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
    let mut packets = vec![
        encode_v924(&[V924::StartGamePacket(start_game)])?,
        encode_v924(&[V924::PlayStatusPacket(PlayStatusPacket {
            status: PlayStatus::PlayerSpawn,
        })])?,
    ];
    packets.extend(encode_creative_content_packet()?);
    Ok(packets)
}

pub(crate) fn encode_entity_moved_packets(
    entity_id: EntityId,
    player: &PlayerSnapshot,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    Ok(vec![encode_v924(&[V924::MovePlayerPacket(
        MovePlayerPacket {
            player_runtime_id: ActorRuntimeID(bedrock_actor_id(entity_id)),
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

pub(crate) fn encode_chunk_batch_packets(
    chunks: &[ChunkColumn],
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    chunks
        .iter()
        .map(level_chunk_packet)
        .map(|packet| packet.and_then(|packet| encode_v924(&[packet]).map(|payload| vec![payload])))
        .collect::<Result<Vec<_>, _>>()
        .map(|packets| packets.into_iter().flatten().collect())
}

pub(crate) fn encode_block_changed_packets(
    position: BlockPos,
    block: &BlockState,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    Ok(vec![encode_v924(&[V924::UpdateBlockPacket(
        UpdateBlockPacket {
            block_position: block_pos_to_network(position),
            block_runtime_id: block_runtime_id(block),
            flags: 0,
            layer: 0,
        },
    )])?])
}

pub(crate) fn encode_inventory_contents_packets(
    window_id: u8,
    container: InventoryContainer,
    contents: &InventoryWindowContents,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    encode_bedrock_inventory_contents_packets(window_id, container, contents)
}

pub(crate) fn encode_container_opened_packets(
    window_id: u8,
    container: InventoryContainer,
    _title: &str,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    encode_bedrock_container_opened_packets(window_id, container)
}

pub(crate) fn encode_container_closed_packets(
    window_id: u8,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    encode_bedrock_container_closed_packets(window_id)
}

pub(crate) fn encode_container_property_changed_packets(
    window_id: u8,
    property_id: u8,
    value: i16,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    encode_bedrock_container_property_changed_packets(window_id, property_id, value)
}

pub(crate) fn encode_inventory_slot_changed_packets(
    window_id: u8,
    container: InventoryContainer,
    slot: InventorySlot,
    stack: Option<&ItemStack>,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    encode_bedrock_inventory_slot_changed_packets(window_id, container, slot, stack)
}

pub(crate) fn encode_selected_hotbar_slot_changed_packets(
    slot: u8,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    encode_bedrock_selected_hotbar_slot_changed_packets(slot)
}

fn play_status(status: PlayStatus) -> Result<Vec<u8>, ProtocolError> {
    encode_v924(&[V924::PlayStatusPacket(PlayStatusPacket { status })])
}
