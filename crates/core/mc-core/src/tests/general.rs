use super::*;

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
fn survival_place_rejection_emits_authoritative_corrections() {
    let (mut survival, lone) = logged_in_core(CoreConfig::default(), 3, "lone");
    let reject_events = survival.apply_command(
        CoreCommand::PlaceBlock {
            player_id: lone,
            hand: InteractionHand::Main,
            position: BlockPos::new(2, 3, 0),
            face: Some(BlockFace::Top),
            held_item: Some(item("minecraft:stone", 64)),
        },
        0,
    );

    assert_player_event(&reject_events, lone, |event| {
        matches!(
            event,
            CoreEvent::BlockChanged {
                position,
                block,
            } if *position == BlockPos::new(2, 4, 0) && block.is_air()
        )
    });
    assert_inventory_slot_changed(&reject_events, lone, InventorySlot::Hotbar(0));
}
