use super::support::*;

fn block_change_count<F>(events: &[TargetedEvent], position: BlockPos, predicate: F) -> usize
where
    F: Fn(&BlockState) -> bool,
{
    events
        .iter()
        .filter(|event| match &event.event {
            CoreEvent::BlockChanged {
                position: event_position,
                block,
            } if *event_position == position => predicate(block),
            _ => false,
        })
        .count()
}

fn snapshot_block(core: &ServerCore, position: BlockPos) -> BlockState {
    core.snapshot()
        .chunks
        .get(&position.chunk_pos())
        .map(|chunk| {
            chunk.get_block(
                u8::try_from(position.x.rem_euclid(CHUNK_WIDTH))
                    .expect("snapshot block x should fit into u8"),
                position.y,
                u8::try_from(position.z.rem_euclid(CHUNK_WIDTH))
                    .expect("snapshot block z should fit into u8"),
            )
        })
        .unwrap_or_else(BlockState::air)
}

fn dropped_item_entity_id(events: &[TargetedEvent], player_id: PlayerId) -> EntityId {
    events
        .iter()
        .find_map(|event| match (&event.target, &event.event) {
            (
                EventTarget::Player(event_player_id),
                CoreEvent::DroppedItemSpawned { entity_id, .. },
            ) if *event_player_id == player_id => Some(*entity_id),
            _ => None,
        })
        .expect("dropped item spawn event should exist for player")
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

    let (_first, first_events) = login_player(&mut core, 1, "first");
    assert!(first_events.iter().any(|event| matches!(
        event.event,
        CoreEvent::PlayBootstrap {
            view_distance: 1,
            ..
        }
    )));
    assert!(first_events.iter().any(|event| {
        matches!(event.event, CoreEvent::ChunkBatch { ref chunks } if chunks.len() == 9)
    }));
    assert_connection_inventory_contents(&first_events, ConnectionId(1));
    assert_connection_selected_hotbar_slot(&first_events, ConnectionId(1), 0);

    let (second, second_events) = login_player(&mut core, 2, "second");
    assert_connection_event(&second_events, ConnectionId(2), |event| {
        matches!(event, CoreEvent::EntitySpawned { .. })
    });
    assert_everyone_except_event(&second_events, second, |event| {
        matches!(event, CoreEvent::EntitySpawned { .. })
    });
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
    let (core, player) = logged_in_creative_core("readonly");
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

    let (_first, _) = login_player(&mut core, 1, "first");
    let (second, _) = login_player(&mut core, 2, "second");
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

    assert_everyone_except_event(&events, second, |event| {
        matches!(event, CoreEvent::EntityMoved { .. })
    });
    assert!(
        count_player_events(&events, second, |event| {
            matches!(event, CoreEvent::ChunkBatch { .. })
        }) >= 3
    );
}

#[test]
fn keepalive_tick_emits_keepalive() {
    let (mut core, first) = logged_in_core(CoreConfig::default(), 1, "first");
    let events = core.tick(DEFAULT_KEEPALIVE_INTERVAL_MS + 1);

    assert_player_event(&events, first, |event| {
        matches!(event, CoreEvent::KeepAliveRequested { .. })
    });
}

#[test]
fn world_snapshot_roundtrip_uses_semantic_types() {
    let (core, first) = logged_in_core(CoreConfig::default(), 1, "first");
    let snapshot = core.snapshot();
    let json = serde_json::to_string(&snapshot).expect("snapshot should serialize");
    let decoded: WorldSnapshot = serde_json::from_str(&json).expect("snapshot should deserialize");

    assert_eq!(decoded.meta.level_type, "FLAT");
    assert_eq!(
        decoded
            .chunks
            .values()
            .next()
            .expect("generated chunk should exist")
            .get_block(0, 3, 0)
            .key
            .as_str(),
        "minecraft:grass_block"
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
    let (mut core, first) = logged_in_creative_core("first");

    let slot_events = creative_inventory_set(
        &mut core,
        first,
        InventorySlot::Hotbar(0),
        Some(item("minecraft:glass", 64)),
    );
    assert_inventory_slot_changed_to(
        &slot_events,
        first,
        InventorySlot::Hotbar(0),
        Some(("minecraft:glass", 64)),
    );

    let held_events = set_held_slot(&mut core, first, 4);
    assert_player_selected_hotbar_slot(&held_events, first, 4);

    let snapshot = core.snapshot();
    let player = snapshot.players.get(&first).expect("player should persist");
    assert_eq!(player.selected_hotbar_slot, 4);
    assert_eq!(
        player
            .inventory
            .get_slot(InventorySlot::Hotbar(0))
            .map(stack_summary),
        Some(("minecraft:glass", 64))
    );
}

#[test]
fn update_client_view_clamps_to_server_distance() {
    let mut core = ServerCore::new(CoreConfig {
        view_distance: 2,
        ..CoreConfig::default()
    });
    let (first, _) = login_player(&mut core, 1, "first");

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
        count_player_events(&events, first, |event| {
            matches!(event, CoreEvent::ChunkBatch { chunks } if chunks.len() == 1)
        }),
        16
    );
}

#[test]
fn creative_place_and_break_emit_authoritative_corrections() {
    let mut creative = ServerCore::new(CoreConfig {
        game_mode: 1,
        ..CoreConfig::default()
    });
    let (first, _) = login_player(&mut creative, 1, "first");
    let (_second, _) = login_player(&mut creative, 2, "second");
    let corrected_block = BlockPos::new(2, 4, 0);

    let place_events = creative.apply_command(
        CoreCommand::PlaceBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: BlockPos::new(2, 3, 0),
            face: Some(BlockFace::Top),
            held_item: Some(item("minecraft:stone", 64)),
        },
        0,
    );
    assert!(block_change_count(&place_events, corrected_block, |_| true) >= 2);

    let break_events = creative.apply_command(
        CoreCommand::DigBlock {
            player_id: first,
            position: corrected_block,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    assert!(block_change_count(&break_events, corrected_block, BlockState::is_air) >= 2);
}

#[test]
fn use_block_places_opens_closes_and_roundtrips_world_backed_chest() {
    let (mut core, first) = logged_in_creative_core("world-chest-open");
    let chest_pos = BlockPos::new(2, 4, 0);

    let _ = creative_inventory_set(
        &mut core,
        first,
        InventorySlot::Hotbar(0),
        Some(item("minecraft:chest", 1)),
    );
    let place_events = core.apply_command(
        CoreCommand::UseBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: BlockPos::new(2, 3, 0),
            face: Some(BlockFace::Top),
            held_item: Some(item("minecraft:chest", 1)),
        },
        0,
    );
    assert!(
        block_change_count(&place_events, chest_pos, |block| {
            block.key.as_str() == "minecraft:chest"
        }) >= 1
    );
    assert_eq!(
        core.snapshot()
            .block_entities
            .get(&chest_pos)
            .and_then(BlockEntityState::chest_slots)
            .map(<[_]>::len),
        Some(27)
    );

    let open_events = core.apply_command(
        CoreCommand::UseBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: chest_pos,
            face: Some(BlockFace::Top),
            held_item: None,
        },
        0,
    );
    assert_container_opened(&open_events, first, 1, InventoryContainer::Chest);
    assert_player_window_contents(&open_events, first, 1, InventoryContainer::Chest);

    let close_events = core.apply_command(
        CoreCommand::CloseContainer {
            player_id: first,
            window_id: 1,
        },
        0,
    );
    assert_container_closed(&close_events, first, 1);

    let reopen_events = core.apply_command(
        CoreCommand::UseBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: chest_pos,
            face: Some(BlockFace::Top),
            held_item: None,
        },
        0,
    );
    assert_container_opened(&reopen_events, first, 2, InventoryContainer::Chest);

    let snapshot = core.snapshot();
    let restored = ServerCore::from_snapshot(
        CoreConfig {
            game_mode: 1,
            ..CoreConfig::default()
        },
        snapshot.clone(),
    );
    assert_eq!(restored.snapshot().block_entities, snapshot.block_entities);
}

#[test]
fn world_backed_chest_multiview_syncs_and_only_breaks_when_empty() {
    let mut core = ServerCore::new(CoreConfig {
        game_mode: 1,
        ..CoreConfig::default()
    });
    let (first, _) = login_player(&mut core, 1, "chest-owner");
    let (second, _) = login_player(&mut core, 2, "chest-viewer");
    let chest_pos = BlockPos::new(2, 4, 0);

    let _ = creative_inventory_set(
        &mut core,
        first,
        InventorySlot::Hotbar(0),
        Some(item("minecraft:chest", 1)),
    );
    let place_events = core.apply_command(
        CoreCommand::UseBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: BlockPos::new(2, 3, 0),
            face: Some(BlockFace::Top),
            held_item: Some(item("minecraft:chest", 1)),
        },
        0,
    );
    assert!(
        block_change_count(&place_events, chest_pos, |block| {
            block.key.as_str() == "minecraft:chest"
        }) >= 1
    );
    assert_eq!(
        core.snapshot()
            .chunks
            .get(&chest_pos.chunk_pos())
            .expect("chest chunk should exist")
            .get_block(
                u8::try_from(chest_pos.x.rem_euclid(CHUNK_WIDTH))
                    .expect("local x should fit into u8"),
                chest_pos.y,
                u8::try_from(chest_pos.z.rem_euclid(CHUNK_WIDTH))
                    .expect("local z should fit into u8"),
            )
            .key
            .as_str(),
        "minecraft:chest"
    );
    let _ = creative_inventory_set(
        &mut core,
        first,
        InventorySlot::Hotbar(0),
        Some(item("minecraft:stone", 2)),
    );

    let first_open = core.apply_command(
        CoreCommand::UseBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: chest_pos,
            face: Some(BlockFace::Top),
            held_item: None,
        },
        0,
    );
    let second_open = core.apply_command(
        CoreCommand::UseBlock {
            player_id: second,
            hand: InteractionHand::Main,
            position: chest_pos,
            face: Some(BlockFace::Top),
            held_item: None,
        },
        0,
    );
    assert_container_opened(&first_open, first, 1, InventoryContainer::Chest);
    assert_container_opened(&second_open, second, 1, InventoryContainer::Chest);

    let pickup_events = click_slot(
        &mut core,
        first,
        0,
        1,
        InventorySlot::Hotbar(0),
        InventoryClickButton::Left,
        None,
    );
    assert_transaction_processed(&pickup_events, first, 0, 1, true);

    let place_events = click_slot(
        &mut core,
        first,
        1,
        2,
        InventorySlot::Container(0),
        InventoryClickButton::Left,
        Some(item("minecraft:stone", 2)),
    );
    assert_transaction_processed(&place_events, first, 1, 2, true);
    assert_inventory_slot_changed_in_window_to(
        &place_events,
        first,
        1,
        InventorySlot::Container(0),
        Some(("minecraft:stone", 2)),
    );
    assert_inventory_slot_changed_in_window_to(
        &place_events,
        second,
        1,
        InventorySlot::Container(0),
        Some(("minecraft:stone", 2)),
    );

    let reject_break = core.apply_command(
        CoreCommand::DigBlock {
            player_id: first,
            position: chest_pos,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    assert_eq!(
        block_change_count(&reject_break, chest_pos, BlockState::is_air),
        0
    );
    assert_eq!(
        core.snapshot()
            .block_entities
            .get(&chest_pos)
            .and_then(BlockEntityState::chest_slots)
            .and_then(|slots: &[Option<ItemStack>]| slots.first())
            .and_then(Option::as_ref)
            .map(stack_summary),
        Some(("minecraft:stone", 2))
    );

    let take_back_events = click_slot(
        &mut core,
        first,
        1,
        3,
        InventorySlot::Container(0),
        InventoryClickButton::Left,
        None,
    );
    assert_transaction_processed(&take_back_events, first, 1, 3, true);
    assert_inventory_slot_changed_in_window_to(
        &take_back_events,
        second,
        1,
        InventorySlot::Container(0),
        None,
    );

    let return_hotbar_events = click_slot(
        &mut core,
        first,
        1,
        4,
        InventorySlot::Hotbar(0),
        InventoryClickButton::Left,
        Some(item("minecraft:stone", 2)),
    );
    assert_transaction_processed(&return_hotbar_events, first, 1, 4, true);

    let break_events = core.apply_command(
        CoreCommand::DigBlock {
            player_id: first,
            position: chest_pos,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    assert!(block_change_count(&break_events, chest_pos, BlockState::is_air) >= 2);
    assert_container_closed(&break_events, first, 1);
    assert_container_closed(&break_events, second, 1);
    assert!(!core.snapshot().block_entities.contains_key(&chest_pos));
}

#[test]
fn survival_place_consumes_selected_stack_and_updates_world() {
    let (mut survival, lone) = logged_in_core(CoreConfig::default(), 3, "lone");
    let place_events = survival.apply_command(
        CoreCommand::PlaceBlock {
            player_id: lone,
            hand: InteractionHand::Main,
            position: BlockPos::new(2, 3, 0),
            face: Some(BlockFace::Top),
            held_item: Some(item("minecraft:stone", 64)),
        },
        0,
    );

    assert!(
        block_change_count(&place_events, BlockPos::new(2, 4, 0), |block| {
            *block == BlockState::stone()
        }) >= 1
    );
    assert_inventory_slot_changed_to(
        &place_events,
        lone,
        InventorySlot::Hotbar(0),
        Some(("minecraft:stone", 63)),
    );
    assert_eq!(
        snapshot_block(&survival, BlockPos::new(2, 4, 0)),
        BlockState::stone()
    );
    assert_eq!(
        survival
            .snapshot()
            .players
            .get(&lone)
            .and_then(|player| player.inventory.get_slot(InventorySlot::Hotbar(0)))
            .map(stack_summary),
        Some(("minecraft:stone", 63))
    );
}

#[test]
fn survival_break_spawns_drop_and_snapshot_roundtrip_omits_active_drops() {
    let (mut core, player) = logged_in_core(CoreConfig::default(), 1, "breaker");
    let break_pos = BlockPos::new(2, 1, 0);

    let break_events = core.apply_command(
        CoreCommand::DigBlock {
            player_id: player,
            position: break_pos,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );

    assert!(block_change_count(&break_events, break_pos, BlockState::is_air) >= 1);
    assert_player_dropped_item_spawned_at(
        &break_events,
        player,
        "minecraft:cobblestone",
        1,
        Vec3::new(2.5, 1.5, 0.5),
    );
    assert_eq!(snapshot_block(&core, break_pos), BlockState::air());

    let snapshot = core.snapshot();
    let mut restored = ServerCore::from_snapshot(CoreConfig::default(), snapshot);
    let (_late, login_events) = login_player(&mut restored, 2, "late");
    assert_eq!(
        login_events
            .iter()
            .filter(|event| matches!(event.event, CoreEvent::DroppedItemSpawned { .. }))
            .count(),
        0
    );
}

#[test]
fn survival_break_drop_mapping_handles_grass_and_glass() {
    let (mut core, player) = logged_in_core(CoreConfig::default(), 1, "mapper");

    let grass_break = core.apply_command(
        CoreCommand::DigBlock {
            player_id: player,
            position: BlockPos::new(2, 3, 0),
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    assert_player_dropped_item_spawned(&grass_break, player, Some(("minecraft:dirt", 1)));

    let glass_pos = BlockPos::new(4, 4, 0);
    let _ = core.apply_gameplay_effect(
        GameplayEffect {
            mutations: vec![GameplayMutation::Block {
                position: glass_pos,
                block: BlockState::glass(),
            }],
            emitted_events: Vec::new(),
        },
        0,
    );
    let glass_break = core.apply_command(
        CoreCommand::DigBlock {
            player_id: player,
            position: glass_pos,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    assert_eq!(
        count_player_events(&glass_break, player, |event| {
            matches!(event, CoreEvent::DroppedItemSpawned { .. })
        }),
        0
    );
}

#[test]
fn survival_pickup_delay_and_nearest_player_pickup_work() {
    let mut core = ServerCore::new(CoreConfig::default());
    let (first, _) = login_player(&mut core, 1, "near");
    let (second, _) = login_player(&mut core, 2, "far");

    let _ = core.apply_command(
        CoreCommand::MoveIntent {
            player_id: first,
            position: Some(Vec3::new(1.5, 4.0, 0.5)),
            yaw: Some(0.0),
            pitch: Some(0.0),
            on_ground: true,
        },
        0,
    );
    let _ = core.apply_command(
        CoreCommand::MoveIntent {
            player_id: second,
            position: Some(Vec3::new(4.5, 4.0, 0.5)),
            yaw: Some(0.0),
            pitch: Some(0.0),
            on_ground: true,
        },
        0,
    );

    let spawn_events = core.apply_gameplay_effect(
        GameplayEffect {
            mutations: vec![GameplayMutation::DroppedItem {
                position: Vec3::new(1.5, 4.5, 0.5),
                item: item("minecraft:cobblestone", 1),
            }],
            emitted_events: Vec::new(),
        },
        0,
    );
    let entity_id = dropped_item_entity_id(&spawn_events, first);

    let no_pickup = core.tick(499);
    assert_eq!(
        count_player_events(&no_pickup, first, |event| {
            matches!(
                event,
                CoreEvent::InventorySlotChanged {
                    slot: InventorySlot::MainInventory(0),
                    ..
                }
            )
        }),
        0
    );
    assert_eq!(
        count_player_events(&no_pickup, first, |event| {
            matches!(event, CoreEvent::EntityDespawned { .. })
        }),
        0
    );

    let pickup = core.tick(500);
    assert_inventory_slot_changed_to(
        &pickup,
        first,
        InventorySlot::MainInventory(0),
        Some(("minecraft:cobblestone", 1)),
    );
    assert_eq!(
        count_player_events(&pickup, second, |event| {
            matches!(event, CoreEvent::InventorySlotChanged { .. })
        }),
        0
    );
    assert_player_entity_despawned(&pickup, first, entity_id);
    assert_player_entity_despawned(&pickup, second, entity_id);
}

#[test]
fn survival_partial_pickup_leaves_leftover_drop_for_late_joiners() {
    let (mut core, first) = logged_in_core(CoreConfig::default(), 1, "picker");
    let _ = core.apply_command(
        CoreCommand::MoveIntent {
            player_id: first,
            position: Some(Vec3::new(1.5, 4.0, 0.5)),
            yaw: Some(0.0),
            pitch: Some(0.0),
            on_ground: true,
        },
        0,
    );

    let mut fill_mutations = vec![
        GameplayMutation::InventorySlot {
            player_id: first,
            slot: InventorySlot::MainInventory(0),
            stack: Some(item("minecraft:cobblestone", 60)),
        },
        GameplayMutation::InventorySlot {
            player_id: first,
            slot: InventorySlot::Offhand,
            stack: Some(item("minecraft:dirt", 64)),
        },
    ];
    for slot in 1_u8..27 {
        fill_mutations.push(GameplayMutation::InventorySlot {
            player_id: first,
            slot: InventorySlot::MainInventory(slot),
            stack: Some(item("minecraft:dirt", 64)),
        });
    }
    let _ = core.apply_gameplay_effect(
        GameplayEffect {
            mutations: fill_mutations,
            emitted_events: Vec::new(),
        },
        0,
    );

    let _ = core.apply_gameplay_effect(
        GameplayEffect {
            mutations: vec![GameplayMutation::DroppedItem {
                position: Vec3::new(1.5, 4.5, 0.5),
                item: item("minecraft:cobblestone", 10),
            }],
            emitted_events: Vec::new(),
        },
        0,
    );

    let pickup = core.tick(500);
    assert_inventory_slot_changed_to(
        &pickup,
        first,
        InventorySlot::MainInventory(0),
        Some(("minecraft:cobblestone", 64)),
    );
    assert_eq!(
        count_player_events(&pickup, first, |event| {
            matches!(event, CoreEvent::EntityDespawned { .. })
        }),
        0
    );

    let (_late, login_events) = login_player(&mut core, 2, "late-leftover");
    assert_connection_dropped_item_spawned(
        &login_events,
        ConnectionId(2),
        Some(("minecraft:cobblestone", 6)),
    );
}
