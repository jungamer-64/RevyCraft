use super::*;

#[tokio::test]
async fn storage_skip_keeps_dirty_state_after_runtime_save_failure() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let server = build_test_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            storage_profile: failing_storage_plugin::PROFILE_ID.to_string(),
            plugin_failure_policy_storage: PluginFailureAction::Skip,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        in_process_failing_storage_registries(PluginFailureAction::Skip)?,
    )
    .await?;

    {
        let mut state = server.runtime.state.lock().await;
        state.dirty = true;
    }
    server.runtime.maybe_save().await?;
    assert!(server.runtime.state.lock().await.dirty);

    server.shutdown().await
}

#[tokio::test]
async fn plain_server_builder_rejects_reload_watch_without_reload_host() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let config = ServerConfig {
        server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
        server_port: 0,
        plugin_reload_watch: true,
        world_dir: temp_dir.path().join("world"),
        ..ServerConfig::default()
    };
    let LoadedPluginTestEnvironment { loaded_plugins, .. } =
        in_process_failing_storage_registries(PluginFailureAction::Skip)?;
    let error = match ServerBuilder::new(ServerConfigSource::Inline(config), loaded_plugins)
        .build()
        .await
    {
        Ok(_) => panic!("plain server builder should reject reload watch settings"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        RuntimeError::Config(message)
            if message.contains("plugin-reload-watch requires ServerBuilder::with_reload_host")
    ));
    Ok(())
}

#[tokio::test]
async fn reloadable_server_builder_applies_reload_host_failure_policy() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let config = ServerConfig {
        server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
        server_port: 0,
        storage_profile: failing_storage_plugin::PROFILE_ID.to_string(),
        plugin_failure_policy_storage: PluginFailureAction::Skip,
        world_dir: temp_dir.path().join("world"),
        ..ServerConfig::default()
    };
    let LoadedPluginTestEnvironment { loaded_plugins, .. } =
        in_process_failing_storage_registries(PluginFailureAction::Skip)?;
    let LoadedPluginTestEnvironment {
        plugin_host: Some(reload_host),
        ..
    } = in_process_failing_storage_registries(PluginFailureAction::FailFast)?
    else {
        panic!("failing storage registries should include a plugin host");
    };
    let server = ServerBuilder::new(ServerConfigSource::Inline(config), loaded_plugins)
        .with_reload_host(reload_host.runtime_host())
        .build()
        .await?;

    {
        let mut state = server.runtime.state.lock().await;
        state.dirty = true;
    }
    let error = server
        .runtime
        .maybe_save()
        .await
        .expect_err("reload host should apply fail-fast failure policy");
    assert!(
        matches!(error, RuntimeError::PluginFatal(message) if message.contains("failed during runtime"))
    );

    let shutdown_error = server
        .shutdown()
        .await
        .expect_err("fail-fast runtime should surface plugin fatal during shutdown");
    assert!(
        matches!(shutdown_error, RuntimeError::PluginFatal(message) if message.contains("failed during runtime"))
    );
    Ok(())
}

#[tokio::test]
async fn storage_fail_fast_returns_plugin_fatal_on_runtime_save_failure() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let server = build_test_server(
        ServerConfig {
            server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
            server_port: 0,
            storage_profile: failing_storage_plugin::PROFILE_ID.to_string(),
            plugin_failure_policy_storage: PluginFailureAction::FailFast,
            world_dir: temp_dir.path().join("world"),
            ..ServerConfig::default()
        },
        in_process_failing_storage_registries(PluginFailureAction::FailFast)?,
    )
    .await?;

    {
        let mut state = server.runtime.state.lock().await;
        state.dirty = true;
    }
    let error = server
        .runtime
        .maybe_save()
        .await
        .expect_err("fail-fast storage policy should return a fatal runtime error");
    assert!(matches!(
        error,
        RuntimeError::PluginFatal(message) if message.contains("storage plugin")
    ));
    {
        let mut state = server.runtime.state.lock().await;
        state.dirty = false;
    }

    server.shutdown().await
}
