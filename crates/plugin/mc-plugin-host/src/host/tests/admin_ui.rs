use super::*;
use mc_plugin_api::codec::admin_ui::{AdminPermission, AdminPrincipal};

#[test]
fn in_process_admin_ui_profile_parses_and_renders() -> Result<(), RuntimeError> {
    let host = TestPluginHostBuilder::new()
        .gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-canonical".to_string(),
            manifest: canonical_gameplay_entrypoints().manifest,
            api: canonical_gameplay_entrypoints().api,
        })
        .gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-readonly".to_string(),
            manifest: readonly_gameplay_entrypoints().manifest,
            api: readonly_gameplay_entrypoints().api,
        })
        .storage_raw(InProcessStoragePlugin {
            plugin_id: "storage-je-anvil-1_7_10".to_string(),
            manifest: storage_entrypoints().manifest,
            api: storage_entrypoints().api,
        })
        .auth_raw(InProcessAuthPlugin {
            plugin_id: "auth-offline".to_string(),
            manifest: offline_auth_entrypoints().manifest,
            api: offline_auth_entrypoints().api,
        })
        .admin_ui_raw(InProcessAdminUiPlugin {
            plugin_id: "admin-ui-console".to_string(),
            manifest: console_admin_ui_entrypoints().manifest,
            api: console_admin_ui_entrypoints().api,
        })
        .build();
    let _loaded = host.load_plugin_set(&runtime_selection_config())?;
    let profile = host
        .resolve_admin_ui_profile("console-v1")
        .expect("console admin-ui profile should resolve");

    assert_eq!(
        profile.parse_line("reload runtime artifacts")?,
        AdminRequest::ReloadRuntime {
            mode: mc_plugin_api::codec::admin_ui::RuntimeReloadMode::Artifacts,
        }
    );
    assert!(
        profile
            .render_response(&AdminResponse::PermissionDenied {
                principal: AdminPrincipal::LocalConsole,
                permission: AdminPermission::Shutdown,
            })?
            .contains("permission denied")
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn packaged_admin_ui_reload_swaps_generation_and_keeps_last_good() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("plugin-host-admin-ui-reload");
    seed_packaged_plugins(
        &dist_dir,
        &[
            "admin-ui-console",
            "gameplay-canonical",
            "gameplay-readonly",
            "storage-je-anvil-1_7_10",
            "auth-offline",
        ],
    )?;

    let bootstrap = bootstrap_config_with_plugins_dir(dist_dir.clone());
    let runtime_selection = RuntimeSelectionConfig {
        plugin_allowlist: Some(vec![
            "admin-ui-console".to_string(),
            "gameplay-canonical".to_string(),
            "gameplay-readonly".to_string(),
            "storage-je-anvil-1_7_10".to_string(),
            "auth-offline".to_string(),
        ]),
        ..runtime_selection_config()
    };
    let host =
        TestPluginHost::discover(&bootstrap)?.expect("packaged plugins should be discovered");
    let _loaded = host.load_plugin_set(&runtime_selection)?;
    let profile = host
        .resolve_admin_ui_profile("console-v1")
        .expect("console admin-ui profile should resolve");
    let first_generation = profile
        .plugin_generation_id()
        .expect("admin-ui profile should report plugin generation");

    assert_eq!(profile.parse_line("status")?, AdminRequest::Status);

    harness
        .install_admin_ui_plugin_for_reload(
            "mc-plugin-admin-ui-console",
            "admin-ui-console",
            &dist_dir,
            &target_dir,
            "admin-ui-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified_with_context(&protocol_reload_context(Vec::new()))?;
    assert_eq!(reloaded, vec!["admin-ui-console".to_string()]);
    let second_generation = profile
        .plugin_generation_id()
        .expect("reloaded admin-ui should report plugin generation");
    assert_ne!(first_generation, second_generation);
    assert_eq!(profile.parse_line("sessions")?, AdminRequest::Sessions);

    harness
        .install_admin_ui_plugin_for_reload(
            "mc-plugin-proto-je-5",
            "admin-ui-console",
            &dist_dir,
            &target_dir,
            "admin-ui-broken",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified_with_context(&protocol_reload_context(Vec::new()))?;
    assert!(reloaded.is_empty());
    assert_eq!(profile.plugin_generation_id(), Some(second_generation));
    assert_eq!(profile.parse_line("help")?, AdminRequest::Help);

    Ok(())
}
