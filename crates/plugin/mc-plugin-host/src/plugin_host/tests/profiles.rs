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
                api: storage.api,
            })
            .auth_raw(InProcessAuthPlugin {
                plugin_id: "auth-offline".to_string(),
                manifest: auth.manifest,
                api: auth.api,
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
fn gameplay_profiles_activate_and_resolve() {
    let canonical = canonical_gameplay_entrypoints();
    let readonly = readonly_gameplay_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .gameplay_raw(InProcessGameplayPlugin {
                plugin_id: "gameplay-canonical".to_string(),
                manifest: canonical.manifest,
                api: canonical.api,
            })
            .gameplay_raw(InProcessGameplayPlugin {
                plugin_id: "gameplay-readonly".to_string(),
                manifest: readonly.manifest,
                api: readonly.api,
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    host.activate_gameplay_profiles(&RuntimeSelectionConfig {
        default_gameplay_profile: "canonical".to_string(),
        gameplay_profile_map: std::iter::once(("je-1_7_10".to_string(), "readonly".to_string()))
            .collect(),
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
                plugin_id: "je-1_7_10".to_string(),
                manifest: protocol.manifest,
                api: protocol.api,
            })
            .gameplay_raw(InProcessGameplayPlugin {
                plugin_id: "gameplay-canonical".to_string(),
                manifest: canonical.manifest,
                api: canonical.api,
            })
            .storage_raw(InProcessStoragePlugin {
                plugin_id: "storage-je-anvil-1_7_10".to_string(),
                manifest: storage.manifest,
                api: storage.api,
            })
            .auth_raw(InProcessAuthPlugin {
                plugin_id: "auth-offline".to_string(),
                manifest: auth.manifest,
                api: auth.api,
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    let registries = host
        .load_plugin_set(&runtime_selection_config())
        .expect("load_plugin_set should initialize runtime profiles");

    assert!(
        registries
            .protocols()
            .resolve_adapter("je-1_7_10")
            .is_some()
    );
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
    use mc_core::{
        BlockPos, BlockState, CapabilitySet, CoreCommand, DimensionId, EntityId, GameplayProfileId,
        GameplayQuery, PlayerId, SessionCapabilitySet, WorldMeta,
    };

    struct NoopQuery;

    impl GameplayQuery for NoopQuery {
        fn world_meta(&self) -> WorldMeta {
            WorldMeta {
                level_name: "world".to_string(),
                seed: 0,
                spawn: BlockPos::new(0, 64, 0),
                dimension: DimensionId::Overworld,
                age: 0,
                time: 0,
                level_type: "FLAT".to_string(),
                game_mode: 0,
                difficulty: 1,
                max_players: 20,
            }
        }

        fn player_snapshot(&self, _player_id: PlayerId) -> Option<mc_core::PlayerSnapshot> {
            None
        }

        fn block_state(&self, _position: BlockPos) -> BlockState {
            BlockState::air()
        }

        fn can_edit_block(&self, _player_id: PlayerId, _position: BlockPos) -> bool {
            true
        }
    }

    let _ = entity_id_probe_gameplay_plugin::take_recorded_session();

    let probe = entity_id_probe_gameplay_plugin::in_process_plugin_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-entity-aware".to_string(),
            manifest: probe.manifest,
            api: probe.api,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    host.activate_gameplay_profiles(&RuntimeSelectionConfig {
        default_gameplay_profile: "entity-aware".to_string(),
        ..runtime_selection_config()
    })
    .expect("entity-aware gameplay profile should activate");

    let profile = host
        .resolve_gameplay_profile("entity-aware")
        .expect("entity-aware gameplay profile should resolve");
    let player_id = PlayerId(Uuid::from_u128(7));
    profile
        .handle_command(
            &NoopQuery,
            &SessionCapabilitySet {
                protocol: CapabilitySet::new(),
                gameplay: CapabilitySet::new(),
                gameplay_profile: GameplayProfileId::new("entity-aware"),
                entity_id: Some(EntityId(41)),
                protocol_generation: None,
                gameplay_generation: None,
            },
            &CoreCommand::SetHeldSlot { player_id, slot: 0 },
        )
        .expect("gameplay command should succeed");

    let recorded = entity_id_probe_gameplay_plugin::take_recorded_session()
        .expect("gameplay plugin should receive a session snapshot");
    assert_eq!(recorded.player_id, Some(player_id));
    assert_eq!(recorded.entity_id, Some(EntityId(41)));
    assert_eq!(recorded.gameplay_profile.as_str(), "entity-aware");
}

#[test]
fn unknown_gameplay_profile_fails_activation() {
    let canonical = canonical_gameplay_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-canonical".to_string(),
            manifest: canonical.manifest,
            api: canonical.api,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    let error = host
        .activate_gameplay_profiles(&RuntimeSelectionConfig {
            default_gameplay_profile: "readonly".to_string(),
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
    let auth = offline_auth_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .storage_raw(InProcessStoragePlugin {
                plugin_id: "storage-je-anvil-1_7_10".to_string(),
                manifest: storage.manifest,
                api: storage.api,
            })
            .auth_raw(InProcessAuthPlugin {
                plugin_id: "auth-offline".to_string(),
                manifest: auth.manifest,
                api: auth.api,
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    host.activate_storage_profile("je-anvil-1_7_10")
        .expect("known storage profile should activate");
    host.activate_auth_profile("offline-v1")
        .expect("known auth profile should activate");

    assert!(host.resolve_storage_profile("je-anvil-1_7_10").is_some());
    assert!(host.resolve_auth_profile("offline-v1").is_some());
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
