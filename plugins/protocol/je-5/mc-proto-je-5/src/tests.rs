use crate::{JE_5_ADAPTER_ID, Je5Adapter, PROTOCOL_VERSION_1_7_10, VERSION_NAME_1_7_10};
use mc_core::{
    BlockPos, BlockState, ChunkColumn, ChunkPos, ConnectionId, CoreCommand, CoreConfig, CoreEvent,
    DroppedItemSnapshot, EntityId, InventoryClickButton, InventoryClickTarget,
    InventoryClickValidation, InventoryContainer, InventorySlot, InventoryTransactionContext,
    InventoryWindowContents, ItemStack, PlayerId, PlayerInventory, PlayerSnapshot, RuntimeCommand,
    ServerCore, SessionCommand, Vec3,
};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeProbe, LoginRequest, PacketReader, PacketWriter,
    PlayEncodingContext, PlaySyncAdapter, ProtocolAdapter, ProtocolDescriptor,
    ProtocolSessionSnapshot, ServerListStatus, SessionAdapter, StatusRequest, TransportKind,
    WireFormatKind,
};
use mc_proto_je_common::__version_support::{
    blocks::legacy_block, chunks::get_nibble, inventory::read_slot,
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

fn decode_session(player_id: PlayerId) -> ProtocolSessionSnapshot {
    ProtocolSessionSnapshot {
        connection_id: ConnectionId(1),
        phase: ConnectionPhase::Play,
        player_id: Some(player_id),
        entity_id: None,
    }
}

fn encode_session(context: &PlayEncodingContext) -> ProtocolSessionSnapshot {
    ProtocolSessionSnapshot {
        connection_id: ConnectionId(1),
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
    let adapter = Je5Adapter::new();

    let handshake = [
        0x00, 0x05, 0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', 0x63, 0xdd, 0x02,
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
    let adapter = Je5Adapter::new();
    assert_eq!(adapter.descriptor().adapter_id, JE_5_ADAPTER_ID);
    assert_eq!(adapter.transport_kind(), TransportKind::Tcp);
    let status_packet = adapter
        .encode_status_response(&ServerListStatus {
            version: ProtocolDescriptor {
                adapter_id: JE_5_ADAPTER_ID.to_string(),
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
fn encodes_spawn_position_as_position_iii() {
    let packet = crate::encoding::encode_spawn_position(mc_core::BlockPos::new(12, 64, -3));
    let mut reader = PacketReader::new(&packet);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x05);
    assert_eq!(reader.read_i32().expect("x should decode"), 12);
    assert_eq!(reader.read_i32().expect("y should decode"), 64);
    assert_eq!(reader.read_i32().expect("z should decode"), -3);
}

#[test]
fn decodes_play_packets_into_core_commands() {
    let adapter = Je5Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"decode-play"));
    let mut writer = PacketWriter::default();
    writer.write_varint(0x04);
    writer.write_f64(42.0);
    writer.write_f64(43.62);
    writer.write_f64(43.0);
    writer.write_f64(10.0);
    writer.write_bool(true);

    let command = adapter
        .decode_play_for(player_id, &writer.into_inner())
        .expect("position should decode")
        .expect("position should produce a command");
    assert!(matches!(
        command,
        RuntimeCommand::Core(CoreCommand::MoveIntent {
            position: Some(_),
            ..
        })
    ));
}

#[test]
fn decodes_inventory_and_edit_packets_into_core_commands() {
    let adapter = Je5Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"decode-edit"));

    let mut held_item = PacketWriter::default();
    held_item.write_varint(0x09);
    held_item.write_i16(4);
    let command = adapter
        .decode_play_for(player_id, &held_item.into_inner())
        .expect("held item change should decode")
        .expect("held item change should produce command");
    assert!(matches!(
        command,
        RuntimeCommand::Core(CoreCommand::SetHeldSlot { slot: 4, .. })
    ));

    let mut settings = PacketWriter::default();
    settings.write_varint(0x15);
    let _ = settings.write_string("ja_JP");
    settings.write_i8(7);
    settings.write_i8(0);
    settings.write_bool(true);
    settings.write_u8(1);
    settings.write_bool(true);
    let command = adapter
        .decode_play_for(player_id, &settings.into_inner())
        .expect("settings should decode")
        .expect("settings should produce command");
    assert!(matches!(
        command,
        RuntimeCommand::Core(CoreCommand::UpdateClientView {
            view_distance: 7,
            ..
        })
    ));

    let mut creative_inventory = PacketWriter::default();
    creative_inventory.write_varint(0x10);
    creative_inventory.write_i16(36);
    creative_inventory.write_i16(20);
    creative_inventory.write_u8(64);
    creative_inventory.write_i16(0);
    creative_inventory.write_i16(-1);
    let command = adapter
        .decode_play_for(player_id, &creative_inventory.into_inner())
        .expect("creative inventory should decode")
        .expect("creative inventory should produce command");
    assert!(matches!(
        command,
        RuntimeCommand::Core(CoreCommand::CreativeInventorySet {
            slot: InventorySlot::Hotbar(0),
            stack: Some(ref stack),
            ..
        })
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
        .decode_play_for(player_id, &placement.into_inner())
        .expect("placement should decode")
        .expect("placement should produce command");
    assert!(matches!(
        command,
        RuntimeCommand::Core(CoreCommand::UseBlock {
            position: mc_core::BlockPos { x: 2, y: 3, z: 0 },
            face: Some(mc_core::BlockFace::Top),
            held_item: Some(ref stack),
            ..
        }) if stack.key.as_str() == "minecraft:stone"
    ));
}

#[test]
fn decodes_window_zero_clicks_and_encodes_cursor_sync() {
    let adapter = Je5Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"window-click-1710"));

    let mut click = PacketWriter::default();
    click.write_varint(0x0e);
    click.write_i8(0);
    click.write_i16(1);
    click.write_i8(0);
    click.write_i16(7);
    click.write_i8(0);
    click.write_i16(17);
    click.write_u8(1);
    click.write_i16(0);
    click.write_i16(-1);
    let command = adapter
        .decode_play_for(player_id, &click.into_inner())
        .expect("click window should decode")
        .expect("click window should produce a command");
    assert!(matches!(
        command,
        RuntimeCommand::Core(CoreCommand::InventoryClick {
            transaction: InventoryTransactionContext {
                window_id: 0,
                action_number: 7,
            },
            target: InventoryClickTarget::Slot(InventorySlot::Auxiliary(1)),
            button: InventoryClickButton::Left,
            validation: InventoryClickValidation::StrictSlotEcho {
                clicked_item: Some(ref stack),
            },
            ..
        }) if stack.key.as_str() == "minecraft:oak_log" && stack.count == 1
    ));

    let mut confirm = PacketWriter::default();
    confirm.write_varint(0x0f);
    confirm.write_u8(0);
    confirm.write_i16(7);
    confirm.write_bool(false);
    let command = adapter
        .decode_play_for(player_id, &confirm.into_inner())
        .expect("confirm transaction should decode")
        .expect("confirm transaction should produce a command");
    assert!(matches!(
        command,
        RuntimeCommand::Session(SessionCommand::InventoryTransactionAck {
            transaction: InventoryTransactionContext {
                window_id: 0,
                action_number: 7,
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
                    action_number: 7,
                },
                accepted: true,
            },
            &PlayEncodingContext {
                player_id,
                entity_id: mc_core::EntityId(1),
            },
        )
        .expect("confirm transaction should encode");
    let mut reader = PacketReader::new(&packet[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x32);
    assert_eq!(reader.read_u8().expect("window id should decode"), 0);
    assert_eq!(reader.read_i16().expect("action number should decode"), 7);
    assert!(reader.read_bool().expect("accepted should decode"));

    let packet = adapter
        .encode_play_event_for(
            &CoreEvent::CursorChanged {
                stack: Some(ItemStack::new("minecraft:oak_log", 1, 0)),
            },
            &PlayEncodingContext {
                player_id,
                entity_id: mc_core::EntityId(1),
            },
        )
        .expect("cursor update should encode");
    let mut reader = PacketReader::new(&packet[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x2f);
    assert_eq!(reader.read_i8().expect("window id should decode"), -1);
    assert_eq!(reader.read_i16().expect("slot should decode"), -1);
}

#[test]
fn chunk_encoding_uses_legacy_block_layout() {
    let mut chunk = ChunkColumn::new(ChunkPos::new(0, 0));
    chunk.set_block(0, 0, 0, BlockState::bedrock());
    chunk.set_block(1, 0, 0, BlockState::stone());
    let (_, data) =
        mc_proto_je_common::__version_support::chunks::build_chunk_data_1_7(&chunk, true);
    assert_eq!(data[0], 7);
    assert_eq!(data[1], 1);
    assert_eq!(get_nibble(&data[4096..6144], 0), 0);
    assert_eq!(legacy_block(&BlockState::grass_block()), (2, 0));
}

#[test]
fn encodes_legacy_slots_with_length_sentinel() {
    let packet = crate::encoding::encode_window_items(
        0,
        InventoryContainer::Player,
        &InventoryWindowContents::player(PlayerInventory::creative_starter()),
    )
    .expect("window items should encode");
    let mut reader = PacketReader::new(&packet);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x30);
    assert_eq!(reader.read_i8().expect("window id should decode"), 0);
    assert_eq!(reader.read_i16().expect("slot count should decode"), 45);
    for _ in 0..36 {
        assert_eq!(reader.read_i16().expect("empty slot should decode"), -1);
    }
    assert!(reader.read_i16().expect("item id should decode") >= 0);
    assert_eq!(reader.read_u8().expect("count should decode"), 64);
    let _ = reader.read_i16().expect("damage should decode");
    assert_eq!(reader.read_i16().expect("nbt sentinel should decode"), -1);
}

#[test]
fn play_bootstrap_and_chunk_batch_emit_join_game_and_chunks() {
    let adapter = Je5Adapter::new();
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
        .encode_play_event_for(&play_bootstrap, &context)
        .expect("play bootstrap should encode");
    let chunk_packets = adapter
        .encode_play_event_for(&chunk_batch, &context)
        .expect("chunk batch should encode");

    assert!(bootstrap_packets.iter().any(|packet| packet[0] == 0x01));
    assert!(chunk_packets.iter().any(|packet| packet[0] == 0x26));
    assert!(bootstrap_packets.iter().any(|packet| packet[0] == 0x39));
}

#[test]
fn encodes_inventory_and_block_events() {
    let adapter = Je5Adapter::new();
    let context = PlayEncodingContext {
        player_id: PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"encode-play-events")),
        entity_id: mc_core::EntityId(1),
    };
    let inventory = PlayerInventory::creative_starter();
    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::InventoryContents {
                window_id: 0,
                container: InventoryContainer::Player,
                contents: InventoryWindowContents::player(inventory),
            },
            &context,
        )
        .expect("inventory snapshot should encode");
    assert_eq!(packets[0][0], 0x30);

    let packets = adapter
        .encode_play_event_for(&CoreEvent::SelectedHotbarSlotChanged { slot: 4 }, &context)
        .expect("held slot change should encode");
    assert_eq!(packets[0][0], 0x09);

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::BlockChanged {
                position: mc_core::BlockPos::new(2, 4, 0),
                block: BlockState::glass(),
            },
            &context,
        )
        .expect("block change should encode");
    assert_eq!(packets[0][0], 0x23);
}

#[test]
fn encodes_and_decodes_container_window_packets() {
    let adapter = Je5Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"window-open-1710"));

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerOpened {
                window_id: 2,
                container: InventoryContainer::CraftingTable,
                title: "Crafting".to_string(),
            },
            &PlayEncodingContext {
                player_id,
                entity_id: mc_core::EntityId(1),
            },
        )
        .expect("open window should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x2d);
    assert_eq!(reader.read_u8().expect("window id should decode"), 2);
    assert_eq!(
        reader.read_string(32).expect("window type should decode"),
        "minecraft:crafting_table"
    );
    assert_eq!(
        reader.read_string(32).expect("title should decode"),
        "Crafting"
    );
    assert_eq!(reader.read_u8().expect("slot count should decode"), 0);
    assert!(reader.read_bool().expect("use title should decode"));

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerClosed { window_id: 2 },
            &PlayEncodingContext {
                player_id,
                entity_id: mc_core::EntityId(1),
            },
        )
        .expect("close window should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x2e);
    assert_eq!(reader.read_u8().expect("window id should decode"), 2);

    let mut close = PacketWriter::default();
    close.write_varint(0x0d);
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
    let adapter = Je5Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"chest-1710"));
    let context = PlayEncodingContext {
        player_id,
        entity_id: mc_core::EntityId(1),
    };

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerOpened {
                window_id: 4,
                container: InventoryContainer::Chest,
                title: "Chest".to_string(),
            },
            &context,
        )
        .expect("chest open should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x2d);
    assert_eq!(reader.read_u8().expect("window id should decode"), 4);
    assert_eq!(
        reader.read_string(32).expect("window type should decode"),
        "minecraft:chest"
    );
    assert_eq!(
        reader.read_string(32).expect("title should decode"),
        "Chest"
    );
    assert_eq!(reader.read_u8().expect("slot count should decode"), 27);
    assert!(reader.read_bool().expect("use title should decode"));

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::InventorySlotChanged {
                window_id: 4,
                container: InventoryContainer::Chest,
                slot: InventorySlot::MainInventory(0),
                stack: Some(ItemStack::new("minecraft:glass", 1, 0)),
            },
            &context,
        )
        .expect("main inventory remap should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x2f);
    assert_eq!(reader.read_i8().expect("window id should decode"), 4);
    assert_eq!(reader.read_i16().expect("slot should decode"), 27);

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::InventorySlotChanged {
                window_id: 4,
                container: InventoryContainer::Chest,
                slot: InventorySlot::Hotbar(0),
                stack: Some(ItemStack::new("minecraft:glass", 1, 0)),
            },
            &context,
        )
        .expect("hotbar remap should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x2f);
    assert_eq!(reader.read_i8().expect("window id should decode"), 4);
    assert_eq!(reader.read_i16().expect("slot should decode"), 54);
}

#[test]
fn furnace_packets_use_expected_window_type_slot_mapping_and_properties() {
    let adapter = Je5Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"furnace-1710"));
    let context = PlayEncodingContext {
        player_id,
        entity_id: mc_core::EntityId(1),
    };

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerOpened {
                window_id: 3,
                container: InventoryContainer::Furnace,
                title: "Furnace".to_string(),
            },
            &context,
        )
        .expect("furnace open should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x2d);
    assert_eq!(reader.read_u8().expect("window id should decode"), 3);
    assert_eq!(
        reader.read_string(32).expect("window type should decode"),
        "minecraft:furnace"
    );
    assert_eq!(
        reader.read_string(32).expect("title should decode"),
        "Furnace"
    );
    assert_eq!(reader.read_u8().expect("slot count should decode"), 3);
    assert!(reader.read_bool().expect("use title should decode"));

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::InventorySlotChanged {
                window_id: 3,
                container: InventoryContainer::Furnace,
                slot: InventorySlot::MainInventory(0),
                stack: Some(ItemStack::new("minecraft:glass", 1, 0)),
            },
            &context,
        )
        .expect("main inventory remap should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x2f);
    assert_eq!(reader.read_i8().expect("window id should decode"), 3);
    assert_eq!(reader.read_i16().expect("slot should decode"), 3);

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::InventorySlotChanged {
                window_id: 3,
                container: InventoryContainer::Furnace,
                slot: InventorySlot::Hotbar(0),
                stack: Some(ItemStack::new("minecraft:glass", 1, 0)),
            },
            &context,
        )
        .expect("hotbar remap should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x2f);
    assert_eq!(reader.read_i8().expect("window id should decode"), 3);
    assert_eq!(reader.read_i16().expect("slot should decode"), 30);

    let packets = adapter
        .encode_play_event_for(
            &CoreEvent::ContainerPropertyChanged {
                window_id: 3,
                property_id: 1,
                value: 300,
            },
            &context,
        )
        .expect("furnace property should encode");
    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x31);
    assert_eq!(reader.read_u8().expect("window id should decode"), 3);
    assert_eq!(reader.read_i16().expect("property id should decode"), 1);
    assert_eq!(
        reader.read_i16().expect("property value should decode"),
        300
    );
}

#[test]
fn encodes_dropped_item_spawn_and_metadata() {
    let adapter = Je5Adapter::new();
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
                player_id: PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"dropped-item-1710")),
                entity_id: EntityId(1),
            },
        )
        .expect("dropped item should encode");
    assert_eq!(packets.len(), 2);

    let mut spawn = PacketReader::new(&packets[0]);
    assert_eq!(spawn.read_varint().expect("packet id should decode"), 0x0e);
    assert_eq!(spawn.read_i32().expect("entity id should decode"), 11);
    assert_eq!(spawn.read_u8().expect("object type should decode"), 2);

    let mut metadata = PacketReader::new(&packets[1]);
    assert_eq!(
        metadata.read_varint().expect("packet id should decode"),
        0x1c
    );
    assert_eq!(metadata.read_i32().expect("entity id should decode"), 11);
    assert_eq!(
        metadata.read_u8().expect("metadata key should decode"),
        (5 << 5) | 10
    );
    assert_eq!(
        read_slot(&mut metadata, crate::INVENTORY_SPEC.slot_nbt)
            .expect("metadata slot should decode"),
        Some(ItemStack::new("minecraft:cobblestone", 1, 0))
    );
    assert_eq!(metadata.read_u8().expect("terminator should decode"), 0x7f);
}

#[test]
fn encodes_block_break_animation_stage_and_clear() {
    let adapter = Je5Adapter::new();
    let context = PlayEncodingContext {
        player_id: PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"break-1710")),
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
    assert_eq!(
        mc_proto_test_support::TestJavaProtocol::Je5
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
        mc_proto_test_support::TestJavaProtocol::Je5
            .decode_block_break_animation(&clear_packet[0])
            .expect("clear packet should decode"),
        (11, 2, 4, 0, -1)
    );
}
