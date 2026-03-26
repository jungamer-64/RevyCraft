use super::*;
use mc_proto_common::ConnectionPhase;
use tokio::sync::Mutex as TokioMutex;

static RELOAD_CORE_TEST_LOCK: TokioMutex<()> = TokioMutex::const_new(());

fn core_reload_server_config(world_dir: PathBuf, dist_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.game_mode = 1;
    config.bootstrap.plugins_dir = dist_dir;
    config
}

#[tokio::test]
async fn core_reload_preserves_live_java_session() -> Result<(), RuntimeError> {
    let _guard = RELOAD_CORE_TEST_LOCK.lock().await;
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.game_mode = 1;
    let server = build_reloadable_test_server(config, plugin_test_registries_all()?).await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (_stream, _buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "core-reload").await?;

    let result = server.reload_runtime_core().await?;
    assert_eq!(result, crate::runtime::CoreReloadResult {});
    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].phase, ConnectionPhase::Play);
    assert_eq!(
        server
            .runtime
            .kernel
            .export_core_runtime_state()
            .await
            .blob
            .online_players
            .len(),
        1
    );

    server.shutdown().await
}

#[tokio::test]
async fn core_reload_updates_live_core_config_and_preserves_keepalive_state()
-> Result<(), RuntimeError> {
    let _guard = RELOAD_CORE_TEST_LOCK.lock().await;
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;

    let initial = core_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (_stream, _buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "corecfg").await?;
    let before = server.runtime.kernel.export_core_runtime_state().await;
    let before_session = before
        .blob
        .online_players
        .values()
        .next()
        .expect("one online player should exist")
        .session
        .clone();

    std::thread::sleep(Duration::from_secs(1));
    let mut updated = initial.clone();
    updated.bootstrap.level_name = "renamed-world".to_string();
    updated.bootstrap.view_distance = 4;
    updated.bootstrap.game_mode = 0;
    updated.bootstrap.difficulty = 3;
    updated.network.max_players = 31;
    write_server_toml(&config_path, &updated)?;

    let result = server.reload_runtime_core().await?;
    assert_eq!(result, crate::runtime::CoreReloadResult {});

    let after = server.runtime.kernel.export_core_runtime_state().await;
    let after_session = after
        .blob
        .online_players
        .values()
        .next()
        .expect("one online player should still exist")
        .session
        .clone();
    assert_eq!(
        before_session.pending_keep_alive_id,
        after_session.pending_keep_alive_id
    );
    assert_eq!(
        before_session.last_keep_alive_sent_at,
        after_session.last_keep_alive_sent_at
    );
    assert_eq!(
        before_session.next_keep_alive_at,
        after_session.next_keep_alive_at
    );
    assert_eq!(after.blob.snapshot.meta.level_name, "renamed-world");
    assert_eq!(after.blob.snapshot.meta.game_mode, 0);
    assert_eq!(after.blob.snapshot.meta.difficulty, 3);
    assert_eq!(after.blob.snapshot.meta.max_players, 31);
    let selection = server.runtime.selection_state().await;
    assert_eq!(selection.config.bootstrap.level_name, "renamed-world");
    assert_eq!(selection.config.bootstrap.view_distance, 4);
    assert_eq!(selection.config.bootstrap.game_mode, 0);
    assert_eq!(selection.config.bootstrap.difficulty, 3);
    assert_eq!(selection.config.network.max_players, 31);

    server.shutdown().await
}

#[tokio::test]
async fn full_reload_preserves_live_java_session() -> Result<(), RuntimeError> {
    let _guard = RELOAD_CORE_TEST_LOCK.lock().await;
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.game_mode = 1;
    let server = build_reloadable_test_server(config, plugin_test_registries_all()?).await?;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;

    let (_stream, _buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "full-reload").await?;
    let before_generation = server.runtime.active_generation_id();

    let result = server.reload_runtime_full().await?;
    assert_eq!(result.topology.activated_generation_id, before_generation);
    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].phase, ConnectionPhase::Play);

    server.shutdown().await
}

#[tokio::test]
async fn full_reload_updates_live_play_session_generations_without_resending_login_success()
-> Result<(), RuntimeError> {
    let _guard = RELOAD_CORE_TEST_LOCK.lock().await;
    let temp_dir = tempdir()?;
    let (server, dist_dir, target_dir, before_protocol_generation) =
        spawn_protocol_reload_server(&temp_dir, "full-reload-generation-swap").await?;
    let before_gameplay_generation = loaded_plugins_snapshot(&server)
        .await
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should resolve")
        .plugin_generation_id()
        .expect("canonical gameplay profile should report generation");
    let codec = MinecraftWireCodec;
    let addr = listener_addr(&server);
    let (mut alpha, mut alpha_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "full-hot").await?;
    let _ = read_until_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;

    std::thread::sleep(Duration::from_secs(1));
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            JE_5_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    harness
        .install_gameplay_plugin(
            "mc-plugin-gameplay-canonical",
            "gameplay-canonical",
            &dist_dir,
            &target_dir,
            "gameplay-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let result = server.reload_runtime_full().await?;
    assert!(
        result
            .reloaded_plugin_ids
            .iter()
            .any(|plugin_id| plugin_id == JE_5_ADAPTER_ID)
    );
    assert!(
        result
            .reloaded_plugin_ids
            .iter()
            .any(|plugin_id| plugin_id == "gameplay-canonical")
    );
    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].phase, ConnectionPhase::Play);
    assert_ne!(
        sessions[0].protocol_generation,
        Some(before_protocol_generation)
    );
    assert_ne!(
        sessions[0].gameplay_generation,
        Some(before_gameplay_generation)
    );
    assert_eq!(
        protocol_build_tag(&server, JE_5_ADAPTER_ID).as_deref(),
        Some("protocol-reload-v2")
    );
    assert_eq!(
        gameplay_build_tag(&server, "canonical").as_deref(),
        Some("gameplay-reload-v2")
    );

    assert_no_packet_id(&mut alpha, &codec, &mut alpha_buffer, 0x02).await?;
    write_packet(&mut alpha, &codec, &held_item_change(4)).await?;
    let held_item = read_until_java_packet(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        TestJavaPacket::HeldItemChange,
    )
    .await?;
    assert_eq!(held_item_from_packet(&held_item)?, 4);

    server.shutdown().await
}

#[tokio::test]
async fn core_reload_rolls_back_core_swap_when_reattach_fails() -> Result<(), RuntimeError> {
    let _guard = RELOAD_CORE_TEST_LOCK.lock().await;
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;
    let initial = core_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let codec = MinecraftWireCodec;
    let addr = listener_addr(&server);
    let (_alpha, _alpha_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "rollback-a").await?;
    let (_beta, _beta_buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "rollback-b").await?;
    let before_snapshot = server.runtime.kernel.export_core_runtime_state().await;
    let before_selection = server.runtime.selection_state().await;

    std::thread::sleep(Duration::from_secs(1));
    let mut updated = initial.clone();
    updated.bootstrap.level_name = "rollback-world".to_string();
    updated.bootstrap.view_distance = 4;
    updated.network.max_players = 31;
    write_server_toml(&config_path, &updated)?;

    server.runtime.fail_nth_reattach_send_for_test(2);
    let error = server
        .reload_runtime_core()
        .await
        .expect_err("reattach failure should abort core reload");
    assert!(error.to_string().contains("injected reattach failure"));
    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 2);
    assert!(
        sessions
            .iter()
            .all(|session| session.phase == ConnectionPhase::Play)
    );
    let after_snapshot = server.runtime.kernel.export_core_runtime_state().await;
    let after_selection = server.runtime.selection_state().await;
    assert_eq!(
        before_snapshot.blob.snapshot.meta.level_name,
        after_snapshot.blob.snapshot.meta.level_name
    );
    assert_eq!(
        before_snapshot.blob.snapshot.meta.max_players,
        after_snapshot.blob.snapshot.meta.max_players
    );
    assert_eq!(
        before_selection.config.bootstrap.level_name,
        after_selection.config.bootstrap.level_name
    );
    assert_eq!(
        before_selection.config.bootstrap.view_distance,
        after_selection.config.bootstrap.view_distance
    );
    assert_eq!(
        before_selection.config.network.max_players,
        after_selection.config.network.max_players
    );

    server.shutdown().await
}

#[tokio::test]
async fn full_reload_keeps_plugin_host_and_session_generations_unchanged_when_reattach_fails()
-> Result<(), RuntimeError> {
    let _guard = RELOAD_CORE_TEST_LOCK.lock().await;
    let temp_dir = tempdir()?;
    let (server, dist_dir, target_dir, before_protocol_generation) =
        spawn_protocol_reload_server(&temp_dir, "full-reload-reattach-fail").await?;
    let before_gameplay_generation = loaded_plugins_snapshot(&server)
        .await
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should resolve")
        .plugin_generation_id()
        .expect("canonical gameplay profile should report generation");
    let before_runtime_generation = server.runtime.active_generation_id();
    let codec = MinecraftWireCodec;
    let addr = listener_addr(&server);
    let (_stream, _buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "reattach-fail").await?;

    std::thread::sleep(Duration::from_secs(1));
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    harness
        .install_protocol_plugin(
            "mc-plugin-proto-je-5-reload-test",
            JE_5_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    harness
        .install_gameplay_plugin(
            "mc-plugin-gameplay-canonical",
            "gameplay-canonical",
            &dist_dir,
            &target_dir,
            "gameplay-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    server.runtime.fail_nth_reattach_send_for_test(1);
    let error = server
        .reload_runtime_full()
        .await
        .expect_err("reattach failure should abort full reload");
    assert!(error.to_string().contains("injected reattach failure"));
    assert_eq!(
        server.runtime.active_generation_id(),
        before_runtime_generation
    );
    assert_eq!(
        protocol_build_tag(&server, JE_5_ADAPTER_ID).as_deref(),
        Some("protocol-reload-v1")
    );
    assert_eq!(
        gameplay_build_tag(&server, "canonical").as_deref(),
        Some(PACKAGED_PLUGIN_TEST_HARNESS_TAG)
    );
    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].phase, ConnectionPhase::Play);
    assert_eq!(
        sessions[0].protocol_generation,
        Some(before_protocol_generation)
    );
    assert_eq!(
        sessions[0].gameplay_generation,
        Some(before_gameplay_generation)
    );

    server.shutdown().await
}

#[tokio::test]
async fn full_reload_keeps_runtime_state_unchanged_when_topology_precommit_fails()
-> Result<(), RuntimeError> {
    let _guard = RELOAD_CORE_TEST_LOCK.lock().await;
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    let target_dir = PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .scoped_target_dir("full-reload-topology-precommit-fail");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut initial = core_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.topology.default_adapter = JE_5_ADAPTER_ID.into();
    initial.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into(), JE_47_ADAPTER_ID.into()]);
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID, JE_47_ADAPTER_ID])?,
    )
    .await?;
    let before_generation = server.runtime.active_generation_id();
    let before_protocol_generation = active_protocol_registry(&server)
        .resolve_adapter(JE_5_ADAPTER_ID)
        .and_then(|adapter| adapter.plugin_generation_id())
        .expect("je-5 should report a protocol generation");
    let before_gameplay_generation = loaded_plugins_snapshot(&server)
        .await
        .resolve_gameplay_profile("canonical")
        .expect("canonical gameplay profile should resolve")
        .plugin_generation_id()
        .expect("canonical gameplay profile should report generation");
    let codec = MinecraftWireCodec;
    let addr = listener_addr(&server);
    let (_stream, _buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "precommit-fail")
            .await?;

    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    std::thread::sleep(Duration::from_secs(1));
    harness
        .install_gameplay_plugin(
            "mc-plugin-gameplay-canonical",
            "gameplay-canonical",
            &dist_dir,
            &target_dir,
            "gameplay-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let mut updated = initial.clone();
    updated.topology.default_adapter = JE_47_ADAPTER_ID.into();
    updated.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into(), JE_47_ADAPTER_ID.into()]);
    write_server_toml(&config_path, &updated)?;

    server.runtime.topology.fail_next_precommit_for_test();
    let error = server
        .reload_runtime_full()
        .await
        .expect_err("topology precommit failure should abort full reload");
    assert!(
        error
            .to_string()
            .contains("injected topology precommit failure")
    );
    assert_eq!(server.runtime.active_generation_id(), before_generation);
    let sessions = server.session_status().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].phase, ConnectionPhase::Play);
    assert_eq!(sessions[0].generation_id, before_generation);
    assert_eq!(
        sessions[0].protocol_generation,
        Some(before_protocol_generation)
    );
    assert_eq!(
        sessions[0].gameplay_generation,
        Some(before_gameplay_generation)
    );
    assert_eq!(
        protocol_build_tag(&server, JE_5_ADAPTER_ID).as_deref(),
        Some(PACKAGED_PLUGIN_TEST_HARNESS_TAG)
    );
    assert_eq!(
        gameplay_build_tag(&server, "canonical").as_deref(),
        Some(PACKAGED_PLUGIN_TEST_HARNESS_TAG)
    );

    server.shutdown().await
}
