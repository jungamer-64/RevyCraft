use super::*;

fn admin_reload_server_config(world_dir: PathBuf, dist_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.plugins_dir = dist_dir;
    config.plugins.allowlist = Some(plugin_allowlist_with_supporting_plugins(
        &[JE_5_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    ));
    set_console_surface(&mut config, "console-v1");
    set_console_permissions(
        &mut config,
        vec![
            crate::config::AdminPermission::Status,
            crate::config::AdminPermission::Sessions,
            crate::config::AdminPermission::ReloadRuntime,
            crate::config::AdminPermission::UpgradeRuntime,
            crate::config::AdminPermission::Shutdown,
        ],
    );
    config
}

fn remote_admin_principal(
    permissions: Vec<crate::config::AdminPermission>,
) -> crate::config::AdminPrincipalConfig {
    crate::config::AdminPrincipalConfig { permissions }
}

async fn console_subject(
    control: &crate::runtime::AdminControlPlaneHandle,
) -> Result<crate::runtime::AdminSubject, RuntimeError> {
    control
        .subject_for_remote_principal(CONSOLE_PRINCIPAL_ID)
        .await
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

#[tokio::test]
async fn admin_control_plane_reload_runtime_full_updates_console_permissions_for_next_command()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;

    let mut initial = admin_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    set_console_permissions(
        &mut initial,
        vec![
            crate::config::AdminPermission::Status,
            crate::config::AdminPermission::ReloadRuntime,
        ],
    );
    initial.plugins.buffer_limits.protocol_response_bytes = 4096;
    write_server_toml(&config_path, &initial)?;

    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let control = server.admin_control_plane();
    let console = console_subject(&control).await?;
    assert!(matches!(
        control.shutdown(&console).await,
        Err(crate::runtime::AdminCommandError::PermissionDenied {
            permission: crate::runtime::AdminPermission::Shutdown,
            ..
        })
    ));

    let mut updated = initial.clone();
    set_console_permissions(&mut updated, vec![crate::config::AdminPermission::Status]);
    updated.plugins.buffer_limits.protocol_response_bytes = 8192;
    write_server_toml_for_reload(&config_path, &updated)?;

    assert!(
        control
            .reload_runtime(&console, crate::runtime::RuntimeReloadMode::Full)
            .await
            .is_ok()
    );

    assert!(matches!(
        control
            .reload_runtime(&console, crate::runtime::RuntimeReloadMode::Artifacts)
            .await,
        Err(crate::runtime::AdminCommandError::PermissionDenied {
            permission: crate::runtime::AdminPermission::ReloadRuntime,
            ..
        })
    ));
    assert!(matches!(control.status(&console).await, Ok(_)));
    assert_eq!(
        server
            .runtime
            .selection_state()
            .await
            .config
            .plugins
            .buffer_limits
            .protocol_response_bytes,
        8192
    );

    server.shutdown().await
}

#[tokio::test]
async fn admin_control_plane_reload_runtime_artifacts_ignores_pending_config_changes()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;

    let mut initial = admin_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    set_console_permissions(
        &mut initial,
        vec![
            crate::config::AdminPermission::Status,
            crate::config::AdminPermission::ReloadRuntime,
        ],
    );
    initial.plugins.buffer_limits.protocol_response_bytes = 4096;
    write_server_toml(&config_path, &initial)?;

    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir, &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let control = server.admin_control_plane();
    let console = console_subject(&control).await?;

    let mut updated = initial.clone();
    set_console_permissions(
        &mut updated,
        vec![
            crate::config::AdminPermission::Status,
            crate::config::AdminPermission::ReloadRuntime,
        ],
    );
    updated.plugins.buffer_limits.protocol_response_bytes = 8192;
    write_server_toml_for_reload(&config_path, &updated)?;

    assert!(matches!(
        control
            .reload_runtime(&console, crate::runtime::RuntimeReloadMode::Artifacts)
            .await,
        Ok(_)
    ));
    assert_eq!(
        server
            .runtime
            .selection_state()
            .await
            .config
            .plugins
            .buffer_limits
            .protocol_response_bytes,
        4096
    );
    assert!(
        control
            .reload_runtime(&console, crate::runtime::RuntimeReloadMode::Full)
            .await
            .is_ok()
    );
    assert_eq!(
        server
            .runtime
            .selection_state()
            .await
            .config
            .plugins
            .buffer_limits
            .protocol_response_bytes,
        8192
    );
    assert!(matches!(
        control
            .reload_runtime(&console, crate::runtime::RuntimeReloadMode::Artifacts)
            .await,
        Ok(_)
    ));

    server.shutdown().await
}

#[tokio::test]
async fn admin_control_plane_reload_runtime_full_rejects_bootstrap_changes()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;

    let initial = admin_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    write_server_toml(&config_path, &initial)?;

    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let control = server.admin_control_plane();
    let console = console_subject(&control).await?;

    let mut invalid = initial.clone();
    invalid.bootstrap.world_dir = temp_dir.path().join("other-world");
    write_server_toml_for_reload(&config_path, &invalid)?;

    let error = control
        .reload_runtime(&console, crate::runtime::RuntimeReloadMode::Full)
        .await
        .expect_err("bootstrap diff should surface as admin reload error");
    assert!(matches!(
        error,
        crate::runtime::AdminCommandError::Runtime(RuntimeError::Config(message))
            if message.contains("bootstrap config changes require a restart")
    ));

    server.shutdown().await
}

#[tokio::test]
async fn admin_control_plane_reload_runtime_full_updates_remote_principals_for_next_request()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;

    let mut initial = admin_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.admin.principals.insert(
        "ops".to_string(),
        remote_admin_principal(vec![crate::config::AdminPermission::Status]),
    );
    write_server_toml(&config_path, &initial)?;

    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let control = server.admin_control_plane();
    let console = console_subject(&control).await?;

    let ops_subject = control
        .subject_for_remote_principal("ops")
        .await
        .expect("ops principal should authenticate");
    assert!(control.status(&ops_subject).await.is_ok());
    assert!(matches!(
        control
            .reload_runtime(&ops_subject, crate::runtime::RuntimeReloadMode::Artifacts)
            .await,
        Err(crate::runtime::AdminCommandError::PermissionDenied {
            permission: crate::runtime::AdminPermission::ReloadRuntime,
            ..
        })
    ));

    let mut updated = initial.clone();
    updated.admin.principals.clear();
    updated.admin.principals.insert(
        "backup".to_string(),
        remote_admin_principal(vec![
            crate::config::AdminPermission::Status,
            crate::config::AdminPermission::ReloadRuntime,
        ]),
    );
    write_server_toml_for_reload(&config_path, &updated)?;

    assert!(
        control
            .reload_runtime(&console, crate::runtime::RuntimeReloadMode::Full)
            .await
            .is_ok()
    );

    assert!(matches!(
        control.subject_for_remote_principal("ops").await,
        Err(crate::runtime::AdminAuthError::InvalidPrincipalId)
    ));
    assert!(matches!(
        control.status(&ops_subject).await,
        Err(crate::runtime::AdminCommandError::InvalidSubject { .. })
    ));

    let backup_subject = control
        .subject_for_remote_principal("backup")
        .await
        .expect("backup principal should authenticate after reload");
    assert_eq!(backup_subject.principal_id(), "backup");
    assert!(
        control
            .reload_runtime(
                &backup_subject,
                crate::runtime::RuntimeReloadMode::Artifacts
            )
            .await
            .is_ok()
    );

    server.shutdown().await
}

#[tokio::test]
async fn admin_control_plane_reload_runtime_full_updates_remote_permissions_for_existing_subject()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;

    let mut initial = admin_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.admin.principals.insert(
        "ops".to_string(),
        remote_admin_principal(vec![
            crate::config::AdminPermission::Status,
            crate::config::AdminPermission::ReloadRuntime,
        ]),
    );
    write_server_toml(&config_path, &initial)?;

    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let control = server.admin_control_plane();
    let console = console_subject(&control).await?;

    let subject = control
        .subject_for_remote_principal("ops")
        .await
        .expect("ops principal should authenticate");
    assert!(
        control
            .reload_runtime(&subject, crate::runtime::RuntimeReloadMode::Artifacts)
            .await
            .is_ok()
    );

    let mut updated = initial.clone();
    updated.admin.principals.insert(
        "ops".to_string(),
        remote_admin_principal(vec![crate::config::AdminPermission::Status]),
    );
    write_server_toml_for_reload(&config_path, &updated)?;

    assert!(
        control
            .reload_runtime(&console, crate::runtime::RuntimeReloadMode::Full)
            .await
            .is_ok()
    );

    assert!(control.status(&subject).await.is_ok());
    assert!(matches!(
        control
            .reload_runtime(&subject, crate::runtime::RuntimeReloadMode::Artifacts)
            .await,
        Err(crate::runtime::AdminCommandError::PermissionDenied {
            permission: crate::runtime::AdminPermission::ReloadRuntime,
            ..
        })
    ));

    server.shutdown().await
}

#[tokio::test]
async fn admin_control_plane_reload_runtime_full_invalidates_existing_subject_when_principal_is_removed()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;

    let mut initial = admin_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.admin.principals.insert(
        "ops".to_string(),
        remote_admin_principal(vec![crate::config::AdminPermission::Status]),
    );
    write_server_toml(&config_path, &initial)?;

    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let control = server.admin_control_plane();
    let console = console_subject(&control).await?;

    let ops_subject = control
        .subject_for_remote_principal("ops")
        .await
        .expect("ops principal should authenticate");
    assert!(control.status(&ops_subject).await.is_ok());

    let mut updated = initial.clone();
    updated.admin.principals.clear();
    write_server_toml_for_reload(&config_path, &updated)?;

    assert!(
        control
            .reload_runtime(&console, crate::runtime::RuntimeReloadMode::Full)
            .await
            .is_ok()
    );

    assert!(matches!(
        control.subject_for_remote_principal("ops").await,
        Err(crate::runtime::AdminAuthError::InvalidPrincipalId)
    ));
    assert!(matches!(
        control.status(&ops_subject).await,
        Err(crate::runtime::AdminCommandError::InvalidSubject { .. })
    ));

    server.shutdown().await
}
