use super::*;

fn player_id(name: &str) -> PlayerId {
    PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, name.as_bytes()))
}

fn click_slot(
    core: &mut ServerCore,
    player_id: PlayerId,
    window_id: u8,
    action_number: i16,
    slot: InventorySlot,
    button: InventoryClickButton,
    clicked_item: Option<ItemStack>,
) -> Vec<TargetedEvent> {
    core.apply_command(
        CoreCommand::InventoryClick {
            player_id,
            transaction: InventoryTransactionContext {
                window_id,
                action_number,
            },
            target: InventoryClickTarget::Slot(slot),
            button,
            clicked_item,
        },
        0,
    )
}

fn logged_in_creative_core(name: &str) -> (ServerCore, PlayerId) {
    let mut core = ServerCore::new(CoreConfig {
        game_mode: 1,
        ..CoreConfig::default()
    });
    let player = player_id(name);
    let _ = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(1),
            username: name.to_string(),
            player_id: player,
        },
        0,
    );
    (core, player)
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

#[test]
fn window_zero_clicks_move_items_between_storage_and_crafting_slots() {
    let (mut core, player) = logged_in_creative_core("window-zero-move");
    let _ = core.apply_command(
        CoreCommand::CreativeInventorySet {
            player_id: player,
            slot: InventorySlot::Hotbar(0),
            stack: Some(ItemStack::new("minecraft:oak_log", 4, 0)),
        },
        0,
    );

    let pickup_events = click_slot(
        &mut core,
        player,
        0,
        1,
        InventorySlot::Hotbar(0),
        InventoryClickButton::Left,
        None,
    );
    assert!(pickup_events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::InventoryTransactionProcessed {
                transaction,
                accepted: true,
            },
        } if *id == player
            && *transaction == InventoryTransactionContext {
                window_id: 0,
                action_number: 1,
            }
    )));
    assert!(pickup_events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::CursorChanged {
                stack: Some(stack),
            },
        } if *id == player && stack.key.as_str() == "minecraft:oak_log" && stack.count == 4
    )));

    let place_events = click_slot(
        &mut core,
        player,
        0,
        2,
        InventorySlot::crafting_input(0).expect("craft input should exist"),
        InventoryClickButton::Right,
        Some(ItemStack::new("minecraft:oak_log", 1, 0)),
    );
    assert!(place_events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::InventoryTransactionProcessed {
                transaction,
                accepted: true,
            },
        } if *id == player
            && *transaction == InventoryTransactionContext {
                window_id: 0,
                action_number: 2,
            }
    )));
    assert!(place_events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::InventorySlotChanged {
                slot,
                stack: Some(stack),
                ..
            },
        } if *id == player
            && *slot == InventorySlot::crafting_input(0).expect("craft input should exist")
            && stack.key.as_str() == "minecraft:oak_log"
            && stack.count == 1
    )));

    let online = core
        .online_players
        .get(&player)
        .expect("player should still be online");
    assert_eq!(
        online
            .snapshot
            .inventory
            .get_slot(InventorySlot::crafting_input(0).expect("craft input should exist"))
            .as_ref()
            .map(|stack| (stack.key.as_str(), stack.count)),
        Some(("minecraft:oak_log", 1))
    );
    assert_eq!(
        online
            .snapshot
            .inventory
            .crafting_result()
            .as_ref()
            .map(|stack| (stack.key.as_str(), stack.count)),
        Some(("minecraft:oak_planks", 4))
    );
    assert_eq!(
        online
            .cursor
            .as_ref()
            .map(|stack| (stack.key.as_str(), stack.count)),
        Some(("minecraft:oak_log", 3))
    );

    let _ = click_slot(
        &mut core,
        player,
        0,
        3,
        InventorySlot::Offhand,
        InventoryClickButton::Left,
        Some(ItemStack::new("minecraft:oak_log", 3, 0)),
    );
    let online = core
        .online_players
        .get(&player)
        .expect("player should still be online");
    assert_eq!(
        online
            .snapshot
            .inventory
            .offhand
            .as_ref()
            .map(|stack| (stack.key.as_str(), stack.count)),
        Some(("minecraft:oak_log", 3))
    );
    assert!(online.cursor.is_none());
}

#[test]
fn window_zero_recipe_preview_updates_with_inputs() {
    let (mut core, player) = logged_in_creative_core("recipe-preview");
    let _ = core.apply_command(
        CoreCommand::CreativeInventorySet {
            player_id: player,
            slot: InventorySlot::Hotbar(0),
            stack: Some(ItemStack::new("minecraft:sand", 4, 0)),
        },
        0,
    );

    let _ = click_slot(
        &mut core,
        player,
        0,
        1,
        InventorySlot::Hotbar(0),
        InventoryClickButton::Left,
        None,
    );
    for index in 0_u8..4 {
        let _ = click_slot(
            &mut core,
            player,
            0,
            i16::from(index) + 2,
            InventorySlot::crafting_input(index).expect("craft input should exist"),
            InventoryClickButton::Right,
            Some(ItemStack::new("minecraft:sand", 1, 0)),
        );
    }

    let online = core
        .online_players
        .get(&player)
        .expect("player should still be online");
    assert_eq!(
        online
            .snapshot
            .inventory
            .crafting_result()
            .as_ref()
            .map(|stack| (stack.key.as_str(), stack.count)),
        Some(("minecraft:sandstone", 1))
    );
}

#[test]
fn taking_crafting_result_consumes_inputs_and_recomputes_output() {
    let (mut core, player) = logged_in_creative_core("take-result");
    let _ = core.apply_command(
        CoreCommand::CreativeInventorySet {
            player_id: player,
            slot: InventorySlot::Hotbar(0),
            stack: Some(ItemStack::new("minecraft:oak_planks", 2, 0)),
        },
        0,
    );

    let _ = click_slot(
        &mut core,
        player,
        0,
        1,
        InventorySlot::Hotbar(0),
        InventoryClickButton::Left,
        None,
    );
    let _ = click_slot(
        &mut core,
        player,
        0,
        2,
        InventorySlot::crafting_input(0).expect("craft input should exist"),
        InventoryClickButton::Right,
        Some(ItemStack::new("minecraft:oak_planks", 1, 0)),
    );
    let _ = click_slot(
        &mut core,
        player,
        0,
        3,
        InventorySlot::crafting_input(2).expect("craft input should exist"),
        InventoryClickButton::Right,
        Some(ItemStack::new("minecraft:oak_planks", 1, 0)),
    );

    let result_events = click_slot(
        &mut core,
        player,
        0,
        4,
        InventorySlot::crafting_result(),
        InventoryClickButton::Left,
        None,
    );
    assert!(result_events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::InventoryTransactionProcessed {
                transaction,
                accepted: true,
            },
        } if *id == player
            && *transaction == InventoryTransactionContext {
                window_id: 0,
                action_number: 4,
            }
    )));
    assert!(result_events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::CursorChanged {
                stack: Some(stack),
            },
        } if *id == player && stack.key.as_str() == "minecraft:stick" && stack.count == 4
    )));

    let online = core
        .online_players
        .get(&player)
        .expect("player should still be online");
    assert!(online.snapshot.inventory.crafting_result().is_none());
    for index in 0_u8..4 {
        assert!(
            online.snapshot.inventory.crafting_input(index).is_none(),
            "craft input {index} should be consumed"
        );
    }
}

#[test]
fn disconnect_folds_window_zero_state_back_into_persistent_inventory() {
    let (mut core, player) = logged_in_creative_core("disconnect-fold");
    let _ = core.apply_command(
        CoreCommand::CreativeInventorySet {
            player_id: player,
            slot: InventorySlot::Hotbar(0),
            stack: Some(ItemStack::new("minecraft:oak_log", 1, 0)),
        },
        0,
    );

    let _ = click_slot(
        &mut core,
        player,
        0,
        1,
        InventorySlot::Hotbar(0),
        InventoryClickButton::Left,
        None,
    );
    let _ = click_slot(
        &mut core,
        player,
        0,
        2,
        InventorySlot::crafting_input(0).expect("craft input should exist"),
        InventoryClickButton::Left,
        Some(ItemStack::new("minecraft:oak_log", 1, 0)),
    );
    let _ = click_slot(
        &mut core,
        player,
        0,
        3,
        InventorySlot::crafting_result(),
        InventoryClickButton::Left,
        None,
    );

    let _ = core.apply_command(CoreCommand::Disconnect { player_id: player }, 0);
    let snapshot = core.snapshot();
    let persisted = snapshot
        .players
        .get(&player)
        .expect("player should persist after disconnect");
    assert!(persisted.inventory.crafting_result().is_none());
    for index in 0_u8..4 {
        assert!(
            persisted.inventory.crafting_input(index).is_none(),
            "craft input {index} should be cleared before persistence"
        );
    }
    assert!(
        persisted
            .inventory
            .slots
            .iter()
            .flatten()
            .any(|stack| stack.key.as_str() == "minecraft:oak_planks"),
        "crafted planks should be merged back into persistent inventory"
    );
}

#[test]
fn matching_clicked_item_accepts_window_zero_click() {
    let (mut core, player) = logged_in_creative_core("accept-click");
    let _ = core.apply_command(
        CoreCommand::CreativeInventorySet {
            player_id: player,
            slot: InventorySlot::Hotbar(0),
            stack: Some(ItemStack::new("minecraft:oak_log", 1, 0)),
        },
        0,
    );

    let events = click_slot(
        &mut core,
        player,
        0,
        7,
        InventorySlot::Hotbar(0),
        InventoryClickButton::Left,
        None,
    );

    assert!(events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::InventoryTransactionProcessed {
                transaction,
                accepted: true,
            },
        } if *id == player
            && *transaction == InventoryTransactionContext {
                window_id: 0,
                action_number: 7,
            }
    )));
}

#[test]
fn clicked_item_mismatch_rejects_but_keeps_authoritative_mutation() {
    let (mut core, player) = logged_in_creative_core("reject-click");
    let _ = core.apply_command(
        CoreCommand::CreativeInventorySet {
            player_id: player,
            slot: InventorySlot::Hotbar(0),
            stack: Some(ItemStack::new("minecraft:oak_log", 1, 0)),
        },
        0,
    );

    let events = click_slot(
        &mut core,
        player,
        0,
        8,
        InventorySlot::Hotbar(0),
        InventoryClickButton::Left,
        Some(ItemStack::new("minecraft:oak_log", 1, 0)),
    );

    assert!(events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::InventoryTransactionProcessed {
                transaction,
                accepted: false,
            },
        } if *id == player
            && *transaction == InventoryTransactionContext {
                window_id: 0,
                action_number: 8,
            }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::InventoryContents {
                container: InventoryContainer::Player,
                ..
            },
        } if *id == player
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::CursorChanged {
                stack: Some(stack),
            },
        } if *id == player && stack.key.as_str() == "minecraft:oak_log" && stack.count == 1
    )));

    let online = core
        .online_players
        .get(&player)
        .expect("player should still be online");
    assert!(
        online
            .snapshot
            .inventory
            .get_slot(InventorySlot::Hotbar(0))
            .is_none()
    );
    assert_eq!(
        online
            .cursor
            .as_ref()
            .map(|stack| (stack.key.as_str(), stack.count)),
        Some(("minecraft:oak_log", 1))
    );
}

#[test]
fn non_zero_window_click_rejects_without_mutation_or_resync() {
    let (mut core, player) = logged_in_creative_core("reject-window-id");
    let _ = core.apply_command(
        CoreCommand::CreativeInventorySet {
            player_id: player,
            slot: InventorySlot::Hotbar(0),
            stack: Some(ItemStack::new("minecraft:oak_log", 1, 0)),
        },
        0,
    );
    let before = core
        .online_players
        .get(&player)
        .expect("player should still be online")
        .snapshot
        .clone();

    let events = click_slot(
        &mut core,
        player,
        2,
        9,
        InventorySlot::Hotbar(0),
        InventoryClickButton::Left,
        None,
    );

    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::InventoryTransactionProcessed {
                transaction,
                accepted: false,
            },
        } if *id == player
            && *transaction == InventoryTransactionContext {
                window_id: 2,
                action_number: 9,
            }
    ));
    let after = &core
        .online_players
        .get(&player)
        .expect("player should still be online")
        .snapshot;
    assert_eq!(after.inventory, before.inventory);
}

#[test]
fn rejected_crafting_result_click_still_consumes_inputs_authoritatively() {
    let (mut core, player) = logged_in_creative_core("reject-result");
    let _ = core.apply_command(
        CoreCommand::CreativeInventorySet {
            player_id: player,
            slot: InventorySlot::Hotbar(0),
            stack: Some(ItemStack::new("minecraft:oak_planks", 2, 0)),
        },
        0,
    );

    let _ = click_slot(
        &mut core,
        player,
        0,
        1,
        InventorySlot::Hotbar(0),
        InventoryClickButton::Left,
        None,
    );
    let _ = click_slot(
        &mut core,
        player,
        0,
        2,
        InventorySlot::crafting_input(0).expect("craft input should exist"),
        InventoryClickButton::Right,
        Some(ItemStack::new("minecraft:oak_planks", 1, 0)),
    );
    let _ = click_slot(
        &mut core,
        player,
        0,
        3,
        InventorySlot::crafting_input(2).expect("craft input should exist"),
        InventoryClickButton::Right,
        Some(ItemStack::new("minecraft:oak_planks", 1, 0)),
    );

    let events = click_slot(
        &mut core,
        player,
        0,
        4,
        InventorySlot::crafting_result(),
        InventoryClickButton::Left,
        Some(ItemStack::new("minecraft:stick", 4, 0)),
    );

    assert!(events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::InventoryTransactionProcessed {
                transaction,
                accepted: false,
            },
        } if *id == player
            && *transaction == InventoryTransactionContext {
                window_id: 0,
                action_number: 4,
            }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TargetedEvent {
            target: EventTarget::Player(id),
            event: CoreEvent::InventoryContents {
                container: InventoryContainer::Player,
                ..
            },
        } if *id == player
    )));

    let online = core
        .online_players
        .get(&player)
        .expect("player should still be online");
    assert!(online.snapshot.inventory.crafting_result().is_none());
    assert!(online.snapshot.inventory.crafting_input(0).is_none());
    assert!(online.snapshot.inventory.crafting_input(2).is_none());
    assert_eq!(
        online
            .cursor
            .as_ref()
            .map(|stack| (stack.key.as_str(), stack.count)),
        Some(("minecraft:stick", 4))
    );
}
