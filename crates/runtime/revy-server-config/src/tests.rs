use crate::*;
use revy_voxel_semantic::{AdapterId, AdminSurfaceProfileId, GameplayProfileId, StorageProfileId};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn configured_server_config() -> ServerConfig {
    let mut config = ServerConfig::default();
    config.bootstrap.level_name = "active-world".to_string();
    config.bootstrap.world_dir = PathBuf::from("runtime").join("active-world");
    config.bootstrap.plugins_dir = PathBuf::from("runtime").join("active-plugins");
    config.network.server_port = 25570;
    config.network.motd = "active-motd".to_string();
    config.network.max_players = 16;
    config.topology.default_adapter = AdapterId::new("je-47");
    config.topology.enabled_adapters = Some(vec![AdapterId::new("je-47")]);
    config.plugins.allowlist = Some(vec!["proto-initial".to_string()]);
    config.profiles.default_gameplay = GameplayProfileId::new("canonical");
    config.admin.surfaces.insert(
        "console".to_string(),
        AdminSurfaceConfig {
            profile: AdminSurfaceProfileId::new("console-v1"),
            config: None,
        },
    );
    config.admin.surfaces.insert(
        "remote".to_string(),
        AdminSurfaceConfig {
            profile: AdminSurfaceProfileId::new("grpc-v1"),
            config: Some(PathBuf::from("runtime").join("admin-grpc.toml")),
        },
    );
    config.admin.principals.insert(
        "ops".to_string(),
        AdminPrincipalConfig {
            permissions: vec![AdminPermission::Status],
        },
    );
    config
}

fn assert_config_error_contains(error: ServerConfigError, expected_fragment: &str) {
    match error {
        ServerConfigError::Config(message) => {
            assert!(
                message.contains(expected_fragment),
                "unexpected config error: {message}"
            );
        }
        other => panic!("unexpected config error: {other:?}"),
    }
}

#[test]
fn topology_reload_plan_updates_only_network_and_topology() -> Result<(), ServerConfigError> {
    let active = configured_server_config();
    let mut candidate = active.clone();
    candidate.network.server_port = 25571;
    candidate.network.motd = "candidate-motd".to_string();
    candidate.network.max_players = 24;
    candidate.topology.be_enabled = true;
    candidate.topology.default_adapter = AdapterId::new("je-5");
    candidate.topology.enabled_adapters = Some(vec![AdapterId::new("je-5")]);
    candidate.plugins.allowlist = Some(vec!["proto-candidate".to_string()]);
    candidate.profiles.default_gameplay = GameplayProfileId::new("readonly");
    candidate.admin.surfaces.insert(
        "console".to_string(),
        AdminSurfaceConfig {
            profile: AdminSurfaceProfileId::new("console-v2"),
            config: None,
        },
    );

    let plan = active.plan_topology_reload(&candidate)?;

    assert_eq!(plan.next_active_config.network, candidate.network);
    assert_eq!(plan.next_active_config.topology, candidate.topology);
    assert_eq!(plan.next_active_config.bootstrap, active.bootstrap);
    assert_eq!(plan.next_active_config.plugins, active.plugins);
    assert_eq!(plan.next_active_config.profiles, active.profiles);
    assert_eq!(plan.next_active_config.admin, active.admin);
    Ok(())
}

#[test]
fn topology_reload_plan_rejects_bootstrap_diff() {
    let active = configured_server_config();
    let mut candidate = active.clone();
    candidate.bootstrap.world_dir = PathBuf::from("runtime").join("other-world");

    let error = active
        .plan_topology_reload(&candidate)
        .expect_err("topology reload should reject bootstrap diffs");
    assert_config_error_contains(error, "bootstrap config changes require a restart");
}

#[test]
fn topology_reload_plan_ignores_admin_surface_diff() -> Result<(), ServerConfigError> {
    let active = configured_server_config();
    let mut candidate = active.clone();
    candidate.admin.surfaces.insert(
        "remote".to_string(),
        AdminSurfaceConfig {
            profile: AdminSurfaceProfileId::new("grpc-v1"),
            config: Some(PathBuf::from("runtime").join("other-grpc-admin-surface.toml")),
        },
    );
    candidate.admin.principals.insert(
        "backup".to_string(),
        AdminPrincipalConfig {
            permissions: vec![AdminPermission::Sessions],
        },
    );

    let plan = active.plan_topology_reload(&candidate)?;
    assert_eq!(plan.next_active_config.admin, active.admin);
    Ok(())
}

#[test]
fn core_reload_plan_updates_only_core_reloadable_fields() -> Result<(), ServerConfigError> {
    let active = configured_server_config();
    let mut candidate = active.clone();
    candidate.bootstrap.level_name = "candidate-world".to_string();
    candidate.bootstrap.game_mode = 1;
    candidate.bootstrap.difficulty = 3;
    candidate.bootstrap.view_distance = 5;
    candidate.network.max_players = 31;
    candidate.network.motd = "candidate-motd".to_string();
    candidate.plugins.allowlist = Some(vec!["proto-candidate".to_string()]);
    candidate.profiles.default_gameplay = GameplayProfileId::new("readonly");
    candidate.admin.surfaces.insert(
        "console".to_string(),
        AdminSurfaceConfig {
            profile: AdminSurfaceProfileId::new("console-v2"),
            config: None,
        },
    );

    let plan = active.plan_core_reload(&candidate)?;

    assert_eq!(
        plan.next_active_config.bootstrap.level_name,
        candidate.bootstrap.level_name
    );
    assert_eq!(
        plan.next_active_config.bootstrap.game_mode,
        candidate.bootstrap.game_mode
    );
    assert_eq!(
        plan.next_active_config.bootstrap.difficulty,
        candidate.bootstrap.difficulty
    );
    assert_eq!(
        plan.next_active_config.bootstrap.view_distance,
        candidate.bootstrap.view_distance
    );
    assert_eq!(
        plan.next_active_config.network.max_players,
        candidate.network.max_players
    );
    assert_eq!(plan.next_active_config.network.motd, active.network.motd);
    assert_eq!(plan.next_active_config.topology, active.topology);
    assert_eq!(plan.next_active_config.plugins, active.plugins);
    assert_eq!(plan.next_active_config.profiles, active.profiles);
    assert_eq!(plan.next_active_config.admin, active.admin);
    Ok(())
}

#[test]
fn core_reload_plan_rejects_restart_required_bootstrap_diff() {
    let active = configured_server_config();
    let mut candidate = active.clone();
    candidate.bootstrap.plugins_dir = PathBuf::from("runtime").join("other-plugins");

    let error = active
        .plan_core_reload(&candidate)
        .expect_err("core reload should reject plugins_dir diffs");
    assert_config_error_contains(error, "bootstrap config changes require a restart");
}

#[test]
fn core_reload_plan_ignores_admin_surface_diff() -> Result<(), ServerConfigError> {
    let active = configured_server_config();
    let mut candidate = active.clone();
    candidate.admin.surfaces.insert(
        "remote".to_string(),
        AdminSurfaceConfig {
            profile: AdminSurfaceProfileId::new("grpc-v2"),
            config: Some(PathBuf::from("runtime").join("admin-grpc.toml")),
        },
    );
    candidate.admin.principals.insert(
        "backup".to_string(),
        AdminPrincipalConfig {
            permissions: vec![AdminPermission::Sessions],
        },
    );

    let plan = active.plan_core_reload(&candidate)?;
    assert_eq!(plan.next_active_config.admin, active.admin);
    Ok(())
}

#[test]
fn full_reload_plan_adopts_candidate_config() -> Result<(), ServerConfigError> {
    let active = configured_server_config();
    let mut candidate = active.clone();
    candidate.bootstrap.level_name = "candidate-world".to_string();
    candidate.bootstrap.game_mode = 1;
    candidate.bootstrap.difficulty = 3;
    candidate.bootstrap.view_distance = 5;
    candidate.network.max_players = 31;
    candidate.network.motd = "candidate-motd".to_string();
    candidate.plugins.allowlist = Some(vec!["proto-candidate".to_string()]);
    candidate.profiles.default_gameplay = GameplayProfileId::new("readonly");
    candidate.admin.surfaces.insert(
        "console".to_string(),
        AdminSurfaceConfig {
            profile: AdminSurfaceProfileId::new("console-v2"),
            config: None,
        },
    );

    let plan = active.plan_full_reload(&candidate)?;

    assert_eq!(plan.next_active_config, candidate);
    Ok(())
}

#[test]
fn validate_rejects_plugin_abi_range_when_min_exceeds_max() {
    let mut config = configured_server_config();
    config.bootstrap.plugin_abi_min = mc_plugin_api::abi::PluginAbiVersion { major: 5, minor: 1 };
    config.bootstrap.plugin_abi_max = mc_plugin_api::abi::PluginAbiVersion { major: 5, minor: 0 };

    let error = config
        .validate_owned()
        .expect_err("plugin ABI range should reject min > max");
    assert_config_error_contains(error, "plugin_abi_min");
}

#[test]
fn validate_rejects_plugin_abi_range_when_current_host_abi_is_excluded() {
    let mut config = configured_server_config();
    config.bootstrap.plugin_abi_min = mc_plugin_api::abi::PluginAbiVersion { major: 4, minor: 0 };
    config.bootstrap.plugin_abi_max = mc_plugin_api::abi::PluginAbiVersion { major: 4, minor: 9 };

    let error = config
        .validate_owned()
        .expect_err("plugin ABI range should include current host ABI");
    assert_config_error_contains(error, "does not include current host ABI");
}

#[test]
fn validate_rejects_duplicate_enabled_adapters() {
    let mut config = configured_server_config();
    config.topology.enabled_adapters = Some(vec![AdapterId::new("je-47"), AdapterId::new("je-47")]);

    let error = config
        .validate_owned()
        .expect_err("enabled adapters should reject duplicates");
    assert_config_error_contains(error, "duplicate adapter");
}

#[test]
fn validate_rejects_enabled_adapters_without_default() {
    let mut config = configured_server_config();
    config.topology.enabled_adapters = Some(vec![AdapterId::new("je-5")]);

    let error = config
        .validate_owned()
        .expect_err("enabled adapters should include the default adapter");
    assert_config_error_contains(error, "default-adapter");
}

#[test]
fn full_reload_plan_rejects_restart_required_static_diff() {
    let active = configured_server_config();
    let mut candidate = active.clone();
    candidate.bootstrap.storage_profile = StorageProfileId::new("other-storage");

    let error = active
        .plan_full_reload(&candidate)
        .expect_err("full reload should reject storage profile diffs");
    assert_config_error_contains(error, "bootstrap config changes require a restart");
}

#[test]
fn plugin_host_bootstrap_view_exposes_bootstrap_selection_fields() {
    let mut config = configured_server_config();
    config.bootstrap.plugin_abi_min = mc_plugin_api::abi::PluginAbiVersion { major: 3, minor: 0 };
    config.bootstrap.plugin_abi_max = mc_plugin_api::abi::PluginAbiVersion { major: 3, minor: 1 };

    let view = config.plugin_host_bootstrap_view();

    assert_eq!(view.storage_profile, config.bootstrap.storage_profile);
    assert_eq!(view.plugins_dir, config.bootstrap.plugins_dir);
    assert_eq!(
        view.plugin_abi_min.major,
        config.bootstrap.plugin_abi_min.major
    );
    assert_eq!(
        view.plugin_abi_min.minor,
        config.bootstrap.plugin_abi_min.minor
    );
    assert_eq!(
        view.plugin_abi_max.major,
        config.bootstrap.plugin_abi_max.major
    );
    assert_eq!(
        view.plugin_abi_max.minor,
        config.bootstrap.plugin_abi_max.minor
    );
}

#[test]
fn plugin_host_runtime_selection_view_exposes_runtime_selection_fields() {
    let mut config = configured_server_config();
    config.topology.be_enabled = true;
    config.plugins.buffer_limits.protocol_response_bytes = 1234;
    config.plugins.buffer_limits.metadata_bytes = 7890;
    config.plugins.failure_policy.protocol = PluginFailureAction::Skip;
    config.plugins.failure_policy.gameplay = PluginFailureAction::FailFast;

    let view = config.plugin_host_runtime_selection_view();
    let mut instance_ids = view
        .admin_surfaces
        .iter()
        .map(|surface| surface.instance_id.clone())
        .collect::<Vec<_>>();
    instance_ids.sort();

    assert!(view.be_enabled);
    assert_eq!(view.auth_profile, config.profiles.auth);
    assert_eq!(view.bedrock_auth_profile, config.profiles.bedrock_auth);
    assert_eq!(
        view.default_gameplay_profile,
        config.profiles.default_gameplay
    );
    assert_eq!(view.gameplay_profile_map, config.profiles.gameplay_map);
    assert_eq!(view.plugin_allowlist, config.plugins.allowlist);
    assert_eq!(view.buffer_limits.protocol_response_bytes, 1234);
    assert_eq!(view.buffer_limits.metadata_bytes, 7890);
    assert_eq!(view.failure_matrix, config.plugins.failure_policy);
    assert_eq!(
        instance_ids,
        vec!["console".to_string(), "remote".to_string()]
    );
}

#[test]
fn from_toml_rejects_missing_path_with_selected_path() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "revy-missing-server-config-{}-{nonce}.toml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    let error = ServerConfig::from_toml(&path).expect_err("missing server.toml should fail fast");
    let expected_path = path.display().to_string();

    match error {
        ServerConfigError::Config(message) => {
            assert!(message.contains("server config path"));
            assert!(message.contains(expected_path.as_str()));
        }
        other => panic!("unexpected config error: {other:?}"),
    }
}

#[test]
fn from_toml_parses_admin_surfaces_and_disables_legacy_surface_slots()
-> Result<(), ServerConfigError> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "revy-admin-surfaces-config-{}-{nonce}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("admin"))?;
    let surface_config = root.join("admin").join("grpc.toml");
    std::fs::write(&surface_config, "bind_addr = \"127.0.0.1:50051\"\n")?;
    let server_path = root.join("server.toml");
    std::fs::write(
        &server_path,
        r#"
[live.admin.surfaces.console]
profile = "console-v1"

[live.admin.surfaces.remote]
profile = "grpc-v1"
config = "admin/grpc.toml"
"#,
    )?;

    let parsed = ServerConfig::from_toml(&server_path)?;
    assert_eq!(
        parsed.admin.surfaces.get("console"),
        Some(&AdminSurfaceConfig {
            profile: AdminSurfaceProfileId::new("console-v1"),
            config: None,
        })
    );
    assert_eq!(
        parsed.admin.surfaces.get("remote"),
        Some(&AdminSurfaceConfig {
            profile: AdminSurfaceProfileId::new("grpc-v1"),
            config: Some(surface_config.clone()),
        })
    );

    assert_eq!(parsed.admin.surfaces.len(), 2);
    Ok(())
}

#[test]
fn from_toml_rejects_legacy_admin_keys() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "revy-admin-surface-legacy-{}-{nonce}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root should be created");
    let server_path = root.join("server.toml");
    std::fs::write(
        &server_path,
        r#"
[static.admin.remote]
transport_profile = "grpc-v1"

[live.admin]
ui_profile = "console-v1"
local_console_permissions = ["status"]
"#,
    )
    .expect("server config should be written");

    let error = ServerConfig::from_toml(&server_path).expect_err("legacy admin keys should fail");
    assert_config_error_contains(error, "unknown field");
}

#[test]
fn from_toml_rejects_missing_admin_surface_config_file() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "revy-admin-surface-config-missing-{}-{nonce}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root should be created");
    let server_path = root.join("server.toml");
    std::fs::write(
        &server_path,
        r#"
[live.admin.surfaces.remote]
profile = "grpc-v1"
config = "missing-grpc.toml"
"#,
    )
    .expect("server config should be written");

    let error = ServerConfig::from_toml(&server_path)
        .expect_err("missing admin surface config should be rejected");
    assert_config_error_contains(error, "live.admin.surfaces.remote.config");
}
