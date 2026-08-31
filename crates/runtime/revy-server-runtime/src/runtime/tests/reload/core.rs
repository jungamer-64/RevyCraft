use super::*;
use mc_proto_common::ConnectionPhase;

fn core_reload_server_config(world_dir: PathBuf, dist_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.game_mode = 1;
    config.bootstrap.plugins_dir = dist_dir;
    config
}

#[tokio::test]
async fn core_reload_preserves_live_java_session() -> Result<(), RuntimeError> {
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

    let mut updated = initial.clone();
    updated.bootstrap.level_name = "renamed-world".to_string();
    updated.bootstrap.view_distance = 4;
    updated.bootstrap.game_mode = 0;
    updated.bootstrap.difficulty = 3;
    updated.network.max_players = 31;
    updated.network.motd = "ignored-core-motd".to_string();
    updated.plugins.buffer_limits.protocol_response_bytes = 8192;
    updated.profiles.default_gameplay = "readonly".into();
    write_server_toml_for_reload(&config_path, &updated)?;

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
    assert_eq!(selection.config.network.motd, initial.network.motd);
    assert_eq!(
        selection
            .config
            .plugins
            .buffer_limits
            .protocol_response_bytes,
        initial.plugins.buffer_limits.protocol_response_bytes
    );
    assert_eq!(
        selection.config.profiles.default_gameplay,
        initial.profiles.default_gameplay
    );
    assert_eq!(selection.config.admin.surfaces, initial.admin.surfaces);

    server.shutdown().await
}
#[tokio::test]
async fn reload_paths_fail_fast_when_server_toml_disappears() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;

    let initial = core_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    write_server_toml(&config_path, &initial)?;
    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir, &[JE_5_ADAPTER_ID])?,
    )
    .await?;

    std::fs::remove_file(&config_path)?;
    let expected_path = config_path.display().to_string();

    let error = server
        .reload_runtime_full()
        .await
        .expect_err("manual reload should fail when server.toml is missing");
    assert!(matches!(
        &error,
        RuntimeError::Config(message)
            if message.contains("server config path")
                && message.contains(expected_path.as_str())
    ));

    let reload_host = server
        .runtime
        .reload
        .reload_host()
        .expect("reloadable test server should keep a reload host")
        .clone();
    let watch_error = server
        .runtime
        .maybe_reload_runtime_watch(reload_host.as_ref())
        .await
        .expect_err("watch reload should fail when server.toml is missing");
    assert!(matches!(
        &watch_error,
        RuntimeError::Config(message)
            if message.contains("server config path")
                && message.contains(expected_path.as_str())
    ));

    server.shutdown().await
}
#[tokio::test]
async fn full_reload_preserves_live_java_session() -> Result<(), RuntimeError> {
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

    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    harness
        .install_protocol_plugin_for_reload(
            "mc-plugin-proto-je-5-reload-test",
            JE_5_ADAPTER_ID,
            &dist_dir,
            &target_dir,
            "protocol-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    harness
        .install_gameplay_plugin_for_reload(
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
    let held_item = read_until_held_item_change(
        &mut alpha,
        &codec,
        &mut alpha_buffer,
        TestJavaProtocol::Je5,
        4,
        16,
    )
    .await?;
    assert_eq!(
        held_item_from_packet_for_protocol(TestJavaProtocol::Je5, &held_item)?,
        4
    );

    server.shutdown().await
}
