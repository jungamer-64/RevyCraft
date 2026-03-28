use super::*;

fn admin_reload_server_config(world_dir: PathBuf, dist_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.plugins_dir = dist_dir;
    config.plugins.allowlist = Some(plugin_allowlist_with_supporting_plugins(
        &[JE_5_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    ));
    config
}

fn remote_admin_principal(
    permissions: Vec<crate::config::AdminPermission>,
) -> crate::config::AdminPrincipalConfig {
    crate::config::AdminPrincipalConfig { permissions }
}

fn reload_request(mode: crate::runtime::RuntimeReloadMode) -> crate::runtime::AdminRequest {
    crate::runtime::AdminRequest::ReloadRuntime { mode }
}

#[tokio::test]
async fn admin_control_plane_reload_runtime_full_updates_ui_and_permissions_for_next_command()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;

    let mut initial = admin_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.admin.local_console_permissions = vec![
        crate::config::AdminPermission::Status,
        crate::config::AdminPermission::ReloadRuntime,
    ];
    initial.plugins.buffer_limits.protocol_response_bytes = 4096;
    write_server_toml(&config_path, &initial)?;

    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let control = server.admin_control_plane();
    assert_eq!(
        control
            .parse_local_command("reload runtime full")
            .await
            .map_err(RuntimeError::Config)?,
        reload_request(crate::runtime::RuntimeReloadMode::Full)
    );
    assert!(matches!(
        control
            .execute(
                crate::runtime::AdminPrincipal::LocalConsole,
                crate::runtime::AdminRequest::Shutdown,
            )
            .await,
        crate::runtime::AdminResponse::PermissionDenied {
            permission: crate::runtime::AdminPermission::Shutdown,
            ..
        }
    ));

    let mut updated = initial.clone();
    updated.admin.ui_profile = "missing-ui".into();
    updated.admin.local_console_permissions = vec![crate::config::AdminPermission::Status];
    updated.plugins.buffer_limits.protocol_response_bytes = 8192;
    write_server_toml_for_reload(&config_path, &updated)?;

    let response = control
        .execute(
            crate::runtime::AdminPrincipal::LocalConsole,
            reload_request(crate::runtime::RuntimeReloadMode::Full),
        )
        .await;
    assert!(matches!(
        response,
        crate::runtime::AdminResponse::ReloadRuntime(_)
    ));
    let rendered = control
        .render_local_response(&response)
        .await
        .map_err(RuntimeError::Config)?;
    assert!(rendered.contains("reload runtime full"));
    assert_eq!(
        control
            .parse_local_command("status")
            .await
            .map_err(RuntimeError::Config)?,
        crate::runtime::AdminRequest::Status
    );

    assert!(matches!(
        control
            .execute(
                crate::runtime::AdminPrincipal::LocalConsole,
                reload_request(crate::runtime::RuntimeReloadMode::Artifacts),
            )
            .await,
        crate::runtime::AdminResponse::PermissionDenied {
            permission: crate::runtime::AdminPermission::ReloadRuntime,
            ..
        }
    ));
    assert!(matches!(
        control
            .execute(
                crate::runtime::AdminPrincipal::LocalConsole,
                crate::runtime::AdminRequest::Status,
            )
            .await,
        crate::runtime::AdminResponse::Status(_)
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
        8192
    );

    server.shutdown().await
}

#[tokio::test]
async fn admin_control_plane_parse_reload_runtime_topology_uses_new_command_name()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], STORAGE_AND_AUTH_PLUGIN_IDS)?;

    let server = build_reloadable_test_server(
        admin_reload_server_config(temp_dir.path().join("world"), dist_dir.clone()),
        plugin_test_registries_from_dist(dist_dir, &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let control = server.admin_control_plane();

    assert_eq!(
        control
            .parse_local_command("reload runtime topology")
            .await
            .map_err(RuntimeError::Config)?,
        reload_request(crate::runtime::RuntimeReloadMode::Topology)
    );
    assert!(
        control
            .parse_local_command("reload topology")
            .await
            .is_err()
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
    initial.admin.local_console_permissions = vec![
        crate::config::AdminPermission::Status,
        crate::config::AdminPermission::ReloadRuntime,
    ];
    initial.plugins.buffer_limits.protocol_response_bytes = 4096;
    write_server_toml(&config_path, &initial)?;

    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir, &[JE_5_ADAPTER_ID])?,
    )
    .await?;
    let control = server.admin_control_plane();

    let mut updated = initial.clone();
    updated.admin.ui_profile = "missing-ui".into();
    updated.admin.local_console_permissions = vec![
        crate::config::AdminPermission::Status,
        crate::config::AdminPermission::ReloadRuntime,
    ];
    updated.plugins.buffer_limits.protocol_response_bytes = 8192;
    write_server_toml_for_reload(&config_path, &updated)?;

    assert!(matches!(
        control
            .execute(
                crate::runtime::AdminPrincipal::LocalConsole,
                reload_request(crate::runtime::RuntimeReloadMode::Artifacts),
            )
            .await,
        crate::runtime::AdminResponse::ReloadRuntime(_)
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
    assert_eq!(
        control
            .render_local_response(&crate::runtime::AdminResponse::ShutdownScheduled)
            .await
            .map_err(RuntimeError::Config)?,
        "shutdown scheduled"
    );

    let response = control
        .execute(
            crate::runtime::AdminPrincipal::LocalConsole,
            reload_request(crate::runtime::RuntimeReloadMode::Full),
        )
        .await;
    assert!(matches!(
        response,
        crate::runtime::AdminResponse::ReloadRuntime(_)
    ));
    assert_eq!(
        control
            .render_local_response(&crate::runtime::AdminResponse::ShutdownScheduled)
            .await
            .map_err(RuntimeError::Config)?,
        "shutdown: scheduled"
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
            .execute(
                crate::runtime::AdminPrincipal::LocalConsole,
                reload_request(crate::runtime::RuntimeReloadMode::Artifacts),
            )
            .await,
        crate::runtime::AdminResponse::ReloadRuntime(_)
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

    let mut invalid = initial.clone();
    invalid.bootstrap.world_dir = temp_dir.path().join("other-world");
    write_server_toml_for_reload(&config_path, &invalid)?;

    let response = control
        .execute(
            crate::runtime::AdminPrincipal::LocalConsole,
            reload_request(crate::runtime::RuntimeReloadMode::Full),
        )
        .await;
    let crate::runtime::AdminResponse::Error { message } = &response else {
        panic!("bootstrap diff should surface as admin reload error");
    };
    assert!(message.contains("bootstrap config changes require a restart"));
    assert!(
        control
            .render_local_response(&response)
            .await
            .map_err(RuntimeError::Config)?
            .contains("error:")
    );

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

    let response = control
        .execute(
            crate::runtime::AdminPrincipal::LocalConsole,
            reload_request(crate::runtime::RuntimeReloadMode::Full),
        )
        .await;
    assert!(matches!(
        response,
        crate::runtime::AdminResponse::ReloadRuntime(_)
    ));

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

    let response = control
        .execute(
            crate::runtime::AdminPrincipal::LocalConsole,
            reload_request(crate::runtime::RuntimeReloadMode::Full),
        )
        .await;
    assert!(matches!(
        response,
        crate::runtime::AdminResponse::ReloadRuntime(_)
    ));

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

    let ops_subject = control
        .subject_for_remote_principal("ops")
        .await
        .expect("ops principal should authenticate");
    assert!(control.status(&ops_subject).await.is_ok());

    let mut updated = initial.clone();
    updated.admin.principals.clear();
    write_server_toml_for_reload(&config_path, &updated)?;

    let response = control
        .execute(
            crate::runtime::AdminPrincipal::LocalConsole,
            reload_request(crate::runtime::RuntimeReloadMode::Full),
        )
        .await;
    assert!(matches!(
        response,
        crate::runtime::AdminResponse::ReloadRuntime(_)
    ));

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
