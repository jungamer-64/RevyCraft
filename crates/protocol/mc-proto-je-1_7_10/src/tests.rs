use crate::{JE_1_7_10_ADAPTER_ID, Je1710Adapter, PROTOCOL_VERSION_1_7_10, VERSION_NAME_1_7_10};
use mc_core::{
    BlockState, ChunkColumn, ChunkPos, ConnectionId, CoreCommand, CoreConfig, CoreEvent,
    InventoryClickButton, InventoryClickTarget, InventoryContainer, InventorySlot,
    InventoryTransactionContext, ItemStack, PlayerId, PlayerInventory, PlayerSnapshot, ServerCore,
    Vec3,
};
use mc_proto_common::{
    Edition, HandshakeProbe, LoginRequest, PacketReader, PacketWriter, PlayEncodingContext,
    PlaySyncAdapter, ProtocolAdapter, ProtocolDescriptor, ServerListStatus, SessionAdapter,
    StatusRequest, TransportKind, WireFormatKind,
};
use mc_proto_je_common::__version_support::{blocks::legacy_block, chunks::get_nibble};
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
    let adapter = Je1710Adapter::new();
    assert_eq!(adapter.descriptor().adapter_id, JE_1_7_10_ADAPTER_ID);
    assert_eq!(adapter.transport_kind(), TransportKind::Tcp);
    let status_packet = adapter
        .encode_status_response(&ServerListStatus {
            version: ProtocolDescriptor {
                adapter_id: JE_1_7_10_ADAPTER_ID.to_string(),
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
        CoreCommand::CreativeInventorySet {
            slot: InventorySlot::Hotbar(0),
            stack: Some(ref stack),
            ..
        }
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
fn decodes_window_zero_clicks_and_encodes_cursor_sync() {
    let adapter = Je1710Adapter::new();
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
        .decode_play(player_id, &click.into_inner())
        .expect("click window should decode")
        .expect("click window should produce a command");
    assert!(matches!(
        command,
        CoreCommand::InventoryClick {
            transaction: InventoryTransactionContext {
                window_id: 0,
                action_number: 7,
            },
            target: InventoryClickTarget::Slot(InventorySlot::Auxiliary(1)),
            button: InventoryClickButton::Left,
            clicked_item: Some(ref stack),
            ..
        } if stack.key.as_str() == "minecraft:oak_log" && stack.count == 1
    ));

    let mut confirm = PacketWriter::default();
    confirm.write_varint(0x0f);
    confirm.write_u8(0);
    confirm.write_i16(7);
    confirm.write_bool(false);
    let command = adapter
        .decode_play(player_id, &confirm.into_inner())
        .expect("confirm transaction should decode")
        .expect("confirm transaction should produce a command");
    assert!(matches!(
        command,
        CoreCommand::InventoryTransactionAck {
            transaction: InventoryTransactionContext {
                window_id: 0,
                action_number: 7,
            },
            accepted: false,
            ..
        }
    ));

    let packet = adapter
        .encode_play_event(
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
        .encode_play_event(
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
fn play_bootstrap_and_chunk_batch_emit_join_game_and_chunks() {
    let adapter = Je1710Adapter::new();
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
