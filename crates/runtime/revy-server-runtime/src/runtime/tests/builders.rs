use super::*;

#[tokio::test]
async fn storage_skip_keeps_dirty_state_after_runtime_save_failure() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.storage_profile = failing_storage_plugin::PROFILE_ID.into();
    config.plugins.failure_policy.storage = PluginFailureAction::Skip;
    let server = build_test_server(
        config,
        in_process_failing_storage_registries(PluginFailureAction::Skip)?,
    )
    .await?;

    server.runtime.kernel.set_dirty(true).await;
    server.runtime.maybe_save().await?;
    assert!(server.runtime.kernel.dirty().await);

    server.shutdown().await
}

#[tokio::test]
async fn plain_server_builder_rejects_reload_watch_without_reload_host() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.storage_profile = failing_storage_plugin::PROFILE_ID.into();
    config.plugins.reload_watch = true;
    let LoadedPluginTestEnvironment { loaded_plugins, .. } =
        in_process_failing_storage_registries(PluginFailureAction::Skip)?;
    let source = ServerConfigSource::Inline(config.clone());
    let error = match boot_server(source, config, loaded_plugins, None).await {
        Ok(_) => panic!("plain server builder should reject reload watch settings"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        RuntimeError::Config(message)
            if message.contains("plugins.reload_watch requires a reload-capable supervisor boot")
    ));
    Ok(())
}

#[tokio::test]
async fn reloadable_server_builder_applies_reload_host_failure_policy() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.storage_profile = failing_storage_plugin::PROFILE_ID.into();
    config.plugins.failure_policy.storage = PluginFailureAction::Skip;
    let LoadedPluginTestEnvironment { loaded_plugins, .. } =
        in_process_failing_storage_registries(PluginFailureAction::Skip)?;
    let LoadedPluginTestEnvironment {
        plugin_host: Some(reload_host),
        ..
    } = in_process_failing_storage_registries(PluginFailureAction::FailFast)?
    else {
        panic!("failing storage registries should include a plugin host");
    };
    let source = ServerConfigSource::Inline(config.clone());
    let server = boot_server(
        source,
        config,
        loaded_plugins,
        Some(reload_host.runtime_host()),
    )
    .await?;

    server.runtime.kernel.set_dirty(true).await;
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
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.storage_profile = failing_storage_plugin::PROFILE_ID.into();
    config.plugins.failure_policy.storage = PluginFailureAction::FailFast;
    let server = build_test_server(
        config,
        in_process_failing_storage_registries(PluginFailureAction::FailFast)?,
    )
    .await?;

    server.runtime.kernel.set_dirty(true).await;
    let error = server
        .runtime
        .maybe_save()
        .await
        .expect_err("fail-fast storage policy should return a fatal runtime error");
    assert!(matches!(
        error,
        RuntimeError::PluginFatal(message) if message.contains("storage plugin")
    ));
    server.runtime.kernel.set_dirty(false).await;

    server.shutdown().await
}
