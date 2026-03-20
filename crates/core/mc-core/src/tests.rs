use super::*;

fn player_id(name: &str) -> PlayerId {
    PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, name.as_bytes()))
}

#[test]
fn chunk_column_stores_semantic_states() {
    let mut column = ChunkColumn::new(ChunkPos::new(0, 0));
    column.set_block(1, 12, 2, BlockState::grass_block());
    assert_eq!(
        column.get_block(1, 12, 2).key.as_str(),
        "minecraft:grass_block"
    );
    assert!(column.get_block(1, 32, 2).is_air());
}

#[test]
fn block_index_helpers_round_trip_section_local_coordinates() {
    for y in 0_u8..16 {
        for z in 0_u8..16 {
            for x in 0_u8..16 {
                let index = flatten_block_index(x, y, z);
                assert_eq!(index.expand(), (x, y, z));
                assert_eq!(expand_block_index(index.into_raw()), (x, y, z));
            }
        }
    }
}

#[test]
fn login_emits_initial_chunks_and_existing_entities() {
    let mut core = ServerCore::new(CoreConfig {
        view_distance: 1,
        ..CoreConfig::default()
    });

    let first = player_id("first");
    let events = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(1),
            username: "first".to_string(),
            player_id: first,
        },
        0,
    );
    assert!(events.iter().any(|event| matches!(
        event.event,
        CoreEvent::PlayBootstrap {
            view_distance: 1,
            ..
        }
    )));
    assert!(events.iter().any(|event| {
        matches!(event.event, CoreEvent::ChunkBatch { ref chunks } if chunks.len() == 9)
    }));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            TargetedEvent {
                target: EventTarget::Connection(ConnectionId(1)),
                event: CoreEvent::InventoryContents {
                    container: InventoryContainer::Player,
                    ..
                },
            }
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            TargetedEvent {
                target: EventTarget::Connection(ConnectionId(1)),
                event: CoreEvent::SelectedHotbarSlotChanged { slot: 0 },
            }
        )
    }));

    let second = player_id("second");
    let events = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(2),
            username: "second".to_string(),
            player_id: second,
        },
        0,
    );
    assert!(events.iter().any(|event| {
        matches!(
            event,
            TargetedEvent {
                target: EventTarget::Connection(ConnectionId(2)),
                event: CoreEvent::EntitySpawned { .. },
            }
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            TargetedEvent {
                target: EventTarget::EveryoneExcept(id),
                event: CoreEvent::EntitySpawned { .. },
            } if *id == second
        )
    }));
}

#[test]
fn canonical_policy_matches_default_apply_command() {
    let config = CoreConfig {
        game_mode: 1,
        ..CoreConfig::default()
    };
    let mut default_core = ServerCore::new(config.clone());
    let mut explicit_core = ServerCore::new(config);
    let player = player_id("policy-parity");
    let command = CoreCommand::LoginStart {
        connection_id: ConnectionId(1),
        username: "policy-parity".to_string(),
        player_id: player,
    };

    let default_events = default_core.apply_command(command.clone(), 0);
    let explicit_events = explicit_core
        .apply_command_with_policy(
            command,
            0,
            Some(&canonical_session_capabilities()),
            &CanonicalGameplayPolicy,
        )
        .expect("canonical gameplay policy should succeed");

    assert_eq!(default_events, explicit_events);
    assert_eq!(default_core.snapshot(), explicit_core.snapshot());
}

#[test]
fn readonly_policy_rejects_block_edit_without_mutation() {
    let mut core = ServerCore::new(CoreConfig {
        game_mode: 1,
        ..CoreConfig::default()
    });
    let player = player_id("readonly");
    let _ = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(1),
            username: "readonly".to_string(),
            player_id: player,
        },
        0,
    );
    let before = core.snapshot();
    let mut readonly_capabilities = canonical_session_capabilities();
    readonly_capabilities.gameplay = CapabilitySet::new();
    let _ = readonly_capabilities
        .gameplay
        .insert("gameplay.profile.readonly");
    readonly_capabilities.gameplay_profile = GameplayProfileId::new("readonly");

    let effect = ReadonlyGameplayPolicy
        .handle_command(
            &core,
            &readonly_capabilities,
            &CoreCommand::PlaceBlock {
                player_id: player,
                position: BlockPos::new(2, 3, 0),
                hand: InteractionHand::Main,
                face: Some(BlockFace::Top),
                held_item: None,
            },
        )
        .expect("readonly gameplay policy should handle place rejection");

    assert!(effect.mutations.is_empty());
    assert!(effect.emitted_events.is_empty());
    assert_eq!(before, core.snapshot());
}

#[test]
fn moving_player_updates_other_clients_and_view() {
    let mut core = ServerCore::new(CoreConfig {
        view_distance: 1,
        ..CoreConfig::default()
    });
    let first = player_id("first");
    let second = player_id("second");
    let _ = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(1),
            username: "first".to_string(),
            player_id: first,
        },
        0,
    );
    let _ = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(2),
            username: "second".to_string(),
            player_id: second,
        },
        0,
    );

    let events = core.apply_command(
        CoreCommand::MoveIntent {
            player_id: second,
            position: Some(Vec3::new(32.5, 4.0, 0.5)),
            yaw: Some(90.0),
            pitch: Some(0.0),
            on_ground: true,
        },
        50,
    );

    assert!(events.iter().any(|event| {
        matches!(
            event,
            TargetedEvent {
                target: EventTarget::EveryoneExcept(id),
                event: CoreEvent::EntityMoved { .. },
            } if *id == second
        )
    }));
    assert!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    TargetedEvent {
                        target: EventTarget::Player(id),
                        event: CoreEvent::ChunkBatch { .. },
                    } if *id == second
                )
            })
            .count()
            >= 3
    );
}

#[test]
fn keepalive_tick_emits_keepalive() {
    let mut core = ServerCore::new(CoreConfig::default());
    let first = player_id("first");
    let _ = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(1),
            username: "first".to_string(),
            player_id: first,
        },
        0,
    );
    let events = core.tick(DEFAULT_KEEPALIVE_INTERVAL_MS + 1);
    assert!(events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::KeepAliveRequested { .. },
        } if *id == first
    )));
}

#[test]
fn world_snapshot_roundtrip_uses_semantic_types() {
    let mut core = ServerCore::new(CoreConfig::default());
    let first = player_id("first");
    let _ = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(1),
            username: "first".to_string(),
            player_id: first,
        },
        0,
    );
    let snapshot = core.snapshot();
    let json = serde_json::to_string(&snapshot).expect("snapshot should serialize");
    let decoded: WorldSnapshot = serde_json::from_str(&json).expect("snapshot should deserialize");
    assert_eq!(decoded.meta.level_type, "FLAT");
    assert!(
        decoded
            .chunks
            .values()
            .next()
            .expect("generated chunk should exist")
            .get_block(0, 3, 0)
            .key
            .as_str()
            == "minecraft:grass_block"
    );
    let player = decoded
        .players
        .get(&first)
        .expect("logged in player should persist");
    assert_eq!(player.selected_hotbar_slot, 0);
    assert_eq!(
        player
            .inventory
            .get(36)
            .expect("starter slot 36 should exist")
            .key
            .as_str(),
        "minecraft:stone"
    );
}

#[test]
fn inventory_commands_update_selected_slot_and_slots() {
    let mut core = ServerCore::new(CoreConfig {
        game_mode: 1,
        ..CoreConfig::default()
    });
    let first = player_id("first");
    let _ = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(1),
            username: "first".to_string(),
            player_id: first,
        },
        0,
    );

    let slot_events = core.apply_command(
        CoreCommand::CreativeInventorySet {
            player_id: first,
            slot: InventorySlot::Hotbar(0),
            stack: Some(ItemStack::new("minecraft:glass", 64, 0)),
        },
        0,
    );
    assert!(slot_events.iter().any(|event| {
        matches!(
            event,
            TargetedEvent {
                target: EventTarget::Player(id),
                event: CoreEvent::InventorySlotChanged {
                    container: InventoryContainer::Player,
                    slot: InventorySlot::Hotbar(0),
                    ..
                },
            } if *id == first
        )
    }));

    let held_events = core.apply_command(
        CoreCommand::SetHeldSlot {
            player_id: first,
            slot: 4,
        },
        0,
    );
    assert!(held_events.iter().any(|event| {
        matches!(
            event,
            TargetedEvent {
                target: EventTarget::Player(id),
                event: CoreEvent::SelectedHotbarSlotChanged { slot: 4 },
            } if *id == first
        )
    }));

    let snapshot = core.snapshot();
    let player = snapshot.players.get(&first).expect("player should persist");
    assert_eq!(player.selected_hotbar_slot, 4);
    assert_eq!(
        player
            .inventory
            .get_slot(InventorySlot::Hotbar(0))
            .expect("slot should be updated")
            .key
            .as_str(),
        "minecraft:glass"
    );
}

#[test]
fn update_client_view_clamps_to_server_distance() {
    let mut core = ServerCore::new(CoreConfig {
        view_distance: 2,
        ..CoreConfig::default()
    });
    let first = player_id("first");
    let _ = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(1),
            username: "first".to_string(),
            player_id: first,
        },
        0,
    );

    let _ = core.apply_command(
        CoreCommand::UpdateClientView {
            player_id: first,
            view_distance: 1,
        },
        0,
    );

    let events = core.apply_command(
        CoreCommand::UpdateClientView {
            player_id: first,
            view_distance: 8,
        },
        0,
    );

    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    TargetedEvent {
                        target: EventTarget::Player(id),
                        event: CoreEvent::ChunkBatch { chunks },
                    } if *id == first && chunks.len() == 1
                )
            })
            .count(),
        16
    );
}

#[test]
fn creative_place_and_break_emit_authoritative_corrections() {
    let mut creative = ServerCore::new(CoreConfig {
        game_mode: 1,
        ..CoreConfig::default()
    });
    let first = player_id("first");
    let second = player_id("second");
    let _ = creative.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(1),
            username: "first".to_string(),
            player_id: first,
        },
        0,
    );
    let _ = creative.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(2),
            username: "second".to_string(),
            player_id: second,
        },
        0,
    );

    let place_events = creative.apply_command(
        CoreCommand::PlaceBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: BlockPos::new(2, 3, 0),
            face: Some(BlockFace::Top),
            held_item: Some(ItemStack::new("minecraft:stone", 64, 0)),
        },
        0,
    );
    assert!(
        place_events
            .iter()
            .filter(|event| matches!(
                event.event,
                CoreEvent::BlockChanged {
                    position: BlockPos { x: 2, y: 4, z: 0 },
                    ..
                }
            ))
            .count()
            >= 2
    );

    let break_events = creative.apply_command(
        CoreCommand::DigBlock {
            player_id: first,
            position: BlockPos::new(2, 4, 0),
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    assert!(
        break_events
            .iter()
            .filter(|event| matches!(
                event.event,
                CoreEvent::BlockChanged {
                    position: BlockPos { x: 2, y: 4, z: 0 },
                    ref block,
                } if block.is_air()
            ))
            .count()
            >= 2
    );
}

#[test]
fn survival_place_rejection_emits_authoritative_corrections() {
    let mut survival = ServerCore::new(CoreConfig::default());
    let lone = player_id("lone");
    let _ = survival.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(3),
            username: "lone".to_string(),
            player_id: lone,
        },
        0,
    );
    let reject_events = survival.apply_command(
        CoreCommand::PlaceBlock {
            player_id: lone,
            hand: InteractionHand::Main,
            position: BlockPos::new(2, 3, 0),
            face: Some(BlockFace::Top),
            held_item: Some(ItemStack::new("minecraft:stone", 64, 0)),
        },
        0,
    );
    assert!(reject_events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::BlockChanged {
                position: BlockPos { x: 2, y: 4, z: 0 },
                block,
            },
        } if *id == lone && block.is_air()
    )));
    assert!(reject_events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::InventorySlotChanged {
                container: InventoryContainer::Player,
                slot: InventorySlot::Hotbar(0),
                ..
            },
        } if *id == lone
    )));
}
