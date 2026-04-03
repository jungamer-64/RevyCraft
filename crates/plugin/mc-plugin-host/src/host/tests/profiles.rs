use super::*;

#[test]
fn storage_and_auth_plugins_are_managed_without_quarantine() {
    let storage = storage_entrypoints();
    let auth = offline_auth_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .storage_raw(InProcessStoragePlugin {
                plugin_id: "storage-je-anvil-1_7_10".to_string(),
                manifest: storage.manifest,
                factory: storage.factory,
            })
            .auth_raw(InProcessAuthPlugin {
                plugin_id: "auth-offline".to_string(),
                manifest: auth.manifest,
                factory: auth.factory,
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    let _loaded_plugins = host
        .load_protocol_plugin_set()
        .expect("storage/auth plugin kinds should register with the host");

    let status = host.status();
    assert!(
        status
            .storage
            .iter()
            .find(|plugin| plugin.plugin_id == "storage-je-anvil-1_7_10")
            .and_then(|plugin| plugin.active_quarantine_reason.as_ref())
            .is_none()
    );
    assert!(
        status
            .auth
            .iter()
            .find(|plugin| plugin.plugin_id == "auth-offline")
            .and_then(|plugin| plugin.active_quarantine_reason.as_ref())
            .is_none()
    );
}

#[test]
fn auth_plugins_get_fresh_instances_per_host() {
    let entrypoints = fresh_instance_auth_plugin::in_process_plugin_entrypoints();
    let build_host = || {
        build_test_plugin_host(
            TestPluginHostBuilder::new().auth_raw(InProcessAuthPlugin {
                plugin_id: fresh_instance_auth_plugin::PLUGIN_ID.to_string(),
                manifest: entrypoints.manifest,
                factory: entrypoints.factory,
            }),
            PluginAbiRange::default(),
            PluginFailureMatrix::default(),
        )
    };

    let host_a = build_host();
    let host_b = build_host();
    host_a
        .activate_auth_profile(fresh_instance_auth_plugin::PROFILE_ID)
        .expect("fresh-instance auth profile should activate for host A");
    host_b
        .activate_auth_profile(fresh_instance_auth_plugin::PROFILE_ID)
        .expect("fresh-instance auth profile should activate for host B");

    let player_a = host_a
        .resolve_auth_profile(fresh_instance_auth_plugin::PROFILE_ID)
        .expect("fresh-instance auth profile should resolve for host A")
        .authenticate_offline("tester")
        .expect("host A auth should succeed");
    let player_b = host_b
        .resolve_auth_profile(fresh_instance_auth_plugin::PROFILE_ID)
        .expect("fresh-instance auth profile should resolve for host B")
        .authenticate_offline("tester")
        .expect("host B auth should succeed");

    assert_ne!(player_a, player_b);
}

#[test]
fn gameplay_profiles_activate_and_resolve() {
    let canonical = canonical_gameplay_entrypoints();
    let readonly = readonly_gameplay_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .gameplay_raw(InProcessGameplayPlugin {
                plugin_id: "gameplay-canonical".to_string(),
                manifest: canonical.manifest,
                factory: canonical.factory,
            })
            .gameplay_raw(InProcessGameplayPlugin {
                plugin_id: "gameplay-readonly".to_string(),
                manifest: readonly.manifest,
                factory: readonly.factory,
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    host.activate_gameplay_profiles(&RuntimeSelectionConfig {
        default_gameplay_profile: "canonical".into(),
        gameplay_profile_map: std::iter::once(("je-5".into(), "readonly".into())).collect(),
        ..runtime_selection_config()
    })
    .expect("known gameplay profiles should activate");

    assert!(host.resolve_gameplay_profile("canonical").is_some());
    assert!(host.resolve_gameplay_profile("readonly").is_some());
}

#[test]
fn load_plugin_set_activates_runtime_profiles() {
    let protocol = in_process_protocol_entrypoints();
    let canonical = canonical_gameplay_entrypoints();
    let storage = storage_entrypoints();
    let auth = offline_auth_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .protocol_raw(InProcessProtocolPlugin {
                plugin_id: "je-5".to_string(),
                manifest: protocol.manifest,
                factory: protocol.factory,
            })
            .gameplay_raw(InProcessGameplayPlugin {
                plugin_id: "gameplay-canonical".to_string(),
                manifest: canonical.manifest,
                factory: canonical.factory,
            })
            .storage_raw(InProcessStoragePlugin {
                plugin_id: "storage-je-anvil-1_7_10".to_string(),
                manifest: storage.manifest,
                factory: storage.factory,
            })
            .auth_raw(InProcessAuthPlugin {
                plugin_id: "auth-offline".to_string(),
                manifest: auth.manifest,
                factory: auth.factory,
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    let registries = host
        .load_plugin_set(&runtime_selection_config())
        .expect("load_plugin_set should initialize runtime profiles");

    assert!(registries.protocols().resolve_adapter("je-5").is_some());
    assert!(
        registries
            .resolve_storage_profile("je-anvil-1_7_10")
            .is_some()
    );
    assert!(host.resolve_gameplay_profile("canonical").is_some());
    assert!(host.resolve_storage_profile("je-anvil-1_7_10").is_some());
    assert!(host.resolve_auth_profile("offline-v1").is_some());
}

#[test]
fn gameplay_command_snapshot_preserves_entity_id() {
    use revy_voxel_core::{
        CoreConfig, EntityId, GameplayCapabilitySet, GameplayCommand, GameplayProfileId, PlayerId,
        ProtocolCapabilitySet, SessionCapabilitySet,
    };

    let _ = entity_id_probe_gameplay_plugin::take_recorded_session();

    let probe = entity_id_probe_gameplay_plugin::in_process_plugin_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-entity-aware".to_string(),
            manifest: probe.manifest,
            factory: probe.factory,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    host.activate_gameplay_profiles(&RuntimeSelectionConfig {
        default_gameplay_profile: "entity-aware".into(),
        ..runtime_selection_config()
    })
    .expect("entity-aware gameplay profile should activate");

    let profile = host
        .resolve_gameplay_profile("entity-aware")
        .expect("entity-aware gameplay profile should resolve");
    let player_id = PlayerId(Uuid::from_u128(7));
    let core = test_server_core(CoreConfig::default());
    profile
        .prepare_command(
            boxed_gameplay_read_view(core),
            &SessionCapabilitySet {
                protocol: ProtocolCapabilitySet::new(),
                gameplay: GameplayCapabilitySet::new(),
                gameplay_profile: GameplayProfileId::new("entity-aware"),
                entity_id: Some(EntityId(41)),
                protocol_generation: None,
                gameplay_generation: None,
            },
            &GameplayCommand::SetHeldSlot { player_id, slot: 0 },
            0,
        )
        .expect("gameplay command should prepare successfully");

    let recorded = entity_id_probe_gameplay_plugin::take_recorded_session()
        .expect("gameplay plugin should receive a session snapshot");
    assert_eq!(recorded.player_id, Some(player_id));
    assert_eq!(recorded.entity_id, Some(EntityId(41)));
    assert_eq!(recorded.gameplay_profile.as_str(), "entity-aware");
}

#[test]
fn gameplay_prepare_command_journal_replays_host_mutations() {
    use revy_voxel_core::{
        ConnectionId, CoreCommand, CoreEvent, EventTarget, GameplayCapabilitySet, GameplayCommand,
        GameplayEffectApplyResult, GameplayProfileId, ProtocolCapabilitySet, SessionCapabilitySet,
    };

    let _guard = counting_gameplay_plugin::lock();
    counting_gameplay_plugin::reset_invocations();
    let entrypoints = counting_gameplay_plugin::in_process_plugin_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-counting".to_string(),
            manifest: entrypoints.manifest,
            factory: entrypoints.factory,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    host.activate_gameplay_profiles(&RuntimeSelectionConfig {
        default_gameplay_profile: "counting".into(),
        ..runtime_selection_config()
    })
    .expect("counting gameplay profile should activate");

    let profile = host
        .resolve_gameplay_profile("counting")
        .expect("counting gameplay profile should resolve");
    let player_id = PlayerId(Uuid::from_u128(17));
    let mut core = test_server_core(CoreConfig::default());
    let _ = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(1),
            username: "counting".to_string(),
            player_id,
        },
        0,
    );
    let runtime_state = core.export_runtime_state();
    let entity_id = runtime_state
        .online_players
        .get(&player_id)
        .expect("logged-in player should expose an entity id");
    let batch = profile
        .prepare_command(
            boxed_gameplay_read_view(core.clone()),
            &SessionCapabilitySet {
                protocol: ProtocolCapabilitySet::new(),
                gameplay: GameplayCapabilitySet::new(),
                gameplay_profile: GameplayProfileId::new("counting"),
                entity_id: Some(entity_id.session.entity_id),
                protocol_generation: None,
                gameplay_generation: None,
            },
            &GameplayCommand::SetHeldSlot { player_id, slot: 4 },
            0,
        )
        .expect("counting gameplay profile should prepare a detached effect batch");
    let events = match core.validate_and_apply_gameplay_effects(batch) {
        GameplayEffectApplyResult::Applied(events) => events,
        GameplayEffectApplyResult::Conflict => {
            panic!("prepared gameplay effect batch should replay against an unchanged core")
        }
    };

    assert_eq!(counting_gameplay_plugin::command_invocations(), 1);
    assert!(events.iter().any(|event| {
        matches!(
            (&event.target, &event.event),
            (
                EventTarget::Player(event_player_id),
                CoreEvent::SelectedHotbarSlotChanged { slot: 4 }
            ) if event_player_id == &player_id
        )
    }));
    assert_eq!(
        core.export_runtime_state()
            .online_players
            .get(&player_id)
            .expect("player should remain online")
            .player
            .selected_hotbar_slot,
        4
    );
}

#[test]
fn gameplay_prepare_command_conflict_does_not_reinvoke_callback() {
    use revy_voxel_core::{
        ConnectionId, CoreCommand, GameplayCapabilitySet, GameplayCommand,
        GameplayEffectApplyResult, GameplayProfileId, ProtocolCapabilitySet, SessionCapabilitySet,
    };

    let _guard = counting_gameplay_plugin::lock();
    counting_gameplay_plugin::reset_invocations();
    let entrypoints = counting_gameplay_plugin::in_process_plugin_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-counting".to_string(),
            manifest: entrypoints.manifest,
            factory: entrypoints.factory,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    host.activate_gameplay_profiles(&RuntimeSelectionConfig {
        default_gameplay_profile: "counting".into(),
        ..runtime_selection_config()
    })
    .expect("counting gameplay profile should activate");

    let profile = host
        .resolve_gameplay_profile("counting")
        .expect("counting gameplay profile should resolve");
    let player_id = PlayerId(Uuid::from_u128(18));
    let mut core = test_server_core(CoreConfig::default());
    let _ = core.apply_command(
        CoreCommand::LoginStart {
            connection_id: ConnectionId(2),
            username: "counting-stale".to_string(),
            player_id,
        },
        0,
    );
    let runtime_state = core.export_runtime_state();
    let entity_id = runtime_state
        .online_players
        .get(&player_id)
        .expect("logged-in player should expose an entity id");
    let session = SessionCapabilitySet {
        protocol: ProtocolCapabilitySet::new(),
        gameplay: GameplayCapabilitySet::new(),
        gameplay_profile: GameplayProfileId::new("counting"),
        entity_id: Some(entity_id.session.entity_id),
        protocol_generation: None,
        gameplay_generation: None,
    };
    let batch = profile
        .prepare_command(
            boxed_gameplay_read_view(core.clone()),
            &session,
            &GameplayCommand::SetHeldSlot { player_id, slot: 5 },
            0,
        )
        .expect("counting gameplay profile should prepare a detached effect batch");

    let _ = core.apply_command(
        GameplayCommand::SetHeldSlot { player_id, slot: 1 }.into(),
        0,
    );

    assert_eq!(
        core.validate_and_apply_gameplay_effects(batch),
        GameplayEffectApplyResult::Conflict
    );
    assert_eq!(counting_gameplay_plugin::command_invocations(), 1);
    assert_eq!(
        core.export_runtime_state()
            .online_players
            .get(&player_id)
            .expect("player should remain online")
            .player
            .selected_hotbar_slot,
        1
    );
}

#[test]
fn unknown_gameplay_profile_fails_activation() {
    let canonical = canonical_gameplay_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-canonical".to_string(),
            manifest: canonical.manifest,
            factory: canonical.factory,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    let error = host
        .activate_gameplay_profiles(&RuntimeSelectionConfig {
            default_gameplay_profile: "readonly".into(),
            ..runtime_selection_config()
        })
        .expect_err("unknown gameplay profile should fail fast");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("unknown gameplay profile")
    ));
}

#[test]
fn storage_and_auth_profiles_activate_and_resolve() {
    let storage = storage_entrypoints();
    let modern_storage = storage_1_18_2_entrypoints();
    let auth = offline_auth_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .storage_raw(InProcessStoragePlugin {
                plugin_id: "storage-je-anvil-1_7_10".to_string(),
                manifest: storage.manifest,
                factory: storage.factory,
            })
            .storage_raw(InProcessStoragePlugin {
                plugin_id: JE_1_18_2_STORAGE_PLUGIN_ID.to_string(),
                manifest: modern_storage.manifest,
                factory: modern_storage.factory,
            })
            .auth_raw(InProcessAuthPlugin {
                plugin_id: "auth-offline".to_string(),
                manifest: auth.manifest,
                factory: auth.factory,
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    host.activate_storage_profile("je-anvil-1_7_10")
        .expect("known storage profile should activate");
    host.activate_auth_profile("offline-v1")
        .expect("known auth profile should activate");

    assert!(host.resolve_storage_profile("je-anvil-1_7_10").is_some());
    assert!(
        host.resolve_storage_profile(JE_1_18_2_STORAGE_PROFILE_ID)
            .is_none()
    );
    assert!(host.resolve_auth_profile("offline-v1").is_some());
}

#[test]
fn modern_storage_profile_activates_and_resolves() {
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .storage_raw(InProcessStoragePlugin {
                plugin_id: JE_1_18_2_STORAGE_PLUGIN_ID.to_string(),
                manifest: storage_1_18_2_entrypoints().manifest,
                factory: storage_1_18_2_entrypoints().factory,
            })
            .bootstrap_config(BootstrapConfig {
                storage_profile: JE_1_18_2_STORAGE_PROFILE_ID.into(),
                ..BootstrapConfig::default()
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );

    host.activate_storage_profile(JE_1_18_2_STORAGE_PROFILE_ID)
        .expect("known 1.18.2 storage profile should activate");

    assert!(
        host.resolve_storage_profile(JE_1_18_2_STORAGE_PROFILE_ID)
            .is_some()
    );
    assert!(
        host.status()
            .storage
            .iter()
            .any(|plugin| plugin.plugin_id == JE_1_18_2_STORAGE_PLUGIN_ID)
    );
}

#[test]
fn modern_26_1_storage_profile_activates_and_resolves() {
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .storage_raw(InProcessStoragePlugin {
                plugin_id: JE_26_1_STORAGE_PLUGIN_ID.to_string(),
                manifest: storage_26_1_entrypoints().manifest,
                factory: storage_26_1_entrypoints().factory,
            })
            .bootstrap_config(BootstrapConfig {
                storage_profile: JE_26_1_STORAGE_PROFILE_ID.into(),
                ..BootstrapConfig::default()
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );

    host.activate_storage_profile(JE_26_1_STORAGE_PROFILE_ID)
        .expect("known 26.1 storage profile should activate");

    assert!(
        host.resolve_storage_profile(JE_26_1_STORAGE_PROFILE_ID)
            .is_some()
    );
    assert!(
        host.status()
            .storage
            .iter()
            .any(|plugin| plugin.plugin_id == JE_26_1_STORAGE_PLUGIN_ID)
    );
}

#[test]
fn unknown_storage_and_auth_profiles_fail_activation() {
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new(),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    let storage = host
        .activate_storage_profile("missing")
        .expect_err("unknown storage profile should fail fast");
    assert!(matches!(
        storage,
        RuntimeError::Config(message) if message.contains("unknown storage profile")
    ));

    let auth = host
        .activate_auth_profile("missing")
        .expect_err("unknown auth profile should fail fast");
    assert!(matches!(
        auth,
        RuntimeError::Config(message) if message.contains("unknown auth profile")
    ));
}
