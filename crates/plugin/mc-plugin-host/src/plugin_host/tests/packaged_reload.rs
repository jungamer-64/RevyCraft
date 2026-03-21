use super::*;

#[test]
fn packaged_protocol_plugins_load_via_dlopen() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    seed_packaged_plugins(
        &dist_dir,
        &["je-1_7_10", "je-1_8_x", "je-1_12_2", "be-placeholder"],
    )?;

    let config = ServerConfig {
        plugins_dir: dist_dir,
        ..ServerConfig::default()
    };
    let host = TestPluginHost::discover(&config)?.expect("packaged plugins should be discovered");
    let registries = host.load_protocol_plugin_set()?;

    for adapter_id in ["je-1_7_10", "je-1_8_x", "je-1_12_2", "be-placeholder"] {
        let adapter = registries
            .protocols()
            .resolve_adapter(adapter_id)
            .expect("packaged plugin adapter should resolve");
        assert!(
            adapter
                .capability_set()
                .contains(&format!("build-tag:{}", PACKAGED_PLUGIN_TEST_HARNESS_TAG)),
            "adapter `{adapter_id}` should expose build tag capability"
        );
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
    seed_packaged_plugins(&dist_dir, &["je-1_7_10"])?;

    let config = ServerConfig {
        plugins_dir: dist_dir.clone(),
        ..ServerConfig::default()
    };
    let host = TestPluginHost::discover(&config)?.expect("packaged plugins should be discovered");
    let registries = host.load_protocol_plugin_set()?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("packaged je-1_7_10 adapter should resolve");
    let first_generation = adapter
        .plugin_generation_id()
        .expect("packaged adapter should report plugin generation");
    assert!(
        adapter
            .capability_set()
            .contains(&format!("build-tag:{}", PACKAGED_PLUGIN_TEST_HARNESS_TAG))
    );

    std::thread::sleep(Duration::from_secs(1));
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-1_7_10",
            "je-1_7_10",
            &dist_dir,
            &target_dir,
            "reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified()?;
    assert_eq!(reloaded, vec!["je-1_7_10".to_string()]);

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("reloaded adapter should resolve");
    let next_generation = adapter
        .plugin_generation_id()
        .expect("reloaded adapter should report plugin generation");
    assert_ne!(first_generation, next_generation);
    assert!(adapter.capability_set().contains("build-tag:reload-v2"));
    Ok(())
}

#[test]
fn packaged_protocol_reload_with_context_migrates_protocol_sessions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("plugin-host-protocol-migrate");
    seed_packaged_plugins(&dist_dir, &["je-1_7_10"])?;
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-1_7_10-reload-test",
            "je-1_7_10",
            &dist_dir,
            &target_dir,
            "protocol-reload-v1",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let config = ServerConfig {
        plugins_dir: dist_dir.clone(),
        ..ServerConfig::default()
    };
    let host = TestPluginHost::discover(&config)?.expect("packaged plugins should be discovered");
    let registries = host.load_protocol_plugin_set()?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("packaged je-1_7_10 adapter should resolve");
    let before_generation = adapter
        .plugin_generation_id()
        .expect("packaged adapter should report plugin generation");
    assert!(
        adapter
            .capability_set()
            .contains("build-tag:protocol-reload-v1")
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
            "mc-plugin-proto-je-1_7_10-reload-test",
            "je-1_7_10",
            &dist_dir,
            &target_dir,
            "protocol-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified_with_context(&context)?;
    assert!(
        reloaded.iter().any(|plugin_id| plugin_id == "je-1_7_10"),
        "protocol reload should report the migrated adapter"
    );

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("reloaded adapter should resolve");
    let next_generation = adapter
        .plugin_generation_id()
        .expect("reloaded adapter should report plugin generation");
    assert_ne!(before_generation, next_generation);
    assert!(
        adapter
            .capability_set()
            .contains("build-tag:protocol-reload-v2")
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
    seed_packaged_plugins(&dist_dir, &["je-1_7_10"])?;
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-1_7_10-reload-test",
            "je-1_7_10",
            &dist_dir,
            &target_dir,
            "protocol-reload-v1",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let config = ServerConfig {
        plugins_dir: dist_dir.clone(),
        ..ServerConfig::default()
    };
    let host = TestPluginHost::discover(&config)?.expect("packaged plugins should be discovered");
    let registries = host.load_protocol_plugin_set()?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("packaged je-1_7_10 adapter should resolve");
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
            "mc-plugin-proto-je-1_7_10-reload-test",
            "je-1_7_10",
            &dist_dir,
            &target_dir,
            "protocol-reload-fail",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified_with_context(&context)?;
    assert!(
        !reloaded.iter().any(|plugin_id| plugin_id == "je-1_7_10"),
        "failed protocol migration should keep the current generation"
    );

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("adapter should still resolve after failed reload");
    assert_eq!(adapter.plugin_generation_id(), Some(before_generation));
    assert!(
        adapter
            .capability_set()
            .contains("build-tag:protocol-reload-v1")
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
    seed_packaged_plugins(&dist_dir, &["je-1_7_10"])?;
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-1_7_10-reload-test",
            "je-1_7_10",
            &dist_dir,
            &target_dir,
            "protocol-reload-v1",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let config = ServerConfig {
        plugins_dir: dist_dir.clone(),
        ..ServerConfig::default()
    };
    let host = TestPluginHost::discover(&config)?.expect("packaged plugins should be discovered");
    let registries = host.load_protocol_plugin_set()?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("packaged je-1_7_10 adapter should resolve");
    let before_generation = adapter
        .plugin_generation_id()
        .expect("packaged adapter should report plugin generation");

    std::thread::sleep(Duration::from_secs(1));
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-1_7_10-reload-test",
            "je-1_7_10",
            &dist_dir,
            &target_dir,
            "protocol-reload-incompatible",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified()?;
    assert!(
        !reloaded.iter().any(|plugin_id| plugin_id == "je-1_7_10"),
        "incompatible protocol candidate should be rejected"
    );

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("adapter should still resolve after incompatible reload");
    assert_eq!(adapter.plugin_generation_id(), Some(before_generation));
    assert!(
        adapter
            .capability_set()
            .contains("build-tag:protocol-reload-v1")
    );

    let status = host.status();
    let protocol = status
        .protocols
        .iter()
        .find(|plugin| plugin.plugin_id == "je-1_7_10")
        .expect("je-1_7_10 status snapshot should remain present");
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
            "mc-plugin-proto-je-1_7_10-reload-test",
            "je-1_7_10",
            &dist_dir,
            &target_dir,
            "protocol-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let reloaded = host.reload_modified()?;
    assert!(
        reloaded.iter().any(|plugin_id| plugin_id == "je-1_7_10"),
        "successful replacement should clear the quarantined artifact"
    );

    let status = host.status();
    let protocol = status
        .protocols
        .iter()
        .find(|plugin| plugin.plugin_id == "je-1_7_10")
        .expect("je-1_7_10 status snapshot should remain present");
    assert!(protocol.artifact_quarantine.is_none());
    assert!(protocol.generation_id > before_generation);
    assert!(protocol.loaded_at_ms > 0);
    Ok(())
}
