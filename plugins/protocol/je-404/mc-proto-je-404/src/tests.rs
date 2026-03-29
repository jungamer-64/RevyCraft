use super::{JE_404_ADAPTER_ID, Je404Adapter, PROTOCOL_VERSION_1_13_2, VERSION_NAME_1_13_2};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeProbe, LoginRequest, PacketReader, PacketWriter,
    PlayEncodingContext, PlaySyncAdapter, ProtocolDescriptor, ProtocolSessionSnapshot,
    ServerListStatus, SessionAdapter, StatusRequest, TransportKind, WireFormatKind,
};
use mc_proto_je_common::__version_support::{
    blocks::flattened_block_state_id_1_13_2, inventory::read_slot,
};
use revy_voxel_core::{
    CoreCommand, CoreEvent, EntityId, PlayerId, PlayerSnapshot, RuntimeCommand, SessionCommand,
};
use revy_voxel_model::{
    BlockPos, DimensionId, DroppedItemSnapshot, InteractionHand, InventoryClickButton,
    InventoryClickTarget, InventoryClickValidation, InventorySlot, InventoryTransactionContext,
    InventoryWindowContents, ItemStack, PlayerInventory, Vec3,
};
use revy_voxel_rules::{ContainerKindId, ContainerPropertyKey};
use uuid::Uuid;

fn player_container() -> ContainerKindId {
    ContainerKindId::new("canonical:player")
}

fn crafting_table_container() -> ContainerKindId {
    ContainerKindId::new("canonical:crafting_table")
}

fn chest_container() -> ContainerKindId {
    ContainerKindId::new("canonical:chest_27")
}

fn furnace_container() -> ContainerKindId {
    ContainerKindId::new("canonical:furnace")
}

fn furnace_property(id: u8) -> ContainerPropertyKey {
    match id {
        0 => ContainerPropertyKey::new("canonical:furnace.burn_left"),
        1 => ContainerPropertyKey::new("canonical:furnace.burn_max"),
        2 => ContainerPropertyKey::new("canonical:furnace.cook_progress"),
        3 => ContainerPropertyKey::new("canonical:furnace.cook_total"),
        _ => panic!("unsupported furnace property id {id}"),
    }
}

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
        inventory: mc_content_canonical::creative_starter_inventory(),
        selected_hotbar_slot: 0,
    }
}

fn decode_session(player_id: PlayerId) -> ProtocolSessionSnapshot {
    ProtocolSessionSnapshot {
        connection_id: revy_voxel_core::ConnectionId(1),
        phase: ConnectionPhase::Play,
        player_id: Some(player_id),
        entity_id: None,
    }
}

fn encode_session(context: &PlayEncodingContext) -> ProtocolSessionSnapshot {
    ProtocolSessionSnapshot {
        connection_id: revy_voxel_core::ConnectionId(1),
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
fn decodes_handshake_status_and_login_packets() {
    let adapter = Je404Adapter::new();

    let handshake = [
        0x00, 0x94, 0x03, 0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', 0x63, 0xdd,
        0x02,
    ];
    let intent = adapter
        .try_route(&handshake)
        .expect("handshake should decode")
        .expect("handshake should match JE");
    assert_eq!(intent.protocol_number, PROTOCOL_VERSION_1_13_2);
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
fn encodes_status_and_offhand_inventory_events() {
    let adapter = Je404Adapter::new();
    let status_packet = adapter
        .encode_status_response(&ServerListStatus {
            version: ProtocolDescriptor {
                adapter_id: JE_404_ADAPTER_ID.to_string(),
                transport: TransportKind::Tcp,
                wire_format: WireFormatKind::MinecraftFramed,
                edition: Edition::Je,
                version_name: VERSION_NAME_1_13_2.to_string(),
                protocol_number: PROTOCOL_VERSION_1_13_2,
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

    let player = player_snapshot("alpha");
    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::InventorySlotChanged {
                window_id: 0,
                container: player_container(),
                slot: InventorySlot::Offhand,
                stack: Some(ItemStack::new("minecraft:glass", 1, 0)),
            },
            &PlayEncodingContext {
                player_id: player.id,
                entity_id: revy_voxel_core::EntityId(1),
            },
        )
        .expect("offhand update should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x17);
    assert_eq!(reader.read_i8().expect("window id should decode"), 0);
    assert_eq!(reader.read_i16().expect("slot should decode"), 45);
}

#[test]
fn decodes_offhand_block_place() {
    let adapter = Je404Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"offhand-1122"));
    let mut writer = PacketWriter::default();
    writer.write_varint(0x29);
    writer.write_i64(pack_block_position(BlockPos::new(2, 3, 4)));
    writer.write_varint(1);
    writer.write_varint(1);
    writer.write_f32(0.5);
    writer.write_f32(0.5);
    writer.write_f32(0.5);
    writer.write_bool(false);
    let command = adapter
        .decode_play_for(player_id, &writer.into_inner())
        .expect("block place should decode")
        .expect("block place should produce a command");
    assert!(matches!(
        command,
        RuntimeCommand::Core(CoreCommand::UseBlock {
            hand: InteractionHand::Offhand,
            ..
        })
    ));
}

#[test]
fn encodes_player_spawn_with_player_info() {
    let adapter = Je404Adapter::new();
    let player = player_snapshot("alpha");
    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::EntitySpawned {
                entity_id: revy_voxel_core::EntityId(7),
                player: player.clone(),
            },
            &PlayEncodingContext {
                player_id: player.id,
                entity_id: revy_voxel_core::EntityId(7),
            },
        )
        .expect("spawn should encode");
    assert_eq!(packets.len(), 3);

    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x30);
    assert_eq!(reader.read_varint().expect("action should decode"), 0);
    assert_eq!(reader.read_varint().expect("count should decode"), 1);
    assert_eq!(
        reader.read_bytes(16).expect("uuid should decode"),
        player.id.0.as_bytes()
    );
    assert_eq!(
        reader.read_string(16).expect("username should decode"),
        player.username
    );
    assert_eq!(reader.read_varint().expect("properties should decode"), 0);
    assert_eq!(reader.read_varint().expect("gamemode should decode"), 0);
    assert_eq!(reader.read_varint().expect("ping should decode"), 0);
    assert!(!reader.read_bool().expect("display name flag should decode"));
    assert!(reader.is_exhausted());

    let mut spawn_reader = PacketReader::new(&packets[1]);
    assert_eq!(
        spawn_reader.read_varint().expect("spawn id should decode"),
        0x05
    );

    let mut head_reader = PacketReader::new(&packets[2]);
    assert_eq!(
        head_reader
            .read_varint()
            .expect("head rotation id should decode"),
        0x39
    );
}

#[test]
fn decodes_window_zero_clicks_and_encodes_cursor_sync() {
    let adapter = Je404Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"window-click-1122"));
    let mut writer = PacketWriter::default();
    writer.write_varint(0x08);
    writer.write_u8(0);
    writer.write_i16(45);
    writer.write_i8(0);
    writer.write_i16(11);
    writer.write_varint(0);
    writer.write_bool(true);
    writer.write_varint(64);
    writer.write_u8(1);
    writer.write_u8(0);
    let command = adapter
        .decode_play_for(player_id, &writer.into_inner())
        .expect("click window should decode")
        .expect("click window should produce a command");
    assert!(matches!(
        command,
        RuntimeCommand::Core(CoreCommand::InventoryClick {
            transaction: InventoryTransactionContext {
                window_id: 0,
                action_number: 11,
            },
            target: InventoryClickTarget::Slot(InventorySlot::Offhand),
            button: InventoryClickButton::Left,
            validation: InventoryClickValidation::StrictSlotEcho {
                clicked_item: Some(ref stack),
            },
            ..
        }) if stack.key.as_str() == "minecraft:glass" && stack.count == 1
    ));

    let mut writer = PacketWriter::default();
    writer.write_varint(0x06);
    writer.write_u8(0);
    writer.write_i16(11);
    writer.write_bool(false);
    let command = adapter
        .decode_play_for(player_id, &writer.into_inner())
        .expect("confirm transaction should decode")
        .expect("confirm transaction should produce a command");
    assert!(matches!(
        command,
        RuntimeCommand::Session(SessionCommand::InventoryTransactionAck {
            transaction: InventoryTransactionContext {
                window_id: 0,
                action_number: 11,
            },
            accepted: false,
            ..
        })
    ));

    let packet = adapter
        .encode_play_event_for(
            &CoreEvent::InventoryTransactionProcessed {
                transaction: InventoryTransactionContext {
                    window_id: 0,
                    action_number: 11,
                },
                accepted: true,
            },
            &PlayEncodingContext {
                player_id,
                entity_id: revy_voxel_core::EntityId(1),
            },
        )
        .expect("confirm transaction should encode");
    let mut reader = PacketReader::new(&packet[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x12);
    assert_eq!(reader.read_u8().expect("window id should decode"), 0);
    assert_eq!(reader.read_i16().expect("action number should decode"), 11);
    assert!(reader.read_bool().expect("accepted should decode"));

    let packet = adapter
        .encode_play_event_for(
            &CoreEvent::CursorChanged {
                stack: Some(ItemStack::new("minecraft:stick", 4, 0)),
            },
            &PlayEncodingContext {
                player_id,
                entity_id: revy_voxel_core::EntityId(1),
            },
        )
        .expect("cursor update should encode");
    let mut reader = PacketReader::new(&packet[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x17);
    assert_eq!(reader.read_i8().expect("window id should decode"), -1);
    assert_eq!(reader.read_i16().expect("slot should decode"), -1);
}

fn pack_block_position(position: BlockPos) -> i64 {
    let x = i64::from(position.x) & 0x3ff_ffff;
    let y = i64::from(position.y) & 0xfff;
    let z = i64::from(position.z) & 0x3ff_ffff;
    (x << 38) | (y << 26) | z
}

#[test]
fn encodes_block_change_packets() {
    let adapter = Je404Adapter::new();
    let packet = adapter
        .encode_play_event_for(
            &CoreEvent::BlockChanged {
                position: BlockPos::new(2, 3, 4),
                block: Some(revy_voxel_model::BlockState::new("minecraft:glass")),
            },
            &PlayEncodingContext {
                player_id: PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"block-change-1122")),
                entity_id: revy_voxel_core::EntityId(1),
            },
        )
        .expect("block change should encode");

    let mut reader = PacketReader::new(&packet[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x0b);
    assert_eq!(
        reader.read_i64().expect("position should decode"),
        pack_block_position(BlockPos::new(2, 3, 4))
    );
    assert_eq!(
        reader.read_varint().expect("state id should decode"),
        flattened_block_state_id_1_13_2(&revy_voxel_model::BlockState::new("minecraft:glass"))
    );
}

#[test]
fn encodes_and_decodes_container_window_packets() {
    let adapter = Je404Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"window-open-1122"));

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerOpened {
                window_id: 2,
                container: crafting_table_container(),
                title: "Crafting".to_string(),
            },
            &PlayEncodingContext {
                player_id,
                entity_id: revy_voxel_core::EntityId(1),
            },
        )
        .expect("open window should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x14);
    assert_eq!(reader.read_u8().expect("window id should decode"), 2);
    assert_eq!(
        reader.read_string(32).expect("window type should decode"),
        "minecraft:crafting_table"
    );
    assert_eq!(
        reader.read_string(128).expect("title should decode"),
        "{\"text\":\"Crafting\"}"
    );
    assert_eq!(reader.read_u8().expect("slot count should decode"), 0);

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerClosed { window_id: 2 },
            &PlayEncodingContext {
                player_id,
                entity_id: revy_voxel_core::EntityId(1),
            },
        )
        .expect("close window should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x13);
    assert_eq!(reader.read_u8().expect("window id should decode"), 2);

    let mut close = PacketWriter::default();
    close.write_varint(0x09);
    close.write_u8(2);
    let command = adapter
        .decode_play_for(player_id, &close.into_inner())
        .expect("close window should decode")
        .expect("close window should produce command");
    assert_eq!(
        command,
        RuntimeCommand::Core(CoreCommand::CloseContainer {
            player_id,
            window_id: 2,
        })
    );
}

#[test]
fn chest_packets_use_expected_window_type_and_slot_mapping() {
    let adapter = Je404Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"chest-1122"));
    let context = PlayEncodingContext {
        player_id,
        entity_id: revy_voxel_core::EntityId(1),
    };

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerOpened {
                window_id: 4,
                container: chest_container(),
                title: "Chest".to_string(),
            },
            &context,
        )
        .expect("chest window should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x14);
    assert_eq!(reader.read_u8().expect("window id should decode"), 4);
    assert_eq!(
        reader.read_string(32).expect("window type should decode"),
        "minecraft:chest"
    );
    assert_eq!(
        reader.read_string(128).expect("title should decode"),
        "{\"text\":\"Chest\"}"
    );
    assert_eq!(reader.read_u8().expect("slot count should decode"), 27);

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::InventorySlotChanged {
                window_id: 4,
                container: chest_container(),
                slot: InventorySlot::MainInventory(0),
                stack: Some(ItemStack::new("minecraft:glass", 1, 0)),
            },
            &context,
        )
        .expect("main inventory remap should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x17);
    assert_eq!(reader.read_i8().expect("window id should decode"), 4);
    assert_eq!(reader.read_i16().expect("slot should decode"), 27);

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::InventorySlotChanged {
                window_id: 4,
                container: chest_container(),
                slot: InventorySlot::Hotbar(0),
                stack: Some(ItemStack::new("minecraft:glass", 1, 0)),
            },
            &context,
        )
        .expect("hotbar remap should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x17);
    assert_eq!(reader.read_i8().expect("window id should decode"), 4);
    assert_eq!(reader.read_i16().expect("slot should decode"), 54);
}

#[test]
fn furnace_packets_use_expected_window_type_slot_mapping_and_properties() {
    let adapter = Je404Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"furnace-1122"));
    let context = PlayEncodingContext {
        player_id,
        entity_id: revy_voxel_core::EntityId(1),
    };

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerOpened {
                window_id: 3,
                container: furnace_container(),
                title: "Furnace".to_string(),
            },
            &context,
        )
        .expect("furnace window should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x14);
    assert_eq!(reader.read_u8().expect("window id should decode"), 3);
    assert_eq!(
        reader.read_string(32).expect("window type should decode"),
        "minecraft:furnace"
    );
    assert_eq!(
        reader.read_string(128).expect("title should decode"),
        "{\"text\":\"Furnace\"}"
    );
    assert_eq!(reader.read_u8().expect("slot count should decode"), 3);

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::InventoryContents {
                window_id: 3,
                container: furnace_container(),
                contents: InventoryWindowContents::with_container(
                    PlayerInventory::new_empty(),
                    vec![
                        Some(ItemStack::new("minecraft:sand", 1, 0)),
                        Some(ItemStack::new("minecraft:oak_planks", 1, 0)),
                        Some(ItemStack::new("minecraft:glass", 1, 0)),
                    ],
                ),
            },
            &context,
        )
        .expect("furnace contents should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x15);
    assert_eq!(reader.read_u8().expect("window id should decode"), 3);
    assert_eq!(reader.read_i16().expect("slot count should decode"), 39);

    for expected_slot in [
        InventorySlot::WindowLocal(2),
        InventorySlot::MainInventory(0),
        InventorySlot::Hotbar(0),
    ] {
        let packets = adapter
            .encode_play_event_for(
                &CoreEvent::InventorySlotChanged {
                    window_id: 3,
                    container: furnace_container(),
                    slot: expected_slot,
                    stack: Some(ItemStack::new("minecraft:glass", 1, 0)),
                },
                &context,
            )
            .expect("furnace slot update should encode");
        let mut reader = PacketReader::new(&packets[0]);
        assert_eq!(reader.read_varint().expect("packet id should decode"), 0x17);
        assert_eq!(reader.read_i8().expect("window id should decode"), 3);
        let raw_slot = reader.read_i16().expect("slot should decode");
        let expected_raw_slot = match expected_slot {
            InventorySlot::WindowLocal(2) => 2,
            InventorySlot::MainInventory(0) => 3,
            InventorySlot::Hotbar(0) => 30,
            _ => unreachable!(),
        };
        assert_eq!(raw_slot, expected_raw_slot);
    }

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerPropertyChanged {
                window_id: 3,
                property: furnace_property(2),
                value: 123,
            },
            &context,
        )
        .expect("furnace property update should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x16);
    assert_eq!(reader.read_u8().expect("window id should decode"), 3);
    assert_eq!(reader.read_i16().expect("property id should decode"), 2);
    assert_eq!(
        reader.read_i16().expect("property value should decode"),
        123
    );
}

#[test]
fn encodes_dropped_item_spawn_and_metadata() {
    let adapter = Je404Adapter::new();
    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::DroppedItemSpawned {
                entity_id: EntityId(11),
                item: DroppedItemSnapshot {
                    item: ItemStack::new("minecraft:cobblestone", 1, 0),
                    position: Vec3::new(1.5, 4.5, 0.5),
                    velocity: Vec3::new(0.0, 0.0, 0.0),
                },
            },
            &PlayEncodingContext {
                player_id: player_snapshot("drop-1122").id,
                entity_id: EntityId(1),
            },
        )
        .expect("dropped item should encode");
    assert_eq!(packets.len(), 2);

    let mut spawn = PacketReader::new(&packets[0]);
    assert_eq!(spawn.read_varint().expect("packet id should decode"), 0x00);
    assert_eq!(spawn.read_varint().expect("entity id should decode"), 11);
    assert_eq!(spawn.read_bytes(16).expect("uuid should decode").len(), 16);
    assert_eq!(spawn.read_u8().expect("object type should decode"), 2);

    let mut metadata = PacketReader::new(&packets[1]);
    assert_eq!(
        metadata.read_varint().expect("packet id should decode"),
        0x3f
    );
    assert_eq!(metadata.read_varint().expect("entity id should decode"), 11);
    assert_eq!(metadata.read_u8().expect("metadata index should decode"), 6);
    assert_eq!(metadata.read_u8().expect("metadata type should decode"), 6);
    assert_eq!(
        read_slot(&mut metadata, crate::INVENTORY_SPEC.slot).expect("metadata slot should decode"),
        Some(ItemStack::new("minecraft:cobblestone", 1, 0))
    );
    assert_eq!(metadata.read_u8().expect("terminator should decode"), 0xff);
}

#[test]
fn encodes_block_break_animation_stage_and_clear() {
    let adapter = Je404Adapter::new();
    let context = PlayEncodingContext {
        player_id: player_snapshot("break-1122").id,
        entity_id: EntityId(1),
    };

    let stage_packet = adapter
        .encode_play_event_for(
            &CoreEvent::BlockBreakingProgress {
                breaker_entity_id: EntityId(11),
                position: BlockPos::new(2, 4, 0),
                stage: Some(4),
                duration_ms: 750,
            },
            &context,
        )
        .expect("break stage should encode");
    let mut reader = PacketReader::new(&stage_packet[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x08);
    assert_eq!(reader.read_varint().expect("entity id should decode"), 11);
    assert_eq!(
        mc_proto_test_support::TestJavaProtocol::Je404
            .decode_block_break_animation(&stage_packet[0])
            .expect("animation packet should decode"),
        (11, 2, 4, 0, 4)
    );

    let clear_packet = adapter
        .encode_play_event_for(
            &CoreEvent::BlockBreakingProgress {
                breaker_entity_id: EntityId(11),
                position: BlockPos::new(2, 4, 0),
                stage: None,
                duration_ms: 750,
            },
            &context,
        )
        .expect("break clear should encode");
    assert_eq!(
        mc_proto_test_support::TestJavaProtocol::Je404
            .decode_block_break_animation(&clear_packet[0])
            .expect("clear packet should decode"),
        (11, 2, 4, 0, -1)
    );
}
