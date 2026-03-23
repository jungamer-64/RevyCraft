use super::{JE_1_12_2_ADAPTER_ID, Je1122Adapter, PROTOCOL_VERSION_1_12_2, VERSION_NAME_1_12_2};
use mc_core::{
    CoreCommand, CoreEvent, DimensionId, InteractionHand, InventoryClickButton,
    InventoryClickTarget, InventoryContainer, InventorySlot, InventoryTransactionContext,
    ItemStack, PlayerId, PlayerInventory, PlayerSnapshot, Vec3,
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
    let adapter = Je1122Adapter::new();

    let handshake = [
        0x00, 0xd4, 0x02, 0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', 0x63, 0xdd,
        0x02,
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
    let adapter = Je1122Adapter::new();
    let status_packet = adapter
        .encode_status_response(&ServerListStatus {
            version: ProtocolDescriptor {
                adapter_id: JE_1_12_2_ADAPTER_ID.to_string(),
                transport: TransportKind::Tcp,
                wire_format: WireFormatKind::MinecraftFramed,
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

    let encryption_request = adapter
        .encode_encryption_request("", &[1, 2, 3], &[4, 5])
        .expect("encryption request should encode");
    assert_eq!(encryption_request[0], 0x01);

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
    writer.write_i64(pack_block_position(mc_core::BlockPos::new(2, 3, 4)));
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

#[test]
fn encodes_player_spawn_with_player_info() {
    let adapter = Je1122Adapter::new();
    let player = player_snapshot("alpha");
    let packets = adapter
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
    assert_eq!(packets.len(), 3);

    let mut reader = PacketReader::new(&packets[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x2d);
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
        0x36
    );
}

#[test]
fn decodes_window_zero_clicks_and_encodes_cursor_sync() {
    let adapter = Je1122Adapter::new();
    let player_id = PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, b"window-click-1122"));
    let mut writer = PacketWriter::default();
    writer.write_varint(0x07);
    writer.write_i8(0);
    writer.write_i16(45);
    writer.write_i8(0);
    writer.write_i16(11);
    writer.write_varint(0);
    writer.write_i16(20);
    writer.write_u8(1);
    writer.write_i16(0);
    writer.write_u8(0);
    let command = adapter
        .decode_play(player_id, &writer.into_inner())
        .expect("click window should decode")
        .expect("click window should produce a command");
    assert!(matches!(
        command,
        CoreCommand::InventoryClick {
            transaction: InventoryTransactionContext {
                window_id: 0,
                action_number: 11,
            },
            target: InventoryClickTarget::Slot(InventorySlot::Offhand),
            button: InventoryClickButton::Left,
            clicked_item: Some(ref stack),
            ..
        } if stack.key.as_str() == "minecraft:glass" && stack.count == 1
    ));

    let mut writer = PacketWriter::default();
    writer.write_varint(0x05);
    writer.write_u8(0);
    writer.write_i16(11);
    writer.write_bool(false);
    let command = adapter
        .decode_play(player_id, &writer.into_inner())
        .expect("confirm transaction should decode")
        .expect("confirm transaction should produce a command");
    assert!(matches!(
        command,
        CoreCommand::InventoryTransactionAck {
            transaction: InventoryTransactionContext {
                window_id: 0,
                action_number: 11,
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
                    action_number: 11,
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
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x11);
    assert_eq!(reader.read_u8().expect("window id should decode"), 0);
    assert_eq!(reader.read_i16().expect("action number should decode"), 11);
    assert!(reader.read_bool().expect("accepted should decode"));

    let packet = adapter
        .encode_play_event(
            &CoreEvent::CursorChanged {
                stack: Some(ItemStack::new("minecraft:stick", 4, 0)),
            },
            &PlayEncodingContext {
                player_id,
                entity_id: mc_core::EntityId(1),
            },
        )
        .expect("cursor update should encode");
    let mut reader = PacketReader::new(&packet[0]);
    assert_eq!(reader.read_varint().expect("packet id should decode"), 0x16);
    assert_eq!(reader.read_i8().expect("window id should decode"), -1);
    assert_eq!(reader.read_i16().expect("slot should decode"), -1);
}

fn pack_block_position(position: mc_core::BlockPos) -> i64 {
    let x = i64::from(position.x) & 0x3ff_ffff;
    let y = i64::from(position.y) & 0xfff;
    let z = i64::from(position.z) & 0x3ff_ffff;
    (x << 38) | (y << 26) | z
}
