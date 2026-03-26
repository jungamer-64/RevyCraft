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

fn block_break_progress_count(
    events: &[TargetedEvent],
    player_id: PlayerId,
    position: BlockPos,
    stage: Option<u8>,
) -> usize {
    events
        .iter()
        .filter(|event| match (&event.target, &event.event) {
            (
                EventTarget::Player(event_player_id),
                CoreEvent::BlockBreakingProgress {
                    position: event_position,
                    stage: event_stage,
                    ..
                },
            ) if *event_player_id == player_id && *event_position == position => {
                *event_stage == stage
            }
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
fn runtime_state_round_trips_live_session_state() {
    let mut core = ServerCore::new(CoreConfig {
        view_distance: 2,
        ..CoreConfig::default()
    });
    let (player_id, _) = login_player(&mut core, 1, "coreblob");
    let _ = spawn_dropped_item_via_tx(
        &mut core,
        Vec3::new(1.5, 4.5, 1.5),
        item("minecraft:diamond", 2),
        0,
    );
    let dropped_item_id = *core
        .entities
        .dropped_items
        .keys()
        .next()
        .expect("dropped item should exist");
    let spawned_item = dropped_item_snapshot(&core, dropped_item_id);

    let _ = core.open_crafting_table(player_id, 7, "Workbench");
    {
        let session = core
            .player_session_mut(player_id)
            .expect("player session should exist");
        session.cursor = Some(item("minecraft:stone", 3));
        session.pending_keep_alive_id = Some(91);
        session.last_keep_alive_sent_at = Some(77);
        session.next_keep_alive_at = 1234;
        session.next_non_player_window_id = 9;
    }
    let entity_id = core
        .player_entity_id(player_id)
        .expect("player entity should exist");
    core.entities.player_active_mining.insert(
        entity_id,
        crate::core::ActiveMiningState {
            position: BlockPos::new(1, 4, 1),
            started_at_ms: 10,
            duration_ms: 250,
            last_stage: Some(3),
            tool_context: None,
        },
    );

    let blob = core.export_runtime_state();
    let restored = ServerCore::from_runtime_state(
        CoreConfig {
            max_players: 40,
            ..CoreConfig::default()
        },
        blob,
    );
    let restored_player = online_player(&restored, player_id);
    let restored_session = restored
        .player_session(player_id)
        .expect("player session should restore");

    assert_eq!(restored.world_meta().max_players, 40);
    assert_eq!(restored_player.snapshot.username, "coreblob");
    assert_eq!(restored_player.cursor, Some(item("minecraft:stone", 3)));
    assert!(matches!(
        restored_player.active_container,
        Some(crate::core::OpenInventoryWindow {
            window_id: 7,
            container: InventoryContainer::CraftingTable,
            ..
        })
    ));
    assert!(restored_player.active_mining.is_some());
    assert_eq!(restored_session.pending_keep_alive_id, Some(91));
    assert_eq!(restored_session.last_keep_alive_sent_at, Some(77));
    assert_eq!(restored_session.next_keep_alive_at, 1234);
    assert_eq!(restored_session.next_non_player_window_id, 9);
    assert_eq!(
        dropped_item_snapshot(&restored, dropped_item_id),
        spawned_item
    );
}

#[test]
fn session_resync_events_cover_live_play_state_without_login_success() {
    let mut core = ServerCore::new(CoreConfig {
        view_distance: 1,
        ..CoreConfig::default()
    });
    let (first, _) = login_player(&mut core, 1, "first");
    let (second, _) = login_player(&mut core, 2, "second");
    let second_entity_id = core
        .player_entity_id(second)
        .expect("second player entity should exist");
    let _ = spawn_dropped_item_via_tx(
        &mut core,
        Vec3::new(1.5, 4.5, 1.5),
        item("minecraft:diamond", 1),
        0,
    );
    let _ = core.open_crafting_table(first, 7, "Workbench");
    core.player_session_mut(first)
        .expect("first player session should exist")
        .cursor = Some(item("minecraft:stone", 2));
    core.entities.player_active_mining.insert(
        second_entity_id,
        crate::core::ActiveMiningState {
            position: BlockPos::new(1, 4, 1),
            started_at_ms: 10,
            duration_ms: 250,
            last_stage: Some(4),
            tool_context: None,
        },
    );

    let events = core.session_resync_events(first);

    assert!(
        events
            .iter()
            .all(|event| !matches!(event.event, CoreEvent::LoginAccepted { .. }))
    );
    assert_player_event(&events, first, |event| {
        matches!(event, CoreEvent::PlayBootstrap { .. })
    });
    assert_player_event(&events, first, |event| {
        matches!(event, CoreEvent::ChunkBatch { .. })
    });
    assert_player_event(
        &events,
        first,
        |event| matches!(event, CoreEvent::EntitySpawned { entity_id, .. } if *entity_id == second_entity_id),
    );
    assert_player_event(&events, first, |event| {
        matches!(event, CoreEvent::DroppedItemSpawned { .. })
    });
    assert_player_event(&events, first, |event| {
        matches!(
            event,
            CoreEvent::BlockBreakingProgress {
                breaker_entity_id,
                stage: Some(4),
                ..
            } if *breaker_entity_id == second_entity_id
        )
    });
    assert_player_event(&events, first, |event| {
        matches!(event, CoreEvent::InventoryContents { window_id: 7, .. })
    });
    assert_player_event(&events, first, |event| {
        matches!(
            event,
            CoreEvent::CursorChanged { stack }
            if stack.as_ref() == Some(&item("minecraft:stone", 2))
        )
    });
    assert_player_event(&events, first, |event| {
        matches!(event, CoreEvent::SelectedHotbarSlotChanged { slot: 0 })
    });
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
    let initial_keep_alive_id = core.sessions.next_keep_alive_id;
    let events = core.tick(DEFAULT_KEEPALIVE_INTERVAL_MS + 1);

    assert_player_event(&events, first, |event| {
        matches!(event, CoreEvent::KeepAliveRequested { .. })
    });
    let session = core
        .player_session(first)
        .expect("player session should still exist after keepalive tick");
    assert_eq!(session.pending_keep_alive_id, Some(initial_keep_alive_id));
    assert_eq!(
        session.last_keep_alive_sent_at,
        Some(DEFAULT_KEEPALIVE_INTERVAL_MS + 1)
    );
    assert_eq!(
        session.next_keep_alive_at,
        (DEFAULT_KEEPALIVE_INTERVAL_MS + 1).saturating_add(DEFAULT_KEEPALIVE_INTERVAL_MS)
    );
    assert_eq!(
        core.sessions.next_keep_alive_id,
        initial_keep_alive_id.saturating_add(1)
    );
}

#[test]
fn keepalive_tick_does_not_duplicate_pending_request() {
    let (mut core, first) = logged_in_core(CoreConfig::default(), 1, "ka-pending");
    let _ = core.tick(DEFAULT_KEEPALIVE_INTERVAL_MS + 1);
    let next_keep_alive_id = core.sessions.next_keep_alive_id;

    let later_events = core.tick(DEFAULT_KEEPALIVE_INTERVAL_MS * 2 + 1);

    assert_eq!(
        count_player_events(&later_events, first, |event| {
            matches!(event, CoreEvent::KeepAliveRequested { .. })
        }),
        0
    );
    assert_eq!(core.sessions.next_keep_alive_id, next_keep_alive_id);
}

#[test]
fn keepalive_timeout_disconnects_without_emitting_new_request() {
    let (mut core, first) = logged_in_core(CoreConfig::default(), 1, "ka-timeout");
    {
        let session = core
            .player_session_mut(first)
            .expect("player session should exist");
        session.pending_keep_alive_id = Some(7);
        session.last_keep_alive_sent_at = Some(0);
        session.next_keep_alive_at = 0;
    }

    let events = core.tick(DEFAULT_KEEPALIVE_TIMEOUT_MS + 1);

    assert_eq!(
        count_player_events(&events, first, |event| {
            matches!(event, CoreEvent::KeepAliveRequested { .. })
        }),
        0
    );
    assert!(core.player_session(first).is_none());
}

#[test]
fn keepalive_response_clears_pending_state_without_emitting_events() {
    let (mut core, first) = logged_in_core(CoreConfig::default(), 1, "ka-ack");
    let _ = core.tick(DEFAULT_KEEPALIVE_INTERVAL_MS + 1);
    let keep_alive_id = core
        .player_session(first)
        .expect("player session should exist")
        .pending_keep_alive_id
        .expect("keepalive should be pending");

    let events = core.apply_command(
        CoreCommand::KeepAliveResponse {
            player_id: first,
            keep_alive_id,
        },
        DEFAULT_KEEPALIVE_INTERVAL_MS + 2,
    );

    assert!(events.is_empty());
    let session = core
        .player_session(first)
        .expect("player should remain online after keepalive ack");
    assert_eq!(session.pending_keep_alive_id, None);
    assert_eq!(session.last_keep_alive_sent_at, None);
}

#[test]
fn keepalive_response_ignores_mismatched_pending_id() {
    let (mut core, first) = logged_in_core(CoreConfig::default(), 1, "ka-ack-mismatch");
    let _ = core.tick(DEFAULT_KEEPALIVE_INTERVAL_MS + 1);
    let session_before = core
        .player_session(first)
        .expect("player session should exist before mismatched ack")
        .clone();
    let pending_keep_alive_id = session_before
        .pending_keep_alive_id
        .expect("keepalive should be pending");

    let events = core.apply_command(
        CoreCommand::KeepAliveResponse {
            player_id: first,
            keep_alive_id: pending_keep_alive_id.saturating_add(1),
        },
        DEFAULT_KEEPALIVE_INTERVAL_MS + 2,
    );

    assert!(events.is_empty());
    let session_after = core
        .player_session(first)
        .expect("player should remain online after mismatched keepalive ack");
    assert_eq!(
        session_after.pending_keep_alive_id,
        Some(pending_keep_alive_id)
    );
    assert_eq!(
        session_after.last_keep_alive_sent_at,
        session_before.last_keep_alive_sent_at
    );
    assert_eq!(
        session_after.next_keep_alive_at,
        session_before.next_keep_alive_at
    );
}

#[test]
fn keepalive_response_matches_manual_transaction_acknowledge() {
    let (mut direct, player_id) = logged_in_core(CoreConfig::default(), 1, "ka-ack-parity");
    let _ = direct.tick(DEFAULT_KEEPALIVE_INTERVAL_MS + 1);
    let keep_alive_id = direct
        .player_session(player_id)
        .expect("player session should exist after keepalive tick")
        .pending_keep_alive_id
        .expect("keepalive should be pending");
    let mut via_tx = direct.clone();

    let direct_events = direct.apply_command(
        CoreCommand::KeepAliveResponse {
            player_id,
            keep_alive_id,
        },
        DEFAULT_KEEPALIVE_INTERVAL_MS + 2,
    );
    let tx_events = apply_test_transaction(&mut via_tx, DEFAULT_KEEPALIVE_INTERVAL_MS + 2, |tx| {
        tx.acknowledge_keep_alive(player_id, keep_alive_id);
    });

    assert_eq!(direct_events, tx_events);
    let direct_session = direct
        .player_session(player_id)
        .expect("direct player session should remain online");
    let tx_session = via_tx
        .player_session(player_id)
        .expect("transaction player session should remain online");
    assert_eq!(
        direct_session.pending_keep_alive_id,
        tx_session.pending_keep_alive_id
    );
    assert_eq!(
        direct_session.last_keep_alive_sent_at,
        tx_session.last_keep_alive_sent_at
    );
    assert_eq!(
        direct_session.next_keep_alive_at,
        tx_session.next_keep_alive_at
    );
}

#[test]
fn keepalive_tick_matches_manual_transaction_request_keep_alive() {
    let (mut direct, player_id) = logged_in_core(CoreConfig::default(), 1, "ka-parity");
    let mut via_tx = direct.clone();
    direct
        .player_session_mut(player_id)
        .expect("player session should exist")
        .next_keep_alive_at = 0;

    let direct_events = direct.tick(250);
    let tx_events = apply_test_transaction(&mut via_tx, 250, |tx| {
        tx.request_keep_alive(player_id);
    });

    assert_eq!(direct_events, tx_events);
    assert_eq!(
        direct.sessions.next_keep_alive_id,
        via_tx.sessions.next_keep_alive_id
    );
    let direct_session = direct
        .player_session(player_id)
        .expect("direct session should exist");
    let tx_session = via_tx
        .player_session(player_id)
        .expect("transaction session should exist");
    assert_eq!(
        direct_session.pending_keep_alive_id,
        tx_session.pending_keep_alive_id
    );
    assert_eq!(
        direct_session.last_keep_alive_sent_at,
        tx_session.last_keep_alive_sent_at
    );
    assert_eq!(
        direct_session.next_keep_alive_at,
        tx_session.next_keep_alive_at
    );
}

#[test]
fn gameplay_transaction_keepalive_preview_uses_overlay_counter_and_writeback() {
    let (mut core, player_id) = logged_in_core(CoreConfig::default(), 1, "tx-ka");
    let initial_keep_alive_id = core.sessions.next_keep_alive_id;

    let events = {
        let mut tx = core.begin_gameplay_transaction(500);
        tx.request_keep_alive(player_id);
        let preview_session = tx
            .player_session_state(player_id)
            .expect("previewed keepalive should expose player session");
        assert_eq!(
            preview_session.pending_keep_alive_id,
            Some(initial_keep_alive_id)
        );
        assert_eq!(preview_session.last_keep_alive_sent_at, Some(500));
        assert_eq!(
            preview_session.next_keep_alive_at,
            500_u64.saturating_add(DEFAULT_KEEPALIVE_INTERVAL_MS)
        );
        assert_eq!(
            tx.next_keep_alive_id(),
            initial_keep_alive_id.saturating_add(1)
        );
        tx.commit()
    };

    assert_player_event(&events, player_id, |event| {
        matches!(
            event,
            CoreEvent::KeepAliveRequested { keep_alive_id }
            if *keep_alive_id == initial_keep_alive_id
        )
    });
    let session = core
        .player_session(player_id)
        .expect("committed keepalive should persist to base state");
    assert_eq!(session.pending_keep_alive_id, Some(initial_keep_alive_id));
    assert_eq!(session.last_keep_alive_sent_at, Some(500));
    assert_eq!(
        session.next_keep_alive_at,
        500_u64.saturating_add(DEFAULT_KEEPALIVE_INTERVAL_MS)
    );
    assert_eq!(
        core.sessions.next_keep_alive_id,
        initial_keep_alive_id.saturating_add(1)
    );
}

#[test]
fn gameplay_move_direct_path_matches_manual_transaction_commit() {
    let mut direct = ServerCore::new(CoreConfig {
        view_distance: 1,
        ..CoreConfig::default()
    });
    let (_first, _) = login_player(&mut direct, 1, "direct-first");
    let (mover, _) = login_player(&mut direct, 2, "direct-mover");
    let mut via_tx = direct.clone();

    let direct_events = direct.apply_command(
        CoreCommand::MoveIntent {
            player_id: mover,
            position: Some(Vec3::new(32.5, 4.0, 0.5)),
            yaw: Some(90.0),
            pitch: Some(-15.0),
            on_ground: true,
        },
        50,
    );
    let tx_events = apply_test_transaction(&mut via_tx, 50, |tx| {
        tx.set_player_pose(
            mover,
            Some(Vec3::new(32.5, 4.0, 0.5)),
            Some(90.0),
            Some(-15.0),
            true,
        );
    });

    assert_eq!(direct_events, tx_events);
    assert_eq!(direct.snapshot(), via_tx.snapshot());
}

#[test]
fn gameplay_inventory_direct_path_matches_manual_transaction_commit() {
    let (mut direct, player_id) = logged_in_creative_core("inventory-parity");
    let mut via_tx = direct.clone();

    let direct_events = creative_inventory_set(
        &mut direct,
        player_id,
        InventorySlot::Hotbar(1),
        Some(item("minecraft:glass", 12)),
    );
    let tx_events = apply_test_transaction(&mut via_tx, 0, |tx| {
        tx.set_inventory_slot(
            player_id,
            InventorySlot::Hotbar(1),
            Some(item("minecraft:glass", 12)),
        );
    });

    assert_eq!(direct_events, tx_events);
    assert_eq!(direct.snapshot(), via_tx.snapshot());
}

#[test]
fn gameplay_set_held_slot_direct_path_matches_manual_transaction_commit() {
    let (mut direct, player_id) = logged_in_creative_core("held-slot-parity");
    let mut via_tx = direct.clone();

    let direct_events = set_held_slot(&mut direct, player_id, 4);
    let tx_events = apply_test_transaction(&mut via_tx, 0, |tx| {
        tx.set_selected_hotbar_slot(player_id, 4);
    });

    assert_eq!(direct_events, tx_events);
    assert_eq!(direct.snapshot(), via_tx.snapshot());
}

#[test]
fn gameplay_begin_mining_direct_path_matches_manual_transaction_commit() {
    let (mut direct, player_id) = logged_in_core(CoreConfig::default(), 1, "mining-parity");
    let mut via_tx = direct.clone();
    let position = BlockPos::new(2, 1, 0);

    let direct_events = direct.apply_command(
        CoreCommand::DigBlock {
            player_id,
            position,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    let tx_events = apply_test_transaction(&mut via_tx, 0, |tx| {
        let player = tx
            .player_snapshot(player_id)
            .expect("player should be online during mining parity test");
        let duration_ms = crate::catalog::survival_mining_duration_ms(
            &tx.block_state(position),
            crate::catalog::tool_spec_for_item(
                player
                    .inventory
                    .selected_hotbar_stack(player.selected_hotbar_slot),
            ),
        )
        .unwrap_or(50);
        tx.begin_mining(player_id, position, duration_ms);
    });

    assert_eq!(direct_events, tx_events);
    assert_eq!(direct.snapshot(), via_tx.snapshot());
}

#[test]
fn gameplay_clear_mining_direct_path_matches_manual_transaction_commit() {
    let (mut base, player_id) = logged_in_core(CoreConfig::default(), 1, "clear-mining-parity");
    let position = BlockPos::new(2, 1, 0);
    let _ = base.apply_command(
        CoreCommand::DigBlock {
            player_id,
            position,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    let mut direct = base.clone();
    let mut via_tx = base.clone();

    let direct_events = direct.apply_command(
        CoreCommand::DigBlock {
            player_id,
            position,
            status: 1,
            face: Some(BlockFace::Top),
        },
        100,
    );
    let tx_events = apply_test_transaction(&mut via_tx, 100, |tx| {
        tx.clear_mining(player_id);
    });

    assert_eq!(direct_events, tx_events);
    assert_eq!(direct.snapshot(), via_tx.snapshot());
}

#[test]
fn tick_emits_scheduler_phases_in_canonical_order() {
    let (mut core, player_id) = logged_in_core(CoreConfig::default(), 1, "tick-order");
    let _ = core.open_furnace(player_id, 3, "Furnace");
    {
        let window = active_container_mut(&mut core, player_id);
        let crate::core::OpenInventoryWindowState::Furnace(furnace) = &mut window.state else {
            panic!("expected furnace state");
        };
        furnace.input = Some(item("minecraft:sand", 1));
        furnace.fuel = Some(item("minecraft:oak_planks", 1));
    }
    let player_position = core
        .compose_player_snapshot(player_id)
        .expect("player should stay online")
        .position;
    let _ = spawn_dropped_item_via_tx(&mut core, player_position, item("minecraft:oak_log", 1), 0);
    let mining_pos = BlockPos::new(2, 1, 0);
    let _ = core.apply_command(
        CoreCommand::DigBlock {
            player_id,
            position: mining_pos,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    core.player_session_mut(player_id)
        .expect("player session should exist")
        .next_keep_alive_at = 0;

    let events = core.tick(500);
    let furnace_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::ContainerPropertyChanged { window_id: 3, .. }
                ) if *event_player_id == player_id
            )
        })
        .expect("furnace maintenance events should be present");
    let drop_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::EntityDespawned { entity_ids }
                ) if *event_player_id == player_id && !entity_ids.is_empty()
            )
        })
        .expect("dropped item phase should despawn the picked up entity");
    let mining_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::BlockBreakingProgress {
                        position,
                        stage: Some(_),
                        ..
                    }
                ) if *event_player_id == player_id && *position == mining_pos
            )
        })
        .expect("mining phase should emit progress after the dropped-item phase");
    let keep_alive_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::KeepAliveRequested { .. }
                ) if *event_player_id == player_id
            )
        })
        .expect("keepalive phase should run after gameplay systems");

    assert!(furnace_index < drop_index);
    assert!(drop_index < mining_index);
    assert!(mining_index < keep_alive_index);
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
    let stale_viewer = player_id("stale-chest-viewer");
    core.world
        .chest_viewers
        .entry(chest_pos)
        .or_default()
        .insert(stale_viewer, 1);

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
    assert!(
        !core
            .world
            .chest_viewers
            .get(&chest_pos)
            .is_some_and(|viewers| viewers.contains_key(&stale_viewer))
    );
    let actor_tx_index = place_events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::InventoryTransactionProcessed {
                        transaction:
                            InventoryTransactionContext {
                                window_id: 1,
                                action_number: 2,
                            },
                        accepted: true,
                    }
                ) if *event_player_id == first
            )
        })
        .expect("actor transaction event should be present");
    let actor_slot_index = place_events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::InventorySlotChanged {
                        window_id: 1,
                        slot: InventorySlot::Container(0),
                        stack,
                        ..
                    }
                ) if *event_player_id == first
                    && stack.as_ref().map(stack_summary) == Some(("minecraft:stone", 2))
            )
        })
        .expect("actor window diff should be present");
    let viewer_slot_index = place_events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::InventorySlotChanged {
                        window_id: 1,
                        slot: InventorySlot::Container(0),
                        stack,
                        ..
                    }
                ) if *event_player_id == second
                    && stack.as_ref().map(stack_summary) == Some(("minecraft:stone", 2))
            )
        })
        .expect("viewer window diff should be present");
    assert!(actor_tx_index < actor_slot_index);
    assert!(actor_slot_index < viewer_slot_index);

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
    let close_index = break_events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::ContainerClosed { window_id: 1 }
                ) if *event_player_id == first
            )
        })
        .expect("player close event should be present before chest removal");
    let block_changed_index = break_events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::BlockChanged { position, block }
                ) if *event_player_id == first
                    && *position == chest_pos
                    && block.is_air()
            )
        })
        .expect("player block change event should be present after chest removal");
    assert!(close_index < block_changed_index);
    assert!(!core.snapshot().block_entities.contains_key(&chest_pos));
}

#[test]
fn disconnecting_world_backed_chest_writes_back_contents_and_unregisters_viewer() {
    let (mut core, first) = logged_in_creative_core("wchest-disc");
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
    let _ = creative_inventory_set(
        &mut core,
        first,
        InventorySlot::Hotbar(0),
        Some(item("minecraft:stone", 3)),
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
        Some(item("minecraft:stone", 3)),
    );
    assert_transaction_processed(&place_events, first, 1, 2, true);

    let disconnect_events = core.apply_command(CoreCommand::Disconnect { player_id: first }, 0);

    assert!(disconnect_events.iter().any(|event| {
        matches!(
            (&event.target, &event.event),
            (
                EventTarget::EveryoneExcept(event_player_id),
                CoreEvent::EntityDespawned { .. }
            ) if *event_player_id == first
        )
    }));
    assert_eq!(
        core.snapshot()
            .block_entities
            .get(&chest_pos)
            .and_then(BlockEntityState::chest_slots)
            .and_then(|slots| slots.first())
            .and_then(Option::as_ref)
            .map(stack_summary),
        Some(("minecraft:stone", 3))
    );
    assert!(!core.world.chest_viewers.contains_key(&chest_pos));
}

#[test]
fn use_block_places_ticks_closes_and_roundtrips_world_backed_furnace() {
    let (mut core, first) = logged_in_creative_core("world-furn-open");
    let furnace_pos = BlockPos::new(2, 4, 0);

    let _ = creative_inventory_set(
        &mut core,
        first,
        InventorySlot::Hotbar(0),
        Some(item("minecraft:furnace", 1)),
    );
    let place_events = core.apply_command(
        CoreCommand::UseBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: BlockPos::new(2, 3, 0),
            face: Some(BlockFace::Top),
            held_item: Some(item("minecraft:furnace", 1)),
        },
        0,
    );
    assert!(
        block_change_count(&place_events, furnace_pos, |block| {
            block.key.as_str() == "minecraft:furnace"
        }) >= 1
    );
    assert_eq!(
        core.snapshot().block_entities.get(&furnace_pos),
        Some(&BlockEntityState::furnace())
    );

    let open_events = core.apply_command(
        CoreCommand::UseBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: furnace_pos,
            face: Some(BlockFace::Top),
            held_item: None,
        },
        0,
    );
    assert_container_opened(&open_events, first, 1, InventoryContainer::Furnace);
    assert_player_window_contents(&open_events, first, 1, InventoryContainer::Furnace);
    assert_container_property_changed(&open_events, first, 1, 0, 0);
    assert_container_property_changed(&open_events, first, 1, 1, 0);
    assert_container_property_changed(&open_events, first, 1, 2, 0);
    assert_container_property_changed(&open_events, first, 1, 3, 200);

    {
        let window = active_container_mut(&mut core, first);
        let crate::core::OpenInventoryWindowState::Furnace(furnace) = &mut window.state else {
            panic!("expected furnace state");
        };
        furnace.input = Some(item("minecraft:sand", 1));
        furnace.fuel = Some(item("minecraft:oak_planks", 1));
    }

    let tick_events = core.tick(50);
    assert_container_property_changed(&tick_events, first, 1, 0, 300);
    assert_container_property_changed(&tick_events, first, 1, 1, 300);
    assert_container_property_changed(&tick_events, first, 1, 2, 1);
    assert_eq!(
        core.snapshot().block_entities.get(&furnace_pos),
        Some(&BlockEntityState::Furnace {
            input: Some(item("minecraft:sand", 1)),
            fuel: None,
            output: None,
            burn_left: 300,
            burn_max: 300,
            cook_progress: 1,
            cook_total: 200,
        })
    );

    let close_events = core.apply_command(
        CoreCommand::CloseContainer {
            player_id: first,
            window_id: 1,
        },
        0,
    );
    assert_container_closed(&close_events, first, 1);
    assert_eq!(
        core.snapshot().block_entities.get(&furnace_pos),
        Some(&BlockEntityState::Furnace {
            input: Some(item("minecraft:sand", 1)),
            fuel: None,
            output: None,
            burn_left: 300,
            burn_max: 300,
            cook_progress: 1,
            cook_total: 200,
        })
    );

    let reopen_events = core.apply_command(
        CoreCommand::UseBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: furnace_pos,
            face: Some(BlockFace::Top),
            held_item: None,
        },
        0,
    );
    assert_container_opened(&reopen_events, first, 2, InventoryContainer::Furnace);

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
fn world_backed_furnace_rejects_break_until_empty_and_closes_viewers_when_removed() {
    let (mut core, first) = logged_in_creative_core("world-furn-break");
    let furnace_pos = BlockPos::new(2, 4, 0);

    let _ = creative_inventory_set(
        &mut core,
        first,
        InventorySlot::Hotbar(0),
        Some(item("minecraft:furnace", 1)),
    );
    let _ = core.apply_command(
        CoreCommand::UseBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: BlockPos::new(2, 3, 0),
            face: Some(BlockFace::Top),
            held_item: Some(item("minecraft:furnace", 1)),
        },
        0,
    );
    let open_events = core.apply_command(
        CoreCommand::UseBlock {
            player_id: first,
            hand: InteractionHand::Main,
            position: furnace_pos,
            face: Some(BlockFace::Top),
            held_item: None,
        },
        0,
    );
    assert_container_opened(&open_events, first, 1, InventoryContainer::Furnace);
    {
        let window = active_container_mut(&mut core, first);
        let crate::core::OpenInventoryWindowState::Furnace(furnace) = &mut window.state else {
            panic!("expected furnace state");
        };
        furnace.output = Some(item("minecraft:glass", 1));
    }
    let _ = core.tick(0);

    let reject_break = core.apply_command(
        CoreCommand::DigBlock {
            player_id: first,
            position: furnace_pos,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    assert_eq!(
        block_change_count(&reject_break, furnace_pos, BlockState::is_air),
        0
    );
    assert_eq!(
        core.snapshot().block_entities.get(&furnace_pos),
        Some(&BlockEntityState::Furnace {
            input: None,
            fuel: None,
            output: Some(item("minecraft:glass", 1)),
            burn_left: 0,
            burn_max: 0,
            cook_progress: 0,
            cook_total: 200,
        })
    );

    {
        let window = active_container_mut(&mut core, first);
        let crate::core::OpenInventoryWindowState::Furnace(furnace) = &mut window.state else {
            panic!("expected furnace state");
        };
        furnace.output = None;
    }
    let _ = core.tick(100);

    let break_events = core.apply_command(
        CoreCommand::DigBlock {
            player_id: first,
            position: furnace_pos,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    assert!(block_change_count(&break_events, furnace_pos, BlockState::is_air) >= 1);
    assert_container_closed(&break_events, first, 1);
    let close_index = break_events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::ContainerClosed { window_id: 1 }
                ) if *event_player_id == first
            )
        })
        .expect("furnace close event should be present");
    let block_changed_index = break_events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::BlockChanged { position, block }
                ) if *event_player_id == first
                    && *position == furnace_pos
                    && block.is_air()
            )
        })
        .expect("furnace removal block change should be present");
    assert!(close_index < block_changed_index);
    assert!(!core.snapshot().block_entities.contains_key(&furnace_pos));
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

    let start_events = core.apply_command(
        CoreCommand::DigBlock {
            player_id: player,
            position: break_pos,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    assert!(block_break_progress_count(&start_events, player, break_pos, Some(0)) >= 1);
    assert_eq!(
        block_change_count(&start_events, break_pos, BlockState::is_air),
        0
    );

    let early = core.tick(2_249);
    assert_eq!(block_change_count(&early, break_pos, BlockState::is_air), 0);

    let break_events = core.tick(2_250);
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
    assert_eq!(
        block_change_count(&grass_break, BlockPos::new(2, 3, 0), BlockState::is_air),
        0
    );
    let grass_finish = core.tick(900);
    assert_player_dropped_item_spawned(&grass_finish, player, Some(("minecraft:dirt", 1)));

    let glass_pos = BlockPos::new(4, 4, 0);
    let _ = set_block_via_tx(&mut core, glass_pos, BlockState::glass(), 0);
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
        block_change_count(&glass_break, glass_pos, BlockState::is_air),
        0
    );
    let glass_finish = core.tick(450);
    assert_eq!(
        count_player_events(&glass_finish, player, |event| {
            matches!(event, CoreEvent::DroppedItemSpawned { .. })
        }),
        0
    );
}

#[test]
fn survival_mining_respects_exact_boundaries_for_dirt_sand_and_stone() {
    for (position, threshold_ms, placed_block) in [
        (BlockPos::new(2, 2, 0), 750_u64, None),
        (BlockPos::new(4, 4, 0), 750_u64, Some(BlockState::sand())),
        (BlockPos::new(2, 1, 0), 2_250_u64, None),
    ] {
        let (mut core, player) = logged_in_core(CoreConfig::default(), 1, "boundary");
        if let Some(block) = placed_block.clone() {
            let _ = set_block_via_tx(&mut core, position, block, 0);
        }

        let start = core.apply_command(
            CoreCommand::DigBlock {
                player_id: player,
                position,
                status: 0,
                face: Some(BlockFace::Top),
            },
            0,
        );
        assert_eq!(block_change_count(&start, position, BlockState::is_air), 0);
        let early = core.tick(threshold_ms.saturating_sub(1));
        assert_eq!(block_change_count(&early, position, BlockState::is_air), 0);
        let finish = core.tick(threshold_ms);
        assert!(block_change_count(&finish, position, BlockState::is_air) >= 1);
    }
}

#[test]
fn survival_mining_cancel_and_held_slot_change_clear_progress() {
    let (mut core, player) = logged_in_core(CoreConfig::default(), 1, "cancel");
    let position = BlockPos::new(2, 2, 0);

    let start = core.apply_command(
        CoreCommand::DigBlock {
            player_id: player,
            position,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    assert!(block_break_progress_count(&start, player, position, Some(0)) >= 1);

    let cancel = core.apply_command(
        CoreCommand::DigBlock {
            player_id: player,
            position,
            status: 1,
            face: Some(BlockFace::Top),
        },
        100,
    );
    assert!(block_break_progress_count(&cancel, player, position, None) >= 1);
    let after_cancel = core.tick(1_000);
    assert_eq!(
        block_change_count(&after_cancel, position, BlockState::is_air),
        0
    );
    assert_eq!(snapshot_block(&core, position), BlockState::dirt());

    let restart = core.apply_command(
        CoreCommand::DigBlock {
            player_id: player,
            position,
            status: 0,
            face: Some(BlockFace::Top),
        },
        1_100,
    );
    assert!(block_break_progress_count(&restart, player, position, Some(0)) >= 1);
    let held_change = set_held_slot(&mut core, player, 1);
    assert!(block_break_progress_count(&held_change, player, position, None) >= 1);
    let after_slot_change = core.tick(2_000);
    assert_eq!(
        block_change_count(&after_slot_change, position, BlockState::is_air),
        0
    );
}

#[test]
fn survival_successful_place_and_external_block_change_clear_active_mining() {
    let (mut core, player) = logged_in_core(CoreConfig::default(), 1, "clearers");
    let mined_pos = BlockPos::new(2, 2, 0);

    let _ = core.apply_command(
        CoreCommand::DigBlock {
            player_id: player,
            position: mined_pos,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    let place = core.apply_command(
        CoreCommand::PlaceBlock {
            player_id: player,
            hand: InteractionHand::Main,
            position: BlockPos::new(4, 3, 0),
            face: Some(BlockFace::Top),
            held_item: Some(item("minecraft:stone", 64)),
        },
        100,
    );
    assert!(block_break_progress_count(&place, player, mined_pos, None) >= 1);
    assert_eq!(
        snapshot_block(&core, BlockPos::new(4, 4, 0)),
        BlockState::stone()
    );
    let after_place = core.tick(1_000);
    assert_eq!(
        block_change_count(&after_place, mined_pos, BlockState::is_air),
        0
    );

    let _ = core.apply_command(
        CoreCommand::DigBlock {
            player_id: player,
            position: mined_pos,
            status: 0,
            face: Some(BlockFace::Top),
        },
        1_100,
    );
    let external = set_block_via_tx(&mut core, mined_pos, BlockState::sand(), 1_200);
    assert!(block_break_progress_count(&external, player, mined_pos, None) >= 1);
    let after_external = core.tick(2_000);
    assert_eq!(snapshot_block(&core, mined_pos), BlockState::sand());
    assert_eq!(
        block_change_count(&after_external, mined_pos, BlockState::is_air),
        0
    );
}

#[test]
fn survival_multiple_players_do_not_share_mining_progress() {
    let mut core = ServerCore::new(CoreConfig::default());
    let (first, _) = login_player(&mut core, 1, "first-miner");
    let (second, _) = login_player(&mut core, 2, "second-miner");
    let position = BlockPos::new(2, 1, 0);

    let _ = core.apply_command(
        CoreCommand::DigBlock {
            player_id: first,
            position,
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );
    let _ = core.apply_command(
        CoreCommand::DigBlock {
            player_id: second,
            position,
            status: 0,
            face: Some(BlockFace::Top),
        },
        1_000,
    );

    let early = core.tick(2_249);
    assert_eq!(block_change_count(&early, position, BlockState::is_air), 0);

    let finish = core.tick(2_250);
    assert!(block_change_count(&finish, position, BlockState::is_air) >= 1);
    assert!(online_player(&core, second).active_mining.is_none());
}

#[test]
fn active_mining_is_not_persisted_in_snapshots() {
    let (mut core, _player) = logged_in_core(CoreConfig::default(), 1, "snapshot-miner");
    let _ = core.apply_command(
        CoreCommand::DigBlock {
            player_id: player_id("snapshot-miner"),
            position: BlockPos::new(2, 1, 0),
            status: 0,
            face: Some(BlockFace::Top),
        },
        0,
    );

    let snapshot = core.snapshot();
    let mut restored = ServerCore::from_snapshot(CoreConfig::default(), snapshot);
    let (_player, _) = login_player(&mut restored, 2, "snapshot-miner");
    let events = restored.tick(3_000);
    assert_eq!(
        block_change_count(&events, BlockPos::new(2, 1, 0), BlockState::is_air),
        0
    );
    assert_eq!(
        snapshot_block(&restored, BlockPos::new(2, 1, 0)),
        BlockState::stone()
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

    let spawn_events = spawn_dropped_item_via_tx(
        &mut core,
        Vec3::new(1.5, 4.5, 0.5),
        item("minecraft:cobblestone", 1),
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
fn survival_high_fall_drop_becomes_pickable_after_settling() {
    let mut core = ServerCore::new(CoreConfig::default());
    let (player, _) = login_player(&mut core, 1, "high-fall");

    let _ = core.apply_command(
        CoreCommand::MoveIntent {
            player_id: player,
            position: Some(Vec3::new(1.5, 4.0, 0.5)),
            yaw: Some(0.0),
            pitch: Some(0.0),
            on_ground: true,
        },
        0,
    );

    let spawn_events = spawn_dropped_item_via_tx(
        &mut core,
        Vec3::new(1.5, 24.5, 0.5),
        item("minecraft:cobblestone", 1),
        0,
    );
    let entity_id = dropped_item_entity_id(&spawn_events, player);

    let early = core.tick(500);
    assert_eq!(
        count_player_events(&early, player, |event| {
            matches!(event, CoreEvent::InventorySlotChanged { .. })
        }),
        0
    );
    let falling_y = dropped_item_snapshot(&core, entity_id).position.y;
    assert!(falling_y < 24.5);
    assert!(falling_y > 4.25);

    let landed = core.tick(4_000);
    assert_inventory_slot_changed_to(
        &landed,
        player,
        InventorySlot::MainInventory(0),
        Some(("minecraft:cobblestone", 1)),
    );
    assert_player_entity_despawned(&landed, player, entity_id);
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

    let _ = apply_test_transaction(&mut core, 0, |tx| {
        tx.set_inventory_slot(
            first,
            InventorySlot::MainInventory(0),
            Some(item("minecraft:cobblestone", 60)),
        );
        tx.set_inventory_slot(
            first,
            InventorySlot::Offhand,
            Some(item("minecraft:dirt", 64)),
        );
        for slot in 1_u8..27 {
            tx.set_inventory_slot(
                first,
                InventorySlot::MainInventory(slot),
                Some(item("minecraft:dirt", 64)),
            );
        }
    });

    let _ = spawn_dropped_item_via_tx(
        &mut core,
        Vec3::new(1.5, 4.5, 0.5),
        item("minecraft:cobblestone", 10),
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

#[test]
fn survival_pickup_prefers_leftmost_hotbar_slot_before_main_inventory() {
    let (mut core, first) = logged_in_core(CoreConfig::default(), 1, "hotbar-first");
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

    let _ = apply_test_transaction(&mut core, 0, |tx| {
        tx.set_inventory_slot(first, InventorySlot::Hotbar(0), None);
        tx.set_inventory_slot(first, InventorySlot::MainInventory(0), None);
    });

    let _ = spawn_dropped_item_via_tx(
        &mut core,
        Vec3::new(1.5, 4.5, 0.5),
        item("minecraft:dirt", 1),
        0,
    );

    let pickup = core.tick(500);
    assert_inventory_slot_changed_to(
        &pickup,
        first,
        InventorySlot::Hotbar(0),
        Some(("minecraft:dirt", 1)),
    );
    assert_eq!(
        count_player_events(&pickup, first, |event| {
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
}

#[test]
fn player_entity_and_session_indexes_stay_in_sync() {
    let mut core = ServerCore::new(CoreConfig::default());
    let (player_id, _) = login_player(&mut core, 1, "ecs-sync");

    let session = core
        .player_session(player_id)
        .expect("login should create a player session");
    let entity_id = session.entity_id;
    assert_eq!(
        core.entities.players_by_player_id.get(&player_id),
        Some(&entity_id)
    );
    assert!(core.entities.player_identity.contains_key(&entity_id));
    assert!(core.entities.player_transform.contains_key(&entity_id));
    assert!(core.entities.player_inventory.contains_key(&entity_id));

    let spawn_events = spawn_dropped_item_via_tx(
        &mut core,
        Vec3::new(3.5, 4.5, 0.5),
        item("minecraft:cobblestone", 1),
        0,
    );
    let drop_entity_id = dropped_item_entity_id(&spawn_events, player_id);
    assert!(core.entities.dropped_items.contains_key(&drop_entity_id));
    assert!(
        core.entities
            .players_by_player_id
            .values()
            .all(|mapped| *mapped != drop_entity_id),
        "dropped item entity should not appear in the player index"
    );

    let _ = core.apply_command(CoreCommand::Disconnect { player_id }, 0);
    assert!(core.player_session(player_id).is_none());
    assert!(!core.entities.players_by_player_id.contains_key(&player_id));
    assert!(!core.entities.player_identity.contains_key(&entity_id));
    assert!(!core.entities.player_transform.contains_key(&entity_id));
    assert!(!core.entities.player_inventory.contains_key(&entity_id));
}

#[test]
fn gameplay_transaction_rollback_discards_entity_and_session_changes() {
    let (mut core, player_id) = logged_in_core(CoreConfig::default(), 1, "tx-rollback");
    let before_snapshot = core
        .compose_player_snapshot(player_id)
        .expect("online player snapshot should compose");
    let before_drop_count = core.entities.dropped_items.len();

    {
        let mut tx = core.begin_gameplay_transaction(0);
        tx.set_player_pose(
            player_id,
            Some(Vec3::new(9.5, 10.0, 0.5)),
            Some(45.0),
            Some(-10.0),
            true,
        );
        tx.spawn_dropped_item(Vec3::new(9.5, 10.5, 0.5), item("minecraft:stone", 1));
        let draft_snapshot = tx
            .player_snapshot(player_id)
            .expect("transaction should expose read-after-write state");
        assert_eq!(draft_snapshot.position, Vec3::new(9.5, 10.0, 0.5));
        assert_eq!(draft_snapshot.yaw, 45.0);
        assert_eq!(draft_snapshot.pitch, -10.0);
    }

    let after_snapshot = core
        .compose_player_snapshot(player_id)
        .expect("uncommitted transaction should leave base state intact");
    assert_eq!(after_snapshot, before_snapshot);
    assert_eq!(core.entities.dropped_items.len(), before_drop_count);
}

#[test]
fn gameplay_transaction_overlay_reads_surface_player_and_block_changes_before_commit() {
    let (mut core, player_id) = logged_in_creative_core("tx-overlay-reads");
    let position = BlockPos::new(2, 4, 0);
    let before_snapshot = core.snapshot();

    {
        let mut tx = core.begin_gameplay_transaction(0);
        tx.set_player_pose(
            player_id,
            Some(Vec3::new(3.5, 4.0, 0.5)),
            Some(90.0),
            Some(-15.0),
            true,
        );
        tx.set_inventory_slot(
            player_id,
            InventorySlot::Hotbar(0),
            Some(item("minecraft:glass", 12)),
        );
        tx.set_block(position, BlockState::chest());

        let player = tx
            .player_snapshot(player_id)
            .expect("overlay snapshot should expose uncommitted player changes");
        assert_eq!(player.position, Vec3::new(3.5, 4.0, 0.5));
        assert_eq!(player.yaw, 90.0);
        assert_eq!(player.pitch, -15.0);
        assert_eq!(
            player
                .inventory
                .get_slot(InventorySlot::Hotbar(0))
                .map(stack_summary),
            Some(("minecraft:glass", 12)),
        );
        assert_eq!(tx.block_state(position), BlockState::chest());
        assert_eq!(
            tx.block_entity(position)
                .and_then(|block_entity| block_entity.chest_slots().map(<[_]>::len)),
            Some(27),
        );
    }

    assert_eq!(core.snapshot(), before_snapshot);
}

#[test]
fn gameplay_transaction_commit_materializes_previewed_state() {
    let (mut core, player_id) = logged_in_creative_core("tx-writeback");
    let block_pos = BlockPos::new(2, 4, 0);
    let moved_pos = Vec3::new(3.5, 4.0, 0.5);
    let drop_pos = Vec3::new(3.5, 4.5, 0.5);
    let _events = apply_test_transaction(&mut core, 0, |tx| {
        tx.set_player_pose(player_id, Some(moved_pos), Some(90.0), Some(-15.0), true);
        tx.set_inventory_slot(
            player_id,
            InventorySlot::Hotbar(0),
            Some(item("minecraft:glass", 12)),
        );
        tx.set_block(block_pos, BlockState::chest());
        tx.spawn_dropped_item(drop_pos, item("minecraft:cobblestone", 4));
        assert_eq!(tx.block_state(block_pos), BlockState::chest());
    });

    let player = core
        .compose_player_snapshot(player_id)
        .expect("committed transaction should materialize player changes");
    assert_eq!(player.position, moved_pos);
    assert_eq!(player.yaw, 90.0);
    assert_eq!(player.pitch, -15.0);
    assert_eq!(
        player
            .inventory
            .get_slot(InventorySlot::Hotbar(0))
            .map(stack_summary),
        Some(("minecraft:glass", 12)),
    );
    assert_eq!(snapshot_block(&core, block_pos), BlockState::chest());
    let (_drop_entity_id, dropped_state) = core
        .entities
        .dropped_items
        .iter()
        .next()
        .expect("committed transaction should materialize dropped item state");
    assert_eq!(core.entities.dropped_items.len(), 1);
    let dropped = &dropped_state.snapshot;
    assert_eq!(dropped.position, drop_pos);
    assert_eq!(stack_summary(&dropped.item), ("minecraft:cobblestone", 4));
}

#[test]
fn gameplay_transaction_commit_preserves_previewed_drop_entity_id_and_allocator_progress() {
    let (mut core, player_id) = logged_in_core(CoreConfig::default(), 1, "tx-drop-id");
    let player_entity_id = core
        .player_session(player_id)
        .expect("logged-in player should have a session")
        .entity_id;

    let (preview_entity_id, events) = {
        let mut tx = core.begin_gameplay_transaction(0);
        tx.spawn_dropped_item(Vec3::new(4.5, 4.5, 0.5), item("minecraft:stone", 1));
        let preview_entity_id = tx
            .dropped_item_ids()
            .into_iter()
            .next()
            .expect("previewed dropped item should be visible inside the transaction");
        let events = tx.commit();
        (preview_entity_id, events)
    };

    let committed_entity_id = dropped_item_entity_id(&events, player_id);
    assert_ne!(committed_entity_id, player_entity_id);
    assert_eq!(committed_entity_id, preview_entity_id);
    assert_eq!(
        core.entities.next_entity_id,
        committed_entity_id.0.saturating_add(1)
    );

    let next_events = spawn_dropped_item_via_tx(
        &mut core,
        Vec3::new(5.5, 4.5, 0.5),
        item("minecraft:granite", 2),
        1,
    );
    let next_entity_id = dropped_item_entity_id(&next_events, player_id);
    assert_eq!(next_entity_id.0, committed_entity_id.0.saturating_add(1));
    assert_eq!(
        core.entities.next_entity_id,
        next_entity_id.0.saturating_add(1)
    );
}

#[test]
fn gameplay_transaction_prepare_login_is_overlay_only_until_finalize() {
    let mut core = ServerCore::new(CoreConfig::default());
    let joining = player_id("prepared-only");

    let events = {
        let mut tx = core.begin_gameplay_transaction(0);
        let prepared = tx
            .begin_login(ConnectionId(7), "prepared".to_string(), joining)
            .expect("prepare should succeed");
        assert!(prepared.is_none());
        let snapshot = tx
            .player_snapshot(joining)
            .expect("prepared login should be visible inside the transaction");
        assert_eq!(snapshot.username, "prepared");
        tx.commit()
    };

    assert!(events.is_empty());
    assert!(core.player_session(joining).is_none());
    assert!(core.player_entity_id(joining).is_none());
    assert!(core.compose_player_snapshot(joining).is_none());
    assert!(!core.world.saved_players.contains_key(&joining));
}

#[test]
fn gameplay_transaction_finalize_uses_overlay_player_state_without_leaking_player_events() {
    let mut core = ServerCore::new(CoreConfig::default());
    let joining = player_id("overlay-join");
    let events = {
        let mut tx = core.begin_gameplay_transaction(0);
        let prepared = tx
            .begin_login(ConnectionId(8), "overlay-join".to_string(), joining)
            .expect("prepare should succeed");
        assert!(prepared.is_none());
        tx.set_inventory_slot(
            joining,
            InventorySlot::Hotbar(0),
            Some(item("minecraft:glass", 32)),
        );
        tx.set_selected_hotbar_slot(joining, 4);
        tx.finalize_login(ConnectionId(8), joining)
            .expect("finalize should succeed");
        tx.commit()
    };

    assert_connection_inventory_contents(&events, ConnectionId(8));
    assert_connection_selected_hotbar_slot(&events, ConnectionId(8), 4);
    assert_eq!(
        count_player_events(&events, joining, |_| true),
        0,
        "pre-finalize player-local mutations should fold into bootstrap, not leak player-targeted events",
    );

    let player = core
        .compose_player_snapshot(joining)
        .expect("finalized player should be online");
    assert_eq!(player.selected_hotbar_slot, 4);
    assert_eq!(
        player
            .inventory
            .get_slot(InventorySlot::Hotbar(0))
            .map(stack_summary),
        Some(("minecraft:glass", 32)),
    );
}

#[test]
fn gameplay_transaction_finalize_includes_pre_finalize_drops_in_connection_sync() {
    let mut core = ServerCore::new(CoreConfig::default());
    let joining = player_id("join-with-drop");
    let events = {
        let mut tx = core.begin_gameplay_transaction(0);
        let prepared = tx
            .begin_login(ConnectionId(9), "join-with-drop".to_string(), joining)
            .expect("prepare should succeed");
        assert!(prepared.is_none());
        tx.spawn_dropped_item(Vec3::new(2.5, 4.5, 0.5), item("minecraft:cobblestone", 3));
        tx.finalize_login(ConnectionId(9), joining)
            .expect("finalize should succeed");
        tx.commit()
    };

    assert_connection_dropped_item_spawned(
        &events,
        ConnectionId(9),
        Some(("minecraft:cobblestone", 3)),
    );
}

#[test]
fn gameplay_transaction_commit_preserves_emit_event_order() {
    let (mut core, player_id) = logged_in_core(CoreConfig::default(), 1, "event-order");
    let marker_pos = BlockPos::new(3, 4, 5);
    let events = apply_test_transaction(&mut core, 0, |tx| {
        tx.set_selected_hotbar_slot(player_id, 2);
        tx.emit_event(
            EventTarget::Player(player_id),
            CoreEvent::BlockChanged {
                position: marker_pos,
                block: BlockState::glass(),
            },
        );
        tx.set_inventory_slot(
            player_id,
            InventorySlot::Hotbar(0),
            Some(item("minecraft:glass", 1)),
        );
    });

    let selected_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::SelectedHotbarSlotChanged { slot: 2 }
                ) if *event_player_id == player_id
            )
        })
        .expect("selected-slot event should be present");
    let marker_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::BlockChanged { position, block }
                ) if *event_player_id == player_id
                    && *position == marker_pos
                    && block.key.as_str() == "minecraft:glass"
            )
        })
        .expect("marker event should be present");
    let inventory_index = events
        .iter()
        .position(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::InventorySlotChanged {
                        slot: InventorySlot::Hotbar(0),
                        ..
                    }
                ) if *event_player_id == player_id
            )
        })
        .expect("inventory event should be present");

    assert!(selected_index < marker_index);
    assert!(marker_index < inventory_index);
}
