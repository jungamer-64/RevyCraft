use super::*;
use mc_core::AdminSurfaceCapability;

#[test]
fn in_process_admin_surface_profile_declares_console_resources() -> Result<(), RuntimeError> {
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
        .admin_surface_raw(InProcessAdminSurfacePlugin {
            plugin_id: "admin-ui-console".to_string(),
            manifest: console_admin_surface_entrypoints().manifest,
            api: console_admin_surface_entrypoints().api,
        })
        .build();
    let runtime_selection = RuntimeSelectionConfig {
        admin_surfaces: vec![crate::config::AdminSurfaceSelectionConfig {
            instance_id: "console".to_string(),
            profile: "console-v1".into(),
            config_path: None,
        }],
        ..runtime_selection_config()
    };
    let _loaded = host.load_plugin_set(&runtime_selection)?;
    let profile = host
        .resolve_admin_surface_profile("console-v1")
        .expect("console admin-surface profile should resolve");
    let declaration = profile.declare_instance("console", None)?;

    assert!(
        profile
            .capability_set()
            .contains(&AdminSurfaceCapability::RuntimeReload)
    );
    assert!(declaration.principals.is_empty());
    assert_eq!(
        declaration.required_process_resources,
        vec!["stdio.stdin".to_string(), "stdio.stdout".to_string()]
    );
    assert!(!declaration.supports_upgrade_handoff);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn packaged_admin_surface_reload_swaps_generation_and_keeps_last_good() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let target_dir = harness.scoped_target_dir("plugin-host-admin-surface-reload");
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
    let runtime_selection = RuntimeSelectionConfig {
        admin_surfaces: vec![crate::config::AdminSurfaceSelectionConfig {
            instance_id: "console".to_string(),
            profile: "console-v1".into(),
            config_path: None,
        }],
        ..runtime_selection
    };
    let _loaded = host.load_plugin_set(&runtime_selection)?;
    let profile = host
        .resolve_admin_surface_profile("console-v1")
        .expect("console admin-surface profile should resolve");
    let first_generation = profile
        .plugin_generation_id()
        .expect("admin-surface profile should report plugin generation");

    assert_eq!(
        profile
            .declare_instance("console", None)?
            .required_process_resources,
        vec!["stdio.stdin".to_string(), "stdio.stdout".to_string()]
    );

    harness
        .install_admin_surface_plugin_for_reload(
            "mc-plugin-admin-ui-console",
            "admin-ui-console",
            &dist_dir,
            &target_dir,
            "admin-surface-reload-v2",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified_with_context(&protocol_reload_context(Vec::new()))?;
    assert_eq!(reloaded, vec!["admin-ui-console".to_string()]);
    let second_generation = profile
        .plugin_generation_id()
        .expect("reloaded admin-surface should report plugin generation");
    assert_ne!(first_generation, second_generation);
    assert_eq!(
        profile
            .declare_instance("console", None)?
            .required_process_resources,
        vec!["stdio.stdin".to_string(), "stdio.stdout".to_string()]
    );

    harness
        .install_admin_surface_plugin_for_reload(
            "mc-plugin-proto-je-5",
            "admin-ui-console",
            &dist_dir,
            &target_dir,
            "admin-surface-broken",
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let reloaded = host.reload_modified_with_context(&protocol_reload_context(Vec::new()))?;
    assert!(reloaded.is_empty());
    assert_eq!(profile.plugin_generation_id(), Some(second_generation));
    assert_eq!(
        profile
            .declare_instance("console", None)?
            .required_process_resources,
        vec!["stdio.stdin".to_string(), "stdio.stdout".to_string()]
    );

    Ok(())
}
