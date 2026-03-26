use super::*;

fn assert_server_config_error_contains(
    error: crate::config::ServerConfigError,
    expected_fragment: &str,
) {
    match error {
        crate::config::ServerConfigError::Config(message) => {
            assert!(
                message.contains(expected_fragment),
                "unexpected config error: {message}"
            );
        }
        crate::config::ServerConfigError::PluginHost(mc_plugin_host::PluginHostError::Config(
            message,
        )) => {
            assert!(
                message.contains(expected_fragment),
                "unexpected plugin-host config error: {message}"
            );
        }
        other => panic!("unexpected config error: {other:?}"),
    }
}

fn tracked_runtime_config_path(file_name: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for ancestor in manifest_dir.ancestors() {
        let manifest = ancestor.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&manifest) else {
            continue;
        };
        if contents.contains("[workspace]") {
            return ancestor.join("runtime").join(file_name);
        }
    }
    panic!(
        "server-runtime tests should run under the workspace root: {}",
        manifest_dir.display()
    );
}

async fn assert_spawn_fails_with_message(
    mut config: ServerConfig,
    expected_fragment: &str,
) -> Result<(), RuntimeError> {
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    config.bootstrap.plugins_dir = harness.dist_dir().to_path_buf();
    if config.plugins.allowlist.is_none() {
        config.plugins.allowlist = Some(plugin_allowlist_with_supporting_plugins(
            ALL_PROTOCOL_PLUGIN_IDS,
            STORAGE_AND_AUTH_PLUGIN_IDS,
        ));
    }
    let result = build_test_server(config, plugin_test_registries_all()?).await;
    let Err(error) = result else {
        panic!("build_test_server should have failed");
    };
    assert!(
        matches!(error, RuntimeError::Config(ref message) if message.contains(expected_fragment)),
        "unexpected error: {error:?}"
    );
    Ok(())
}

#[test]
fn server_toml_rejects_legacy_grouped_schema() {
    let temp_dir = tempdir().expect("tempdir should be available");
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[bootstrap]
level_name = "legacy"
"#,
    )
    .expect("server.toml should write");

    let error = ServerConfig::from_toml(&path).expect_err("legacy grouped schema should fail");
    assert!(matches!(
        error,
        crate::config::ServerConfigError::Config(message)
            if message.contains("unknown field `bootstrap`")
                || message.contains("expected `static` or `live`")
    ));
}

#[test]
fn server_toml_accepts_static_live_schema_and_resolves_relative_paths() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[static.bootstrap]
online_mode = false
level_name = "flatland"
level_type = "FLAT"

[live.network]
server_ip = "127.0.0.1"
server_port = 0

[live.topology]
be_enabled = true
default_adapter = "je-5"

[live.plugins]

[live.plugins.failure_policy]

[live.profiles]
auth = "offline-v1"
"#,
    )?;

    let config = ServerConfig::from_toml(&path)?;

    assert_eq!(config.bootstrap.level_name, "flatland");
    assert_eq!(config.bootstrap.level_type, LevelType::Flat);
    assert!(config.topology.be_enabled);
    assert_eq!(config.topology.default_adapter, JE_5_ADAPTER_ID);
    assert_eq!(config.topology.default_bedrock_adapter, BE_924_ADAPTER_ID);
    assert_eq!(
        config.bootstrap.storage_profile,
        JE_1_7_10_STORAGE_PROFILE_ID
    );
    assert_eq!(config.profiles.auth, OFFLINE_AUTH_PROFILE_ID);
    assert_eq!(
        config.profiles.bedrock_auth,
        BEDROCK_OFFLINE_AUTH_PROFILE_ID
    );
    assert_eq!(config.bootstrap.world_dir, temp_dir.path().join("flatland"));
    assert_eq!(
        config.bootstrap.plugins_dir,
        temp_dir.path().join("plugins")
    );
    Ok(())
}

#[test]
fn tracked_runtime_server_toml_parses() -> Result<(), RuntimeError> {
    let config = ServerConfig::from_toml(&tracked_runtime_config_path("server.toml"))?;
    assert!(config.topology.be_enabled);
    assert!(
        config
            .admin
            .local_console_permissions
            .contains(&crate::config::AdminPermission::ReloadRuntime)
    );
    Ok(())
}

#[test]
fn tracked_runtime_server_toml_example_parses() -> Result<(), RuntimeError> {
    let config = ServerConfig::from_toml(&tracked_runtime_config_path("server.toml.example"))?;
    assert!(config.topology.be_enabled);
    assert!(
        config
            .admin
            .local_console_permissions
            .contains(&crate::config::AdminPermission::ReloadRuntime)
    );
    Ok(())
}

#[test]
fn default_config_uses_relative_runtime_paths() {
    let config = ServerConfig::default();
    assert_eq!(
        config.bootstrap.plugins_dir,
        PathBuf::from("runtime").join("plugins")
    );
    assert_eq!(
        config.bootstrap.world_dir,
        PathBuf::from("runtime").join("world")
    );
}

#[test]
fn server_toml_parse_enabled_adapters() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = ServerConfig::default();
    config.topology.enabled_adapters = Some(vec![
        JE_5_ADAPTER_ID.into(),
        JE_47_ADAPTER_ID.into(),
        JE_340_ADAPTER_ID.into(),
    ]);
    let path = temp_dir.path().join("server.toml");
    write_server_toml(&path, &config)?;

    let parsed = ServerConfig::from_toml(&path)?;
    assert_eq!(
        parsed.topology.enabled_adapters,
        config.topology.enabled_adapters
    );
    Ok(())
}

#[test]
fn server_toml_parse_bedrock_adapter_and_auth_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = ServerConfig::default();
    config.topology.be_enabled = true;
    config.topology.default_bedrock_adapter = BE_924_ADAPTER_ID.into();
    config.topology.enabled_bedrock_adapters = Some(vec![
        BE_924_ADAPTER_ID.into(),
        BE_PLACEHOLDER_ADAPTER_ID.into(),
    ]);
    config.profiles.bedrock_auth = "bedrock-xbl-v1".into();
    let path = temp_dir.path().join("server.toml");
    write_server_toml(&path, &config)?;

    let parsed = ServerConfig::from_toml(&path)?;
    assert!(parsed.topology.be_enabled);
    assert_eq!(parsed.topology.default_bedrock_adapter, BE_924_ADAPTER_ID);
    assert_eq!(
        parsed.topology.enabled_bedrock_adapters,
        Some(vec![
            BE_924_ADAPTER_ID.into(),
            BE_PLACEHOLDER_ADAPTER_ID.into(),
        ])
    );
    assert_eq!(parsed.profiles.bedrock_auth, "bedrock-xbl-v1");
    Ok(())
}

#[test]
fn server_toml_parse_gameplay_profile_configuration() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = ServerConfig::default();
    config.profiles.default_gameplay = "canonical".into();
    config.profiles.gameplay_map = gameplay_profile_map(&[
        (JE_5_ADAPTER_ID, "readonly"),
        (JE_340_ADAPTER_ID, "canonical"),
    ]);
    let path = temp_dir.path().join("server.toml");
    write_server_toml(&path, &config)?;

    let parsed = ServerConfig::from_toml(&path)?;
    assert_eq!(parsed.profiles.default_gameplay, "canonical");
    assert_eq!(
        parsed.profiles.gameplay_map,
        gameplay_profile_map(&[
            (JE_5_ADAPTER_ID, "readonly"),
            (JE_340_ADAPTER_ID, "canonical"),
        ])
    );
    Ok(())
}

#[test]
fn server_toml_parse_auth_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = ServerConfig::default();
    config.profiles.auth = OFFLINE_AUTH_PROFILE_ID.into();
    let path = temp_dir.path().join("server.toml");
    write_server_toml(&path, &config)?;

    let parsed = ServerConfig::from_toml(&path)?;
    assert_eq!(parsed.profiles.auth, OFFLINE_AUTH_PROFILE_ID);
    Ok(())
}

#[test]
fn server_toml_parse_admin_section_and_failure_policy() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    fs::write(temp_dir.path().join("ops.token"), "token-ops\n")?;
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[live.plugins.failure_policy]
admin_ui = "quarantine"

[live.admin]
ui_profile = "console-v2"
local_console_permissions = ["status", "reload-runtime", "status"]

[static.admin.grpc]
enabled = true
bind_addr = "127.0.0.1:50052"

[static.admin.grpc.principals.ops]
token_file = "ops.token"
permissions = ["status", "reload-runtime", "status"]
"#,
    )?;

    let parsed = ServerConfig::from_toml(&path)?;

    assert_eq!(
        parsed.plugins.failure_policy.admin_ui,
        PluginFailureAction::Quarantine
    );
    assert_eq!(parsed.admin.ui_profile, "console-v2");
    assert_eq!(
        parsed.admin.local_console_permissions,
        vec![
            crate::config::AdminPermission::Status,
            crate::config::AdminPermission::ReloadRuntime,
        ]
    );
    assert!(parsed.admin.grpc.enabled);
    assert_eq!(
        parsed.admin.grpc.bind_addr,
        "127.0.0.1:50052".parse().expect("socket addr should parse")
    );
    assert!(!parsed.admin.grpc.allow_non_loopback);
    assert_eq!(
        parsed
            .admin
            .grpc
            .principals
            .get("ops")
            .expect("ops principal should exist")
            .token,
        "token-ops"
    );
    assert_eq!(
        parsed
            .admin
            .grpc
            .principals
            .get("ops")
            .expect("ops principal should exist")
            .permissions,
        vec![
            crate::config::AdminPermission::Status,
            crate::config::AdminPermission::ReloadRuntime,
        ]
    );
    Ok(())
}

#[test]
fn server_toml_accepts_reload_runtime_admin_permission() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    fs::write(temp_dir.path().join("ops.token"), "token-ops\n")?;
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[live.admin]
local_console_permissions = ["reload-runtime"]

[static.admin.grpc]
enabled = true
bind_addr = "127.0.0.1:50052"

[static.admin.grpc.principals.ops]
token_file = "ops.token"
permissions = ["reload-runtime"]
"#,
    )?;

    let parsed = ServerConfig::from_toml(&path)?;
    assert_eq!(
        parsed.admin.local_console_permissions,
        vec![crate::config::AdminPermission::ReloadRuntime]
    );
    assert_eq!(
        parsed
            .admin
            .grpc
            .principals
            .get("ops")
            .expect("ops principal should exist")
            .permissions,
        vec![crate::config::AdminPermission::ReloadRuntime]
    );
    Ok(())
}

#[test]
fn server_toml_rejects_reload_topology_admin_permission_token() {
    let temp_dir = tempdir().expect("tempdir should be available");
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[live.admin]
local_console_permissions = ["reload-topology"]
"#,
    )
    .expect("server.toml should write");

    let error =
        ServerConfig::from_toml(&path).expect_err("reload-topology token should be rejected");
    assert!(matches!(
        error,
        crate::config::ServerConfigError::Config(message)
            if message.contains("unsupported live.admin.local_console_permissions entry `reload-topology`")
    ));
}

#[test]
fn server_toml_reject_duplicate_admin_grpc_tokens() {
    let temp_dir = tempdir().expect("tempdir should be available");
    fs::write(temp_dir.path().join("ops-a.token"), "shared-token\n")
        .expect("ops-a token should write");
    fs::write(temp_dir.path().join("ops-b.token"), "shared-token\n")
        .expect("ops-b token should write");
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[static.admin.grpc]
enabled = true

[static.admin.grpc.principals.ops_a]
token_file = "ops-a.token"
permissions = ["status"]

[static.admin.grpc.principals.ops_b]
token_file = "ops-b.token"
permissions = ["status"]
"#,
    )
    .expect("server.toml should write");

    let error = ServerConfig::from_toml(&path)
        .expect_err("duplicate remote admin tokens should be rejected");
    assert!(matches!(
        error,
        crate::config::ServerConfigError::Config(message)
            if message.contains("resolved to the same token")
    ));
}

#[test]
fn inline_server_config_source_rejects_duplicate_admin_grpc_tokens() {
    let mut config = ServerConfig::default();
    config.admin.grpc.enabled = true;
    config.admin.grpc.principals.insert(
        "ops_a".to_string(),
        crate::config::AdminGrpcPrincipalConfig {
            token_file: PathBuf::from("runtime/admin/ops-a.token"),
            token: "shared-token".to_string(),
            permissions: vec![crate::config::AdminPermission::Status],
        },
    );
    config.admin.grpc.principals.insert(
        "ops_b".to_string(),
        crate::config::AdminGrpcPrincipalConfig {
            token_file: PathBuf::from("runtime/admin/ops-b.token"),
            token: "shared-token".to_string(),
            permissions: vec![crate::config::AdminPermission::Sessions],
        },
    );

    let error = ServerConfigSource::Inline(config)
        .load()
        .expect_err("duplicate remote admin tokens should be rejected for inline configs");
    assert!(matches!(
        error,
        crate::config::ServerConfigError::Config(message)
            if message.contains("resolved to the same token")
    ));
}

#[test]
fn server_toml_reject_enabled_admin_grpc_without_principals() {
    let temp_dir = tempdir().expect("tempdir should be available");
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[static.admin.grpc]
enabled = true
"#,
    )
    .expect("server.toml should write");

    let error = ServerConfig::from_toml(&path)
        .expect_err("enabled admin gRPC without principals should fail");
    assert_server_config_error_contains(
        error,
        "requires at least one static.admin.grpc.principals entry",
    );
}

#[test]
fn server_toml_reject_empty_admin_grpc_permissions() {
    let temp_dir = tempdir().expect("tempdir should be available");
    fs::write(temp_dir.path().join("ops.token"), "ops-token\n").expect("ops token should write");
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[static.admin.grpc]

[static.admin.grpc.principals.ops]
token_file = "ops.token"
permissions = []
"#,
    )
    .expect("server.toml should write");

    let error =
        ServerConfig::from_toml(&path).expect_err("empty remote admin permissions should fail");
    assert!(matches!(
        error,
        crate::config::ServerConfigError::Config(message)
            if message.contains("admin.grpc.principals.ops.permissions must not be empty")
    ));
}

#[test]
fn server_toml_reject_non_loopback_admin_grpc_bind_without_opt_in() {
    let temp_dir = tempdir().expect("tempdir should be available");
    fs::write(temp_dir.path().join("ops.token"), "ops-token\n").expect("ops token should write");
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[static.admin.grpc]
enabled = true
bind_addr = "0.0.0.0:50051"

[static.admin.grpc.principals.ops]
token_file = "ops.token"
permissions = ["status"]
"#,
    )
    .expect("server.toml should write");

    let error = ServerConfig::from_toml(&path)
        .expect_err("non-loopback admin gRPC bind should require explicit opt-in");
    assert!(matches!(
        error,
        crate::config::ServerConfigError::Config(message)
            if message.contains("admin.grpc.allow_non_loopback=true")
                || message.contains("static.admin.grpc.allow_non_loopback=true")
    ));
}

#[test]
fn server_toml_accepts_non_loopback_admin_grpc_bind_with_opt_in() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    fs::write(temp_dir.path().join("ops.token"), "ops-token\n")?;
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[static.admin.grpc]
enabled = true
bind_addr = "0.0.0.0:50051"
allow_non_loopback = true

[static.admin.grpc.principals.ops]
token_file = "ops.token"
permissions = ["status"]
"#,
    )?;

    let parsed = ServerConfig::from_toml(&path)?;
    assert_eq!(
        parsed.admin.grpc.bind_addr,
        "0.0.0.0:50051".parse().expect("socket addr should parse")
    );
    assert!(parsed.admin.grpc.allow_non_loopback);
    Ok(())
}

#[test]
fn admin_grpc_debug_redacts_tokens() {
    let principal = crate::config::AdminGrpcPrincipalConfig {
        token_file: PathBuf::from("runtime/admin/ops.token"),
        token: "super-secret-token".to_string(),
        permissions: vec![crate::config::AdminPermission::Status],
    };

    let principal_debug = format!("{principal:?}");
    assert!(principal_debug.contains("***redacted***"));
    assert!(!principal_debug.contains("super-secret-token"));

    let mut config = ServerConfig::default();
    config
        .admin
        .grpc
        .principals
        .insert("ops".to_string(), principal);
    let config_debug = format!("{config:?}");
    assert!(config_debug.contains("***redacted***"));
    assert!(!config_debug.contains("super-secret-token"));
}

#[test]
fn admin_subject_debug_redacts_remote_credentials() {
    let subject = crate::runtime::AdminSubject::remote("super-secret-token", "ops");
    let subject_debug = format!("{subject:?}");

    assert!(subject_debug.contains("***redacted***"));
    assert!(subject_debug.contains("ops"));
    assert!(!subject_debug.contains("super-secret-token"));
}

#[test]
fn plugin_host_config_splits_bootstrap_and_runtime_selection_fields() {
    let mut config = ServerConfig::default();
    config.topology.be_enabled = true;
    config.bootstrap.storage_profile = "custom-storage".into();
    config.profiles.auth = "custom-auth".into();
    config.profiles.bedrock_auth = "custom-bedrock-auth".into();
    config.profiles.default_gameplay = "readonly".into();
    config.profiles.gameplay_map = gameplay_profile_map(&[
        (JE_5_ADAPTER_ID, "readonly"),
        (BE_924_ADAPTER_ID, "canonical"),
    ]);
    config.bootstrap.plugins_dir = PathBuf::from("custom").join("plugins");
    config.plugins.allowlist = Some(vec![
        "je-5".to_string(),
        "auth-mojang-online".to_string(),
        "auth-bedrock-xbl".to_string(),
    ]);
    config.plugins.buffer_limits.protocol_response_bytes = 1234;
    config.plugins.buffer_limits.gameplay_response_bytes = 2345;
    config.plugins.buffer_limits.storage_response_bytes = 3456;
    config.plugins.buffer_limits.auth_response_bytes = 4567;
    config.plugins.buffer_limits.admin_ui_response_bytes = 5678;
    config.plugins.buffer_limits.callback_payload_bytes = 6789;
    config.plugins.buffer_limits.metadata_bytes = 7890;
    config.plugins.failure_policy.protocol = PluginFailureAction::Skip;
    config.plugins.failure_policy.gameplay = PluginFailureAction::FailFast;
    config.plugins.failure_policy.storage = PluginFailureAction::Skip;
    config.plugins.failure_policy.auth = PluginFailureAction::FailFast;
    config.plugins.failure_policy.admin_ui = PluginFailureAction::Quarantine;
    config.admin.ui_profile = "console-v2".into();
    config.admin.local_console_permissions = vec![
        crate::config::AdminPermission::Status,
        crate::config::AdminPermission::ReloadRuntime,
    ];
    config.bootstrap.plugin_abi_min = mc_plugin_api::abi::PluginAbiVersion { major: 3, minor: 0 };
    config.bootstrap.plugin_abi_max = mc_plugin_api::abi::PluginAbiVersion { major: 3, minor: 1 };

    let bootstrap = config.plugin_host_bootstrap_config();
    let runtime_selection = config.plugin_host_runtime_selection_config();

    assert_eq!(bootstrap.storage_profile, config.bootstrap.storage_profile);
    assert_eq!(bootstrap.plugins_dir, config.bootstrap.plugins_dir);
    assert_eq!(bootstrap.plugin_abi_min, config.bootstrap.plugin_abi_min);
    assert_eq!(bootstrap.plugin_abi_max, config.bootstrap.plugin_abi_max);

    assert_eq!(runtime_selection.be_enabled, config.topology.be_enabled);
    assert_eq!(runtime_selection.auth_profile, config.profiles.auth);
    assert_eq!(
        runtime_selection.bedrock_auth_profile,
        config.profiles.bedrock_auth
    );
    assert_eq!(
        runtime_selection.default_gameplay_profile,
        config.profiles.default_gameplay
    );
    assert_eq!(
        runtime_selection.gameplay_profile_map,
        config.profiles.gameplay_map
    );
    assert_eq!(runtime_selection.admin_ui_profile, config.admin.ui_profile);
    assert_eq!(runtime_selection.plugin_allowlist, config.plugins.allowlist);
    assert_eq!(
        runtime_selection.buffer_limits,
        config.plugins.buffer_limits
    );
    assert_eq!(
        runtime_selection.plugin_failure_policy_protocol,
        config.plugins.failure_policy.protocol
    );
    assert_eq!(
        runtime_selection.plugin_failure_policy_gameplay,
        config.plugins.failure_policy.gameplay
    );
    assert_eq!(
        runtime_selection.plugin_failure_policy_storage,
        config.plugins.failure_policy.storage
    );
    assert_eq!(
        runtime_selection.plugin_failure_policy_auth,
        config.plugins.failure_policy.auth
    );
    assert_eq!(
        runtime_selection.plugin_failure_policy_admin_ui,
        config.plugins.failure_policy.admin_ui
    );
}

#[test]
fn server_config_splits_static_and_live_views() {
    let mut config = ServerConfig::default();
    config.bootstrap.level_name = "split-world".to_string();
    config.bootstrap.online_mode = true;
    config.bootstrap.world_dir = PathBuf::from("runtime").join("split-world");
    config.bootstrap.plugins_dir = PathBuf::from("runtime").join("split-plugins");
    config.network.motd = "Split runtime".to_string();
    config.network.max_players = 32;
    config.topology.reload_watch = true;
    config.plugins.reload_watch = true;
    config.profiles.default_gameplay = "readonly".into();
    config.admin.ui_profile = "console-v2".into();
    config.admin.grpc.enabled = true;
    config.admin.grpc.bind_addr = "127.0.0.1:50052".parse().expect("socket addr should parse");
    config.admin.grpc.allow_non_loopback = true;

    let static_config = config.static_config();
    let live_config = config.live_config();

    assert_eq!(static_config.bootstrap, config.bootstrap);
    assert!(static_config.admin_grpc.enabled);
    assert_eq!(
        static_config.admin_grpc.bind_addr,
        config.admin.grpc.bind_addr
    );
    assert!(static_config.admin_grpc.allow_non_loopback);

    assert_eq!(live_config.network, config.network);
    assert_eq!(live_config.topology, config.topology);
    assert_eq!(live_config.plugins, config.plugins);
    assert_eq!(live_config.profiles, config.profiles);
    assert_eq!(live_config.admin, config.admin);
}

#[test]
fn server_toml_parse_topology_reload_settings() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = ServerConfig::default();
    config.topology.reload_watch = true;
    config.topology.drain_grace_secs = 45;
    let path = temp_dir.path().join("server.toml");
    write_server_toml(&path, &config)?;

    let parsed = ServerConfig::from_toml(&path)?;
    assert!(parsed.topology.reload_watch);
    assert_eq!(parsed.topology.drain_grace_secs, 45);
    Ok(())
}

#[test]
fn server_toml_parse_per_kind_failure_policies() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = ServerConfig::default();
    config.plugins.failure_policy.protocol = PluginFailureAction::Skip;
    config.plugins.failure_policy.gameplay = PluginFailureAction::FailFast;
    config.plugins.failure_policy.storage = PluginFailureAction::Skip;
    config.plugins.failure_policy.auth = PluginFailureAction::FailFast;
    let path = temp_dir.path().join("server.toml");
    write_server_toml(&path, &config)?;

    let parsed = ServerConfig::from_toml(&path)?;
    assert_eq!(
        parsed.plugins.failure_policy.protocol,
        PluginFailureAction::Skip
    );
    assert_eq!(
        parsed.plugins.failure_policy.gameplay,
        PluginFailureAction::FailFast
    );
    assert_eq!(
        parsed.plugins.failure_policy.storage,
        PluginFailureAction::Skip
    );
    assert_eq!(
        parsed.plugins.failure_policy.auth,
        PluginFailureAction::FailFast
    );
    Ok(())
}

#[test]
fn server_toml_parse_plugin_buffer_limits() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = ServerConfig::default();
    config.plugins.buffer_limits.protocol_response_bytes = 11;
    config.plugins.buffer_limits.gameplay_response_bytes = 22;
    config.plugins.buffer_limits.storage_response_bytes = 33;
    config.plugins.buffer_limits.auth_response_bytes = 44;
    config.plugins.buffer_limits.admin_ui_response_bytes = 55;
    config.plugins.buffer_limits.callback_payload_bytes = 66;
    config.plugins.buffer_limits.metadata_bytes = 77;
    let path = temp_dir.path().join("server.toml");
    write_server_toml(&path, &config)?;

    let parsed = ServerConfig::from_toml(&path)?;
    assert_eq!(parsed.plugins.buffer_limits, config.plugins.buffer_limits);
    Ok(())
}

#[test]
fn server_toml_use_balanced_failure_policy_defaults() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.toml");
    write_server_toml(&path, &ServerConfig::default())?;

    let config = ServerConfig::from_toml(&path)?;
    assert_eq!(
        config.plugins.buffer_limits,
        ServerConfig::default().plugins.buffer_limits
    );
    assert_eq!(
        config.plugins.failure_policy.protocol,
        PluginFailureAction::Quarantine
    );
    assert_eq!(
        config.plugins.failure_policy.gameplay,
        PluginFailureAction::Quarantine
    );
    assert_eq!(
        config.plugins.failure_policy.storage,
        PluginFailureAction::FailFast
    );
    assert_eq!(
        config.plugins.failure_policy.auth,
        PluginFailureAction::Skip
    );
    Ok(())
}

#[test]
fn server_toml_reject_invalid_failure_policy_for_kind() {
    let temp_dir = tempdir().expect("tempdir should be available");
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[live.plugins]

[live.plugins.failure_policy]
storage = "quarantine"
"#,
    )
    .expect("server.toml should write");

    let error = ServerConfig::from_toml(&path)
        .expect_err("storage failure policy should reject quarantine");
    assert_server_config_error_contains(error, "plugin-failure-policy-storage");
}

#[test]
fn server_toml_reject_non_flat_level_type() {
    let temp_dir = tempdir().expect("tempdir should be available");
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[static.bootstrap]
level_type = "DEFAULT"
"#,
    )
    .expect("server.toml should write");

    let error = ServerConfig::from_toml(&path).expect_err("DEFAULT should be rejected");
    assert!(matches!(
        error,
        crate::config::ServerConfigError::Unsupported(message)
            if message.contains("only `flat`")
    ));
}

#[test]
fn be_enabled_requires_udp_adapter() {
    let registry =
        plugin_test_registries_tcp_only().expect("tcp-only plugin registry should be available");
    let mut config = ServerConfig::default();
    config.topology.be_enabled = true;
    let error = build_listener_plans(&config, registry.protocols())
        .expect_err("be-enabled should require udp adapter");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("be-enabled=true")
    ));
}

#[tokio::test]
async fn enabled_adapters_must_include_default_adapter() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.topology.enabled_adapters = Some(vec![JE_47_ADAPTER_ID.into()]);
    assert_spawn_fails_with_message(config, "default-adapter").await
}

#[tokio::test]
async fn duplicate_enabled_adapters_fail_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into(), JE_5_ADAPTER_ID.into()]);
    assert_spawn_fails_with_message(config, "duplicate adapter").await
}

#[test]
fn plugin_abi_range_must_include_current_host_abi() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    seed_runtime_plugins(
        &dist_dir,
        TCP_ONLY_PROTOCOL_PLUGIN_IDS,
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )?;
    let mut config = ServerConfig::default();
    config.bootstrap.plugins_dir = dist_dir;
    config.plugins.allowlist = Some(plugin_allowlist_with_supporting_plugins(
        TCP_ONLY_PROTOCOL_PLUGIN_IDS,
        STORAGE_AND_AUTH_PLUGIN_IDS,
    ));
    config.bootstrap.plugin_abi_min = mc_plugin_api::abi::PluginAbiVersion { major: 2, minor: 0 };
    config.bootstrap.plugin_abi_max = mc_plugin_api::abi::PluginAbiVersion { major: 2, minor: 9 };
    let error = match plugin_test_registries_from_config(&config) {
        Ok(_) => panic!("plugin ABI range should reject configs that exclude the current host ABI"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("does not include current host ABI")
    ));
    Ok(())
}
