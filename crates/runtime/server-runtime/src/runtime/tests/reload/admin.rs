use super::*;

fn admin_reload_server_config(world_dir: PathBuf, dist_dir: PathBuf) -> ServerConfig {
    let mut config = loopback_server_config(world_dir);
    config.bootstrap.plugins_dir = dist_dir;
    config.plugins.allowlist = Some(plugin_allowlist_with_supporting_plugins(
        &[JE_1_7_10_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    ));
    config
}

#[tokio::test]
async fn admin_control_plane_reload_config_updates_ui_and_permissions_for_next_command()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;

    let mut initial = admin_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    initial.admin.local_console_permissions = vec![
        crate::runtime::AdminPermission::Status,
        crate::runtime::AdminPermission::ReloadConfig,
    ];
    write_server_toml(&config_path, &initial)?;

    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let control = server.admin_control_plane();
    let ui = server
        .admin_ui()
        .await
        .expect("console admin-ui should be active at boot");

    assert_eq!(
        ui.parse_line("reload config")
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
        crate::runtime::AdminRequest::ReloadConfig
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
    updated.admin.ui_profile = "missing-ui".to_string();
    updated.admin.local_console_permissions = vec![crate::runtime::AdminPermission::Status];
    write_server_toml(&config_path, &updated)?;

    let response = control
        .execute(
            crate::runtime::AdminPrincipal::LocalConsole,
            crate::runtime::AdminRequest::ReloadConfig,
        )
        .await;
    assert!(matches!(
        response,
        crate::runtime::AdminResponse::ReloadConfig(_)
    ));
    let rendered = ui
        .render_response(&response)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    assert!(rendered.contains("reload config"));
    assert!(server.admin_ui().await.is_none());

    assert!(matches!(
        control
            .execute(
                crate::runtime::AdminPrincipal::LocalConsole,
                crate::runtime::AdminRequest::ReloadPlugins,
            )
            .await,
        crate::runtime::AdminResponse::PermissionDenied {
            permission: crate::runtime::AdminPermission::ReloadPlugins,
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

    server.shutdown().await
}

#[tokio::test]
async fn admin_control_plane_reload_config_rejects_bootstrap_changes() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let config_path = temp_dir.path().join("server.toml");
    seed_runtime_plugins(
        &dist_dir,
        &[JE_1_7_10_ADAPTER_ID],
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;

    let initial = admin_reload_server_config(temp_dir.path().join("world"), dist_dir.clone());
    write_server_toml(&config_path, &initial)?;

    let server = build_reloadable_test_server_from_source(
        ServerConfigSource::Toml(config_path.clone()),
        plugin_test_registries_from_dist(dist_dir.clone(), &[JE_1_7_10_ADAPTER_ID])?,
    )
    .await?;
    let control = server.admin_control_plane();
    let ui = server
        .admin_ui()
        .await
        .expect("console admin-ui should be active at boot");

    let mut invalid = initial.clone();
    invalid.bootstrap.world_dir = temp_dir.path().join("other-world");
    write_server_toml(&config_path, &invalid)?;

    let response = control
        .execute(
            crate::runtime::AdminPrincipal::LocalConsole,
            crate::runtime::AdminRequest::ReloadConfig,
        )
        .await;
    let crate::runtime::AdminResponse::Error { message } = &response else {
        panic!("bootstrap diff should surface as admin reload error");
    };
    assert!(message.contains("bootstrap config changes require a restart"));
    assert!(server.admin_ui().await.is_some());
    assert!(
        ui.render_response(&response)
            .map_err(|error| RuntimeError::Config(error.to_string()))?
            .contains("error:")
    );

    server.shutdown().await
}
