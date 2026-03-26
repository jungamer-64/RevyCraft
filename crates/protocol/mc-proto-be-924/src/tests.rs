use crate::runtime_ids::{
    BEDROCK_26_3_RUNTIME_ID_AIR, BEDROCK_26_3_RUNTIME_ID_BEDROCK, BEDROCK_26_3_RUNTIME_ID_BRICKS,
    BEDROCK_26_3_RUNTIME_ID_COBBLESTONE, BEDROCK_26_3_RUNTIME_ID_DIRT,
    BEDROCK_26_3_RUNTIME_ID_GLASS, BEDROCK_26_3_RUNTIME_ID_GRASS_BLOCK,
    BEDROCK_26_3_RUNTIME_ID_OAK_PLANKS, BEDROCK_26_3_RUNTIME_ID_SAND,
    BEDROCK_26_3_RUNTIME_ID_STONE, block_runtime_id,
};
use crate::{BE_924_PROTOCOL_NUMBER, Bedrock924Adapter};
use base64::Engine;
use bedrockrs_proto::V924;
use bedrockrs_proto::codec::{decode_packets, encode_packets};
use bedrockrs_proto::v662::enums::{
    ComplexInventoryTransactionType, ContainerEnumName, InputMode, ItemUseInventoryTransactionType,
    LevelEvent as BedrockLevelEvent, NewInteractionModel, PlayerActionType,
    TextProcessingEventOrigin,
};
use bedrockrs_proto::v662::packets::{
    ItemStackRequestPacket, LegacySetItemSlotsEntry, LoginPacket, PlayerActionPacket,
    RequestNetworkSettingsPacket, RequestsEntry,
};
use bedrockrs_proto::v662::types::{
    ActorRuntimeID, NetworkBlockPosition, NetworkItemStackDescriptor,
};
use bedrockrs_proto::v712::enums::ItemStackRequestActionType;
use bedrockrs_proto::v712::types::{
    ItemStackRequestSlotInfo, PackedItemUseLegacyInventoryTransaction, PredictedResult, TriggerType,
};
use bedrockrs_proto::v729::types::FullContainerName;
use bedrockrs_proto::v766::packets::ClientPlayMode;
use bedrockrs_proto::v766::packets::PlayerAuthInputPacket;
use bedrockrs_proto::v766::packets::player_auth_input_packet::PlayerAuthInputFlags;
use bedrockrs_proto_core::{PacketHeader, ProtoCodec, ProtoCodecLE, ProtoCodecVAR};
use mc_core::{
    BlockPos, BlockState, ChunkColumn, ChunkPos, CoreCommand, CoreEvent, DroppedItemSnapshot,
    EntityId, InventoryClickButton, InventoryClickTarget, InventoryClickValidation,
    InventoryContainer, InventorySlot, InventoryTransactionContext, InventoryWindowContents,
    ItemStack, PlayerId, PlayerInventory, RuntimeCommand,
};
use mc_proto_be_common::__version_support::world::bedrock_actor_id;
use mc_proto_common::{
    ConnectionPhase, HandshakeProbe, LoginRequest, PlayEncodingContext, PlaySyncAdapter,
    ProtocolSessionSnapshot, SessionAdapter,
};
use serde_json::json;
use std::io::Cursor;
use uuid::Uuid;
use vek::{Vec2, Vec3};

fn test_jwt(payload: &serde_json::Value) -> String {
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
    let payload =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
    format!("{header}.{payload}.")
}

fn decode_session(player_id: PlayerId) -> ProtocolSessionSnapshot {
    ProtocolSessionSnapshot {
        connection_id: mc_core::ConnectionId(1),
        phase: ConnectionPhase::Play,
        player_id: Some(player_id),
        entity_id: None,
    }
}

fn encode_session(context: &PlayEncodingContext) -> ProtocolSessionSnapshot {
    ProtocolSessionSnapshot {
        connection_id: mc_core::ConnectionId(1),
        phase: ConnectionPhase::Play,
        player_id: Some(context.player_id),
        entity_id: Some(context.entity_id),
    }
}

trait TestPlaySyncAdapterExt: PlaySyncAdapter {
    fn decode_play_for(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<RuntimeCommand>, mc_proto_common::ProtocolError> {
        mc_proto_common::PlaySyncAdapter::decode_play(self, &decode_session(player_id), frame)
    }

    fn encode_play_event_for(
        &self,
        event: &CoreEvent,
        context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, mc_proto_common::ProtocolError> {
        mc_proto_common::PlaySyncAdapter::encode_play_event(
            self,
            event,
            &encode_session(context),
            context,
        )
    }
}

impl<T: PlaySyncAdapter + ?Sized> TestPlaySyncAdapterExt for T {}

#[test]
fn request_network_settings_maps_to_login_request() {
    let adapter = Bedrock924Adapter::new();
    let frame = encode_packets(
        &[V924::RequestNetworkSettingsPacket(
            RequestNetworkSettingsPacket {
                client_network_version: BE_924_PROTOCOL_NUMBER,
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
            protocol_number: BE_924_PROTOCOL_NUMBER
        }
    );
}

#[test]
fn login_packet_maps_to_bedrock_login_request() {
    let adapter = Bedrock924Adapter::new();
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
            client_network_version: BE_924_PROTOCOL_NUMBER,
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
            assert_eq!(protocol_number, BE_924_PROTOCOL_NUMBER);
            assert_eq!(display_name, "Builder");
        }
        other => panic!("unexpected request: {other:?}"),
    }
}

#[test]
fn probe_matches_raknet_datagram() {
    let adapter = Bedrock924Adapter::new();
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

#[test]
fn encodes_chunk_and_block_packets() {
    let adapter = Bedrock924Adapter::new();
    let mut chunk = ChunkColumn::new(ChunkPos::new(0, 0));
    chunk.set_block(1, 4, 2, BlockState::bricks());

    let frames = adapter
        .encode_play_event_for(
            &CoreEvent::ChunkBatch {
                chunks: vec![chunk],
            },
            &play_context(),
        )
        .expect("chunk batch should encode");
    let packets =
        decode_packets::<V924>(frames[0].clone(), None, None).expect("chunk packet should decode");
    match packets.as_slice() {
        [V924::LevelChunkPacket(packet)] => {
            assert_eq!(packet.chunk_position.x, 0);
            assert_eq!(packet.chunk_position.z, 0);
            assert!(!packet.serialized_chunk_data.is_empty());
        }
        other => panic!("unexpected chunk packets: {other:?}"),
    }

    let frames = adapter
        .encode_play_event_for(
            &CoreEvent::BlockChanged {
                position: BlockPos::new(2, 3, 4),
                block: BlockState::glass(),
            },
            &play_context(),
        )
        .expect("block change should encode");
    let packets =
        decode_packets::<V924>(frames[0].clone(), None, None).expect("block packet should decode");
    match packets.as_slice() {
        [V924::UpdateBlockPacket(packet)] => {
            assert_eq!(packet.block_position.x, 2);
            assert_eq!(packet.block_position.y, 3);
            assert_eq!(packet.block_position.z, 4);
            assert_eq!(
                packet.block_runtime_id,
                block_runtime_id(&BlockState::glass())
            );
        }
        other => panic!("unexpected block packets: {other:?}"),
    }
}

#[test]
fn encodes_inventory_and_container_packets() {
    let adapter = Bedrock924Adapter::new();
    let mut inventory = PlayerInventory::creative_starter();
    let _ = inventory.set_slot(InventorySlot::Offhand, Some(item("minecraft:stick", 1)));
    let contents = InventoryWindowContents::with_container(
        inventory,
        vec![Some(item("minecraft:chest", 1)); 27],
    );

    let frames = adapter
        .encode_play_event_for(
            &CoreEvent::InventoryContents {
                window_id: 2,
                container: InventoryContainer::Chest,
                contents,
            },
            &play_context(),
        )
        .expect("inventory contents should encode");
    let packets = decode_packets::<V924>(frames[0].clone(), None, None)
        .expect("inventory contents packet should decode");
    assert!(
        packets.iter().any(|packet| matches!(
            packet,
            V924::InventoryContentPacket(content) if content.inventory_id == 1 && content.slots.len() == 27
        )),
        "active container inventory should be present",
    );
    assert!(
        packets.iter().any(|packet| matches!(
            packet,
            V924::InventoryContentPacket(content) if content.inventory_id == 0 && content.slots.len() == 36
        )),
        "player storage inventory should be present",
    );
    assert!(
        packets.iter().any(|packet| matches!(
            packet,
            V924::InventoryContentPacket(content) if content.inventory_id == 119 && content.slots.len() == 1
        )),
        "offhand inventory should be present",
    );

    let frames = adapter
        .encode_play_event_for(
            &CoreEvent::InventorySlotChanged {
                window_id: 2,
                container: InventoryContainer::Chest,
                slot: InventorySlot::Container(0),
                stack: Some(item("minecraft:chest", 1)),
            },
            &play_context(),
        )
        .expect("inventory slot change should encode");
    let packets = decode_packets::<V924>(frames[0].clone(), None, None)
        .expect("inventory slot packet should decode");
    match packets.as_slice() {
        [V924::InventorySlotPacket(packet)] => {
            assert_eq!(packet.container_id, 1);
            assert_eq!(packet.slot, 0);
        }
        other => panic!("unexpected slot packets: {other:?}"),
    }

    let frames = adapter
        .encode_play_event_for(
            &CoreEvent::SelectedHotbarSlotChanged { slot: 4 },
            &play_context(),
        )
        .expect("selected hotbar slot should encode");
    let packets =
        decode_packets::<V924>(frames[0].clone(), None, None).expect("hotbar packet should decode");
    assert!(matches!(
        packets.as_slice(),
        [V924::PlayerHotbarPacket(packet)] if packet.selected_slot == 4
    ));

    let frames = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerOpened {
                window_id: 2,
                container: InventoryContainer::CraftingTable,
                title: "Crafting".to_string(),
            },
            &play_context(),
        )
        .expect("container open should encode");
    let packets =
        decode_packets::<V924>(frames[0].clone(), None, None).expect("open packet should decode");
    assert!(matches!(
        packets.as_slice(),
        [V924::ContainerOpenPacket(packet)]
            if matches!(packet.container_id, bedrockrs_proto::v662::enums::ContainerID::First)
                && matches!(packet.container_type, bedrockrs_proto::v662::enums::ContainerType::Workbench)
    ));

    let frames = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerClosed { window_id: 2 },
            &play_context(),
        )
        .expect("container close should encode");
    let packets =
        decode_packets::<V924>(frames[0].clone(), None, None).expect("close packet should decode");
    assert!(matches!(
        packets.as_slice(),
        [V924::ContainerClosePacket(packet)] if packet.server_initiated_close
    ));

    let frames = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerPropertyChanged {
                window_id: 2,
                property_id: 1,
                value: 200,
            },
            &play_context(),
        )
        .expect("container property should encode");
    let packets = decode_packets::<V924>(frames[0].clone(), None, None)
        .expect("property packet should decode");
    assert!(matches!(
        packets.as_slice(),
        [V924::ContainerSetDataPacket(packet)] if packet.id == 1 && packet.value == 200
    ));
}

#[test]
fn decodes_legacy_inventory_transaction_item_use() {
    let adapter = Bedrock924Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(
        &Uuid::NAMESPACE_OID,
        b"bedrock-legacy-item-use",
    ));
    let frame = encode_legacy_item_use_transaction(sample_item_use_transaction(
        ItemUseInventoryTransactionType::Place,
        BlockPos::new(2, 3, 4),
        1,
    ));

    let command = adapter
        .decode_play_for(player_id, &frame)
        .expect("legacy inventory transaction should decode")
        .expect("legacy inventory transaction should produce a command");
    assert!(matches!(
        command,
        RuntimeCommand::Core(CoreCommand::PlaceBlock {
            player_id: decoded_player,
            position,
            face: Some(mc_core::BlockFace::Top),
            ..
        }) if decoded_player == player_id && position == BlockPos::new(2, 3, 4)
    ));
}

#[test]
fn decodes_player_auth_input_item_use() {
    let adapter = Bedrock924Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"bedrock-auth-item-use"));
    let frame = encode_player_auth_input(PlayerAuthInputPacket {
        player_rotation: Vec2::new(0.0, 0.0),
        player_position: Vec3::new(0.0, 0.0, 0.0),
        move_vector: Vec3::new(0.0, 0.0, 0.0),
        player_head_rotation: 0.0,
        input_data: PlayerAuthInputFlags::PerformItemInteraction as u128,
        input_mode: InputMode::Mouse,
        play_mode: ClientPlayMode::Normal,
        new_interaction_model: NewInteractionModel::Crosshair,
        interact_rotation: Vec3::new(0.0, 0.0, 0.0),
        client_tick: 0,
        velocity: Vec3::new(0.0, 0.0, 0.0),
        item_use_transaction: Some(sample_item_use_transaction(
            ItemUseInventoryTransactionType::Destroy,
            BlockPos::new(5, 6, 7),
            4,
        )),
        item_stack_request: None,
        player_block_actions: None,
        client_predicted_vehicle: None,
        analog_move_vector: Vec2::new(0.0, 0.0),
        camera_orientation: Vec3::new(0.0, 0.0, 0.0),
        raw_move_vector: Vec2::new(0.0, 0.0),
    });

    let command = adapter
        .decode_play_for(player_id, &frame)
        .expect("player auth input should decode")
        .expect("player auth input should produce a command");
    assert!(matches!(
        command,
        RuntimeCommand::Core(CoreCommand::DigBlock {
            player_id: decoded_player,
            position,
            face: Some(mc_core::BlockFace::West),
            ..
        }) if decoded_player == player_id && position == BlockPos::new(5, 6, 7)
    ));
}

#[test]
fn decodes_item_stack_request_take_action() {
    let adapter = Bedrock924Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"bedrock-stack-request"));
    let frame = encode_packets(
        &[V924::ItemStackRequestPacket(ItemStackRequestPacket {
            requests: vec![RequestsEntry {
                client_request_id: 12,
                actions: vec![ItemStackRequestActionType::Take {
                    amount: 64,
                    source: request_slot(ContainerEnumName::HotbarContainer, 0),
                    destination: request_slot(ContainerEnumName::CursorContainer, 0),
                }],
                strings_to_filter: Vec::new(),
                strings_to_filter_origin: TextProcessingEventOrigin::Unknown,
            }],
        })],
        None,
        None,
    )
    .expect("item stack request should encode");

    let command = adapter
        .decode_play_for(player_id, &frame)
        .expect("item stack request should decode")
        .expect("item stack request should produce a command");
    assert!(matches!(
        command,
        RuntimeCommand::Core(CoreCommand::InventoryClick {
            player_id: decoded_player,
            transaction: InventoryTransactionContext {
                window_id: 0,
                action_number: 12,
            },
            target: InventoryClickTarget::Slot(InventorySlot::Hotbar(0)),
            button: InventoryClickButton::Left,
            validation: InventoryClickValidation::Authoritative,
        }) if decoded_player == player_id
    ));
}

#[test]
fn encodes_dropped_item_spawn_and_despawn_packets() {
    let adapter = Bedrock924Adapter::new();
    let frames = adapter
        .encode_play_event_for(
            &CoreEvent::DroppedItemSpawned {
                entity_id: EntityId(77),
                item: DroppedItemSnapshot {
                    item: item("minecraft:cobblestone", 1),
                    position: mc_core::Vec3::new(1.5, 4.5, 0.5),
                    velocity: mc_core::Vec3::new(0.0, 0.0, 0.0),
                },
            },
            &play_context(),
        )
        .expect("dropped item should encode");
    let packets =
        decode_packets::<V924>(frames[0].clone(), None, None).expect("item actor should decode");
    match packets.as_slice() {
        [V924::AddItemActorPacket(packet)] => {
            assert_eq!(packet.target_actor_id.0, bedrock_actor_id(EntityId(77)));
            assert_eq!(packet.position.x, 1.5);
            assert_eq!(packet.position.y, 4.5);
            assert_eq!(packet.position.z, 0.5);

            let mut item_bytes = Vec::new();
            packet
                .item
                .serialize(&mut item_bytes)
                .expect("item descriptor should serialize");
            let mut cursor = Cursor::new(item_bytes);
            let item_id =
                <i32 as ProtoCodecVAR>::deserialize(&mut cursor).expect("item id should decode");
            let count =
                <u16 as ProtoCodecLE>::deserialize(&mut cursor).expect("count should decode");
            assert_ne!(item_id, 0);
            assert_eq!(count, 1);
        }
        other => panic!("unexpected dropped item packets: {other:?}"),
    }

    let frames = adapter
        .encode_play_event_for(
            &CoreEvent::EntityDespawned {
                entity_ids: vec![EntityId(77)],
            },
            &play_context(),
        )
        .expect("despawn should encode");
    let packets = decode_packets::<V924>(frames[0].clone(), None, None)
        .expect("despawn packet should decode");
    match packets.as_slice() {
        [V924::RemoveActorPacket(packet)] => {
            assert_eq!(packet.target_actor_id.0, bedrock_actor_id(EntityId(77)));
        }
        other => panic!("unexpected despawn packets: {other:?}"),
    }
}

#[test]
fn encodes_block_break_progress_packets() {
    let adapter = Bedrock924Adapter::new();

    let start = adapter
        .encode_play_event_for(
            &CoreEvent::BlockBreakingProgress {
                breaker_entity_id: EntityId(77),
                position: BlockPos::new(2, 4, 0),
                stage: Some(0),
                duration_ms: 750,
            },
            &play_context(),
        )
        .expect("break start should encode");
    let packets =
        decode_packets::<V924>(start[0].clone(), None, None).expect("level event should decode");
    match packets.as_slice() {
        [V924::LevelEventPacket(packet)] => {
            assert_eq!(
                packet.event_id,
                BedrockLevelEvent::StartBlockCracking as i32
            );
            assert_eq!(packet.position.x, 2.0);
            assert_eq!(packet.position.y, 4.0);
            assert_eq!(packet.position.z, 0.0);
            assert!(packet.data > 0);
        }
        other => panic!("unexpected break start packets: {other:?}"),
    }

    let update = adapter
        .encode_play_event_for(
            &CoreEvent::BlockBreakingProgress {
                breaker_entity_id: EntityId(77),
                position: BlockPos::new(2, 4, 0),
                stage: Some(5),
                duration_ms: 750,
            },
            &play_context(),
        )
        .expect("break update should encode");
    let packets =
        decode_packets::<V924>(update[0].clone(), None, None).expect("level event should decode");
    match packets.as_slice() {
        [V924::LevelEventPacket(packet)] => {
            assert_eq!(
                packet.event_id,
                BedrockLevelEvent::UpdateBlockCracking as i32
            );
            assert_eq!(packet.data, 5);
        }
        other => panic!("unexpected break update packets: {other:?}"),
    }

    let stop = adapter
        .encode_play_event_for(
            &CoreEvent::BlockBreakingProgress {
                breaker_entity_id: EntityId(77),
                position: BlockPos::new(2, 4, 0),
                stage: None,
                duration_ms: 750,
            },
            &play_context(),
        )
        .expect("break stop should encode");
    let packets =
        decode_packets::<V924>(stop[0].clone(), None, None).expect("level event should decode");
    match packets.as_slice() {
        [V924::LevelEventPacket(packet)] => {
            assert_eq!(packet.event_id, BedrockLevelEvent::StopBlockCracking as i32);
            assert_eq!(packet.data, 0);
        }
        other => panic!("unexpected break stop packets: {other:?}"),
    }
}

#[test]
fn decodes_player_action_destroy_packets_to_dig_statuses() {
    let adapter = Bedrock924Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"bedrock-break-actions"));

    for (action, expected_status) in [
        (PlayerActionType::StartDestroyBlock, 0_u8),
        (PlayerActionType::ContinueDestroyBlock, 0_u8),
        (PlayerActionType::AbortDestroyBlock, 1_u8),
        (PlayerActionType::StopDestroyBlock, 1_u8),
        (PlayerActionType::CreativeDestroyBlock, 2_u8),
        (PlayerActionType::PredictDestroyBlock, 2_u8),
    ] {
        let frame = encode_packets(
            &[V924::PlayerActionPacket(PlayerActionPacket {
                player_runtime_id: ActorRuntimeID(1),
                action,
                block_position: NetworkBlockPosition { x: 2, y: 4, z: 0 },
                result_pos: NetworkBlockPosition { x: 2, y: 4, z: 0 },
                face: 1,
            })],
            None,
            None,
        )
        .expect("player action should encode");
        let command = adapter
            .decode_play_for(player_id, &frame)
            .expect("player action should decode")
            .expect("player action should produce a command");
        assert!(matches!(
            command,
            RuntimeCommand::Core(CoreCommand::DigBlock {
                player_id: decoded_player,
                position,
                status,
                face: Some(mc_core::BlockFace::Top),
            }) if decoded_player == player_id
                && position == BlockPos::new(2, 4, 0)
                && status == expected_status
        ));
    }
}

fn play_context() -> PlayEncodingContext {
    PlayEncodingContext {
        player_id: PlayerId(Uuid::new_v3(
            &Uuid::NAMESPACE_OID,
            b"bedrock-encode-context",
        )),
        entity_id: EntityId(1),
    }
}

fn item(key: &str, count: u8) -> ItemStack {
    ItemStack::new(key, count, 0)
}

fn request_slot(container: ContainerEnumName, slot: i8) -> ItemStackRequestSlotInfo<V924> {
    ItemStackRequestSlotInfo {
        container_name: full_container_name(container),
        slot,
        raw_id: 0,
    }
}

fn full_container_name(container: ContainerEnumName) -> FullContainerName<V924> {
    let mut bytes = Vec::new();
    container
        .serialize(&mut bytes)
        .expect("container enum should serialize");
    <Option<i32> as ProtoCodecLE>::serialize(&None, &mut bytes)
        .expect("dynamic id should serialize");
    FullContainerName::deserialize(&mut Cursor::new(bytes))
        .expect("full container name should deserialize")
}

fn empty_item_stack_descriptor() -> NetworkItemStackDescriptor {
    let mut bytes = Vec::new();
    <i32 as ProtoCodecVAR>::serialize(&0, &mut bytes).expect("empty item id should serialize");
    NetworkItemStackDescriptor::deserialize(&mut Cursor::new(bytes))
        .expect("empty item descriptor should deserialize")
}

fn sample_item_use_transaction(
    action_type: ItemUseInventoryTransactionType,
    position: BlockPos,
    face: i32,
) -> PackedItemUseLegacyInventoryTransaction<V924> {
    PackedItemUseLegacyInventoryTransaction {
        id: 0,
        container_slots: None,
        action: bedrockrs_proto::v662::types::InventoryTransaction { action: Vec::new() },
        action_type,
        trigger_type: TriggerType::PlayerInput,
        position: NetworkBlockPosition {
            x: position.x,
            y: u32::try_from(position.y).expect("test block y should fit into u32"),
            z: position.z,
        },
        face,
        slot: 0,
        item: empty_item_stack_descriptor(),
        from_position: Vec3::new(0.0, 0.0, 0.0),
        click_position: Vec3::new(0.5, 0.5, 0.5),
        target_block_id: 0,
        predicted_result: PredictedResult::Success,
    }
}

fn encode_legacy_item_use_transaction(
    transaction: PackedItemUseLegacyInventoryTransaction<V924>,
) -> Vec<u8> {
    let mut packet = Vec::new();
    PacketHeader {
        packet_id: 30,
        sender_sub_client_id: 0,
        target_sub_client_id: 0,
    }
    .serialize(&mut packet)
    .expect("packet header should serialize");
    <i32 as ProtoCodecVAR>::serialize(&0, &mut packet).expect("legacy request id should serialize");
    <Vec<LegacySetItemSlotsEntry> as ProtoCodec>::serialize(&Vec::new(), &mut packet)
        .expect("legacy slots should serialize");
    ComplexInventoryTransactionType::ItemUseTransaction
        .serialize(&mut packet)
        .expect("transaction type should serialize");
    transaction
        .serialize(&mut packet)
        .expect("item use transaction should serialize");

    let mut frame = Vec::new();
    <u32 as ProtoCodecVAR>::serialize(
        &u32::try_from(packet.len()).expect("packet length should fit into u32"),
        &mut frame,
    )
    .expect("frame length should serialize");
    frame.extend_from_slice(&packet);
    frame
}

fn encode_player_auth_input(packet: PlayerAuthInputPacket<V924>) -> Vec<u8> {
    let mut body = Vec::new();
    PacketHeader {
        packet_id: 144,
        sender_sub_client_id: 0,
        target_sub_client_id: 0,
    }
    .serialize(&mut body)
    .expect("packet header should serialize");
    packet
        .serialize(&mut body)
        .expect("player auth input should serialize");
    <Vec2<f32> as ProtoCodecLE>::serialize(&packet.raw_move_vector, &mut body)
        .expect("raw move vector should serialize");

    let mut frame = Vec::new();
    <u32 as ProtoCodecVAR>::serialize(
        &u32::try_from(body.len()).expect("packet length should fit into u32"),
        &mut frame,
    )
    .expect("frame length should serialize");
    frame.extend_from_slice(&body);
    frame
}
