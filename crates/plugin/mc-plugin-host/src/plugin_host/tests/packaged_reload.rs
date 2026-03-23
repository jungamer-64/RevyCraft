use super::*;

fn seed_legacy_gameplay_artifact_without_v2_symbol(dist_dir: &Path) -> Result<(), RuntimeError> {
    let protocol_dir = dist_dir.join("je-5");
    let artifact_name = fs::read_dir(&protocol_dir)?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            (file_name != "plugin.toml").then(|| file_name.into_owned())
        })
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "packaged protocol artifact missing from {}",
                protocol_dir.display()
            ))
        })?;
    let legacy_dir = dist_dir.join("gameplay-legacy");
    fs::create_dir_all(&legacy_dir)?;
    fs::write(
        legacy_dir.join("plugin.toml"),
        format!(
            "[plugin]\nid = \"gameplay-legacy\"\nkind = \"gameplay\"\n\n[artifacts]\n\"{}\" = \"../je-5/{}\"\n",
            current_artifact_key(),
            artifact_name,
        ),
    )?;
    Ok(())
}

fn protocol_build_tag(host: &TestPluginHost, plugin_id: &str) -> Option<String> {
    host.status()
        .protocols
        .iter()
        .find(|plugin| plugin.plugin_id == plugin_id)
        .and_then(|plugin| plugin.build_tag.as_ref())
        .map(|tag| tag.as_str().to_string())
}

#[test]
fn packaged_protocol_plugins_load_via_dlopen() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    seed_packaged_plugins(&dist_dir, &["je-5", "je-47", "je-340", "be-placeholder"])?;

    let bootstrap = bootstrap_config_with_plugins_dir(dist_dir);
    let host =
        TestPluginHost::discover(&bootstrap)?.expect("packaged plugins should be discovered");
    let registries = host.load_protocol_plugin_set()?;

    for adapter_id in ["je-5", "je-47", "je-340", "be-placeholder"] {
        assert!(
            registries.protocols().resolve_adapter(adapter_id).is_some(),
            "packaged plugin adapter should resolve"
        );
        assert_eq!(
            protocol_build_tag(&host, adapter_id).as_deref(),
            Some(PACKAGED_PLUGIN_TEST_HARNESS_TAG),
            "adapter `{adapter_id}` should expose the packaged build tag"
        );
    }

    Ok(())
}

#[test]
fn packaged_gameplay_boot_load_respects_failure_policy_for_missing_v2_symbol()
-> Result<(), RuntimeError> {
    use mc_core::{
        CoreCommand, EntityId, GameplayCapabilitySet, GameplayProfileId, PlayerId,
        ProtocolCapabilitySet, SessionCapabilitySet,
    };
    use uuid::Uuid;

    for action in [
        PluginFailureAction::Skip,
        PluginFailureAction::Quarantine,
        PluginFailureAction::FailFast,
    ] {
        let temp_dir = tempdir()?;
        let dist_dir = temp_dir.path().join("runtime").join("plugins");
        seed_packaged_plugins(
            &dist_dir,
            &[
                "je-5",
                "gameplay-canonical",
                "storage-je-anvil-1_7_10",
                "auth-offline",
            ],
        )?;
        seed_legacy_gameplay_artifact_without_v2_symbol(&dist_dir)?;

        let bootstrap = bootstrap_config_with_plugins_dir(dist_dir);
        let runtime_selection = RuntimeSelectionConfig {
            plugin_failure_policy_gameplay: action,
            ..runtime_selection_config()
        };
        let host =
            TestPluginHost::discover(&bootstrap)?.expect("packaged plugins should be discovered");

        match action {
            PluginFailureAction::FailFast => {
                let error = match host.load_plugin_set(&runtime_selection) {
                    Ok(_) => panic!("fail-fast should reject the incompatible gameplay artifact"),
                    Err(error) => error,
                };
                assert!(matches!(
                    error,
                    RuntimeError::PluginFatal(message)
                        if message.contains("gameplay-legacy")
                            && message.contains("failed to resolve gameplay api symbol")
                ));
            }
            PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                let loaded = host.load_plugin_set(&runtime_selection)?;
                let profile = loaded
                    .resolve_gameplay_profile("canonical")
                    .expect("canonical gameplay profile should resolve");
                let result = profile.handle_command(
                    &StubGameplayQuery {
                        level_name: "world",
                    },
                    &SessionCapabilitySet {
                        protocol: ProtocolCapabilitySet::new(),
                        gameplay: GameplayCapabilitySet::new(),
                        gameplay_profile: GameplayProfileId::new("canonical"),
                        entity_id: Some(EntityId(3)),
                        protocol_generation: None,
                        gameplay_generation: None,
                    },
                    &CoreCommand::SetHeldSlot {
                        player_id: PlayerId(Uuid::from_u128(44)),
                        slot: 1,
                    },
                );
                assert!(
                    result.is_ok(),
                    "compatible packaged gameplay plugin should still invoke"
                );
            }
        }
    }

    Ok(())
}

#[test]
fn packaged_protocol_reload_replaces_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("plugin-host-reload");
    seed_packaged_plugins(&dist_dir, &["je-5"])?;

    let bootstrap = bootstrap_config_with_plugins_dir(dist_dir.clone());
    let host =
        TestPluginHost::discover(&bootstrap)?.expect("packaged plugins should be discovered");
    let registries = host.load_protocol_plugin_set()?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-5")
        .expect("packaged je-5 adapter should resolve");
    let first_generation = adapter
        .plugin_generation_id()
        .expect("packaged adapter should report plugin generation");
    assert_eq!(
        protocol_build_tag(&host, "je-5").as_deref(),
        Some(PACKAGED_PLUGIN_TEST_HARNESS_TAG)
    );

    std::thread::sleep(Duration::from_secs(1));
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5",
            "je-5",
            &dist_dir,
            &target_dir,
            "reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified()?;
    assert_eq!(reloaded, vec!["je-5".to_string()]);

    let adapter = registries
        .protocols()
        .resolve_adapter("je-5")
        .expect("reloaded adapter should resolve");
    let next_generation = adapter
        .plugin_generation_id()
        .expect("reloaded adapter should report plugin generation");
    assert_ne!(first_generation, next_generation);
    assert_eq!(
        protocol_build_tag(&host, "je-5").as_deref(),
        Some("reload-v2")
    );
    Ok(())
}

#[test]
fn packaged_protocol_reload_with_context_migrates_protocol_sessions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("plugin-host-protocol-migrate");
    seed_packaged_plugins(&dist_dir, &["je-5"])?;
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            "je-5",
            &dist_dir,
            &target_dir,
            "protocol-reload-v1",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let bootstrap = bootstrap_config_with_plugins_dir(dist_dir.clone());
    let host =
        TestPluginHost::discover(&bootstrap)?.expect("packaged plugins should be discovered");
    let registries = host.load_protocol_plugin_set()?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-5")
        .expect("packaged je-5 adapter should resolve");
    let before_generation = adapter
        .plugin_generation_id()
        .expect("packaged adapter should report plugin generation");
    assert_eq!(
        protocol_build_tag(&host, "je-5").as_deref(),
        Some("protocol-reload-v1")
    );

    let player_id = PlayerId(Uuid::from_u128(7));
    let context = protocol_reload_context(vec![
        protocol_reload_session(3, ConnectionPhase::Login, None, None),
        protocol_reload_session(
            11,
            ConnectionPhase::Play,
            Some(player_id),
            Some(EntityId(41)),
        ),
    ]);

    std::thread::sleep(Duration::from_secs(1));
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            "je-5",
            &dist_dir,
            &target_dir,
            "protocol-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified_with_context(&context)?;
    assert!(
        reloaded.iter().any(|plugin_id| plugin_id == "je-5"),
        "protocol reload should report the migrated adapter"
    );

    let adapter = registries
        .protocols()
        .resolve_adapter("je-5")
        .expect("reloaded adapter should resolve");
    let next_generation = adapter
        .plugin_generation_id()
        .expect("reloaded adapter should report plugin generation");
    assert_ne!(before_generation, next_generation);
    assert_eq!(
        protocol_build_tag(&host, "je-5").as_deref(),
        Some("protocol-reload-v2")
    );
    Ok(())
}

#[test]
fn packaged_protocol_reload_with_context_is_all_or_nothing() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("plugin-host-protocol-all-or-nothing");
    seed_packaged_plugins(&dist_dir, &["je-5"])?;
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            "je-5",
            &dist_dir,
            &target_dir,
            "protocol-reload-v1",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let bootstrap = bootstrap_config_with_plugins_dir(dist_dir.clone());
    let host =
        TestPluginHost::discover(&bootstrap)?.expect("packaged plugins should be discovered");
    let registries = host.load_protocol_plugin_set()?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-5")
        .expect("packaged je-5 adapter should resolve");
    let before_generation = adapter
        .plugin_generation_id()
        .expect("packaged adapter should report plugin generation");

    let player_id = PlayerId(Uuid::from_u128(9));
    let context = protocol_reload_context(vec![
        protocol_reload_session(5, ConnectionPhase::Login, None, None),
        protocol_reload_session(
            17,
            ConnectionPhase::Play,
            Some(player_id),
            Some(EntityId(55)),
        ),
    ]);

    std::thread::sleep(Duration::from_secs(1));
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            "je-5",
            &dist_dir,
            &target_dir,
            "protocol-reload-fail",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified_with_context(&context)?;
    assert!(
        !reloaded.iter().any(|plugin_id| plugin_id == "je-5"),
        "failed protocol migration should keep the current generation"
    );

    let adapter = registries
        .protocols()
        .resolve_adapter("je-5")
        .expect("adapter should still resolve after failed reload");
    assert_eq!(adapter.plugin_generation_id(), Some(before_generation));
    assert_eq!(
        protocol_build_tag(&host, "je-5").as_deref(),
        Some("protocol-reload-v1")
    );
    Ok(())
}

#[test]
fn packaged_protocol_reload_rejects_incompatible_candidate() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("plugin-host-protocol-incompatible");
    seed_packaged_plugins(&dist_dir, &["je-5"])?;
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            "je-5",
            &dist_dir,
            &target_dir,
            "protocol-reload-v1",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let bootstrap = bootstrap_config_with_plugins_dir(dist_dir.clone());
    let host =
        TestPluginHost::discover(&bootstrap)?.expect("packaged plugins should be discovered");
    let registries = host.load_protocol_plugin_set()?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-5")
        .expect("packaged je-5 adapter should resolve");
    let before_generation = adapter
        .plugin_generation_id()
        .expect("packaged adapter should report plugin generation");

    std::thread::sleep(Duration::from_secs(1));
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            "je-5",
            &dist_dir,
            &target_dir,
            "protocol-reload-incompatible",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified()?;
    assert!(
        !reloaded.iter().any(|plugin_id| plugin_id == "je-5"),
        "incompatible protocol candidate should be rejected"
    );

    let adapter = registries
        .protocols()
        .resolve_adapter("je-5")
        .expect("adapter should still resolve after incompatible reload");
    assert_eq!(adapter.plugin_generation_id(), Some(before_generation));
    assert_eq!(
        protocol_build_tag(&host, "je-5").as_deref(),
        Some("protocol-reload-v1")
    );

    let status = host.status();
    let protocol = status
        .protocols
        .iter()
        .find(|plugin| plugin.plugin_id == "je-5")
        .expect("je-5 status snapshot should remain present");
    assert_eq!(protocol.generation_id, before_generation);
    assert!(protocol.loaded_at_ms > 0);
    assert_eq!(
        protocol.current_artifact.modified_at_ms,
        protocol.loaded_at_ms
    );
    assert!(protocol.artifact_quarantine.is_some());
    assert!(protocol.current_artifact.reason.is_none());

    std::thread::sleep(Duration::from_secs(1));
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            "je-5",
            &dist_dir,
            &target_dir,
            "protocol-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let reloaded = host.reload_modified()?;
    assert!(
        reloaded.iter().any(|plugin_id| plugin_id == "je-5"),
        "successful replacement should clear the quarantined artifact"
    );

    let status = host.status();
    let protocol = status
        .protocols
        .iter()
        .find(|plugin| plugin.plugin_id == "je-5")
        .expect("je-5 status snapshot should remain present");
    assert!(protocol.artifact_quarantine.is_none());
    assert!(protocol.generation_id > before_generation);
    assert!(protocol.loaded_at_ms > 0);
    Ok(())
}
