use super::*;

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
fn server_toml_accepts_grouped_schema_and_resolves_relative_paths() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[bootstrap]
online_mode = false
level_name = "flatland"
level_type = "FLAT"

[network]
server_ip = "127.0.0.1"
server_port = 0

[topology]
be_enabled = true
default_adapter = "je-1_7_10"

[plugins]

[plugins.failure_policy]

[profiles]
auth = "offline-v1"
"#,
    )?;

    let config = ServerConfig::from_toml(&path)?;

    assert_eq!(config.bootstrap.level_name, "flatland");
    assert_eq!(config.bootstrap.level_type, LevelType::Flat);
    assert!(config.topology.be_enabled);
    assert_eq!(config.topology.default_adapter, JE_1_7_10_ADAPTER_ID);
    assert_eq!(
        config.topology.default_bedrock_adapter,
        BE_26_3_ADAPTER_ID.to_string()
    );
    assert_eq!(
        config.bootstrap.storage_profile,
        JE_1_7_10_STORAGE_PROFILE_ID.to_string()
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
        JE_1_7_10_ADAPTER_ID.to_string(),
        JE_1_8_X_ADAPTER_ID.to_string(),
        JE_1_12_2_ADAPTER_ID.to_string(),
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
    config.topology.default_bedrock_adapter = BE_26_3_ADAPTER_ID.to_string();
    config.topology.enabled_bedrock_adapters = Some(vec![
        BE_26_3_ADAPTER_ID.to_string(),
        BE_PLACEHOLDER_ADAPTER_ID.to_string(),
    ]);
    config.profiles.bedrock_auth = "bedrock-xbl-v1".to_string();
    let path = temp_dir.path().join("server.toml");
    write_server_toml(&path, &config)?;

    let parsed = ServerConfig::from_toml(&path)?;
    assert!(parsed.topology.be_enabled);
    assert_eq!(
        parsed.topology.default_bedrock_adapter,
        BE_26_3_ADAPTER_ID.to_string()
    );
    assert_eq!(
        parsed.topology.enabled_bedrock_adapters,
        Some(vec![
            BE_26_3_ADAPTER_ID.to_string(),
            BE_PLACEHOLDER_ADAPTER_ID.to_string(),
        ])
    );
    assert_eq!(parsed.profiles.bedrock_auth, "bedrock-xbl-v1");
    Ok(())
}

#[test]
fn server_toml_parse_gameplay_profile_configuration() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = ServerConfig::default();
    config.profiles.default_gameplay = "canonical".to_string();
    config.profiles.gameplay_map = gameplay_profile_map(&[
        (JE_1_7_10_ADAPTER_ID, "readonly"),
        (JE_1_12_2_ADAPTER_ID, "canonical"),
    ]);
    let path = temp_dir.path().join("server.toml");
    write_server_toml(&path, &config)?;

    let parsed = ServerConfig::from_toml(&path)?;
    assert_eq!(parsed.profiles.default_gameplay, "canonical");
    assert_eq!(
        parsed.profiles.gameplay_map,
        gameplay_profile_map(&[
            (JE_1_7_10_ADAPTER_ID, "readonly"),
            (JE_1_12_2_ADAPTER_ID, "canonical"),
        ])
    );
    Ok(())
}

#[test]
fn server_toml_parse_auth_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = ServerConfig::default();
    config.profiles.auth = OFFLINE_AUTH_PROFILE_ID.to_string();
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
[plugins.failure_policy]
admin_ui = "quarantine"

[admin]
ui_profile = "console-v2"
local_console_permissions = ["status", "reload_config", "status"]

[admin.grpc]
enabled = true
bind_addr = "127.0.0.1:50052"

[admin.grpc.principals.ops]
token_file = "ops.token"
permissions = ["status", "reload_plugins", "status"]
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
            crate::runtime::AdminPermission::Status,
            crate::runtime::AdminPermission::ReloadConfig,
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
            crate::runtime::AdminPermission::Status,
            crate::runtime::AdminPermission::ReloadPlugins,
        ]
    );
    Ok(())
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
[admin.grpc]
enabled = true

[admin.grpc.principals.ops_a]
token_file = "ops-a.token"
permissions = ["status"]

[admin.grpc.principals.ops_b]
token_file = "ops-b.token"
permissions = ["status"]
"#,
    )
    .expect("server.toml should write");

    let error = ServerConfig::from_toml(&path)
        .expect_err("duplicate remote admin tokens should be rejected");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("resolved to the same token")
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
            permissions: vec![crate::runtime::AdminPermission::Status],
        },
    );
    config.admin.grpc.principals.insert(
        "ops_b".to_string(),
        crate::config::AdminGrpcPrincipalConfig {
            token_file: PathBuf::from("runtime/admin/ops-b.token"),
            token: "shared-token".to_string(),
            permissions: vec![crate::runtime::AdminPermission::Sessions],
        },
    );

    let error = ServerConfigSource::Inline(config)
        .load()
        .expect_err("duplicate remote admin tokens should be rejected for inline configs");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("resolved to the same token")
    ));
}

#[test]
fn server_toml_reject_enabled_admin_grpc_without_principals() {
    let temp_dir = tempdir().expect("tempdir should be available");
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[admin.grpc]
enabled = true
"#,
    )
    .expect("server.toml should write");

    let error = ServerConfig::from_toml(&path)
        .expect_err("enabled admin gRPC without principals should fail");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("requires at least one admin.grpc.principals entry")
    ));
}

#[test]
fn server_toml_reject_empty_admin_grpc_permissions() {
    let temp_dir = tempdir().expect("tempdir should be available");
    fs::write(temp_dir.path().join("ops.token"), "ops-token\n").expect("ops token should write");
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[admin.grpc]

[admin.grpc.principals.ops]
token_file = "ops.token"
permissions = []
"#,
    )
    .expect("server.toml should write");

    let error =
        ServerConfig::from_toml(&path).expect_err("empty remote admin permissions should fail");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("admin.grpc.principals.ops.permissions must not be empty")
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
[admin.grpc]
enabled = true
bind_addr = "0.0.0.0:50051"

[admin.grpc.principals.ops]
token_file = "ops.token"
permissions = ["status"]
"#,
    )
    .expect("server.toml should write");

    let error = ServerConfig::from_toml(&path)
        .expect_err("non-loopback admin gRPC bind should require explicit opt-in");
    assert!(matches!(
        error,
        RuntimeError::Config(message)
            if message.contains("admin.grpc.allow_non_loopback=true")
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
[admin.grpc]
enabled = true
bind_addr = "0.0.0.0:50051"
allow_non_loopback = true

[admin.grpc.principals.ops]
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
        permissions: vec![crate::runtime::AdminPermission::Status],
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
fn plugin_host_config_copies_all_plugin_host_fields() {
    let mut config = ServerConfig::default();
    config.topology.be_enabled = true;
    config.bootstrap.storage_profile = "custom-storage".to_string();
    config.profiles.auth = "custom-auth".to_string();
    config.profiles.bedrock_auth = "custom-bedrock-auth".to_string();
    config.profiles.default_gameplay = "readonly".to_string();
    config.profiles.gameplay_map = gameplay_profile_map(&[
        (JE_1_7_10_ADAPTER_ID, "readonly"),
        (BE_26_3_ADAPTER_ID, "canonical"),
    ]);
    config.bootstrap.plugins_dir = PathBuf::from("custom").join("plugins");
    config.plugins.allowlist = Some(vec![
        "je-1_7_10".to_string(),
        "auth-mojang-online".to_string(),
        "auth-bedrock-xbl".to_string(),
    ]);
    config.plugins.failure_policy.protocol = PluginFailureAction::Skip;
    config.plugins.failure_policy.gameplay = PluginFailureAction::FailFast;
    config.plugins.failure_policy.storage = PluginFailureAction::Skip;
    config.plugins.failure_policy.auth = PluginFailureAction::FailFast;
    config.plugins.failure_policy.admin_ui = PluginFailureAction::Quarantine;
    config.admin.ui_profile = "console-v2".to_string();
    config.admin.local_console_permissions = vec![
        crate::runtime::AdminPermission::Status,
        crate::runtime::AdminPermission::ReloadPlugins,
    ];
    config.bootstrap.plugin_abi_min = mc_plugin_api::abi::PluginAbiVersion { major: 3, minor: 0 };
    config.bootstrap.plugin_abi_max = mc_plugin_api::abi::PluginAbiVersion { major: 3, minor: 1 };

    let plugin_host_config = config.plugin_host_config();

    assert_eq!(plugin_host_config.be_enabled, config.topology.be_enabled);
    assert_eq!(
        plugin_host_config.storage_profile,
        config.bootstrap.storage_profile
    );
    assert_eq!(plugin_host_config.auth_profile, config.profiles.auth);
    assert_eq!(
        plugin_host_config.bedrock_auth_profile,
        config.profiles.bedrock_auth
    );
    assert_eq!(
        plugin_host_config.default_gameplay_profile,
        config.profiles.default_gameplay
    );
    assert_eq!(
        plugin_host_config.gameplay_profile_map,
        config.profiles.gameplay_map
    );
    assert_eq!(plugin_host_config.plugins_dir, config.bootstrap.plugins_dir);
    assert_eq!(
        plugin_host_config.plugin_allowlist,
        config.plugins.allowlist
    );
    assert_eq!(
        plugin_host_config.plugin_failure_policy_protocol,
        config.plugins.failure_policy.protocol
    );
    assert_eq!(
        plugin_host_config.plugin_failure_policy_gameplay,
        config.plugins.failure_policy.gameplay
    );
    assert_eq!(
        plugin_host_config.plugin_failure_policy_storage,
        config.plugins.failure_policy.storage
    );
    assert_eq!(
        plugin_host_config.plugin_failure_policy_auth,
        config.plugins.failure_policy.auth
    );
    assert_eq!(
        plugin_host_config.plugin_failure_policy_admin_ui,
        config.plugins.failure_policy.admin_ui
    );
    assert_eq!(plugin_host_config.admin_ui_profile, config.admin.ui_profile);
    assert_eq!(
        plugin_host_config.plugin_abi_min,
        config.bootstrap.plugin_abi_min
    );
    assert_eq!(
        plugin_host_config.plugin_abi_max,
        config.bootstrap.plugin_abi_max
    );
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
fn server_toml_use_balanced_failure_policy_defaults() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.toml");
    write_server_toml(&path, &ServerConfig::default())?;

    let config = ServerConfig::from_toml(&path)?;
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
[bootstrap]

[network]

[topology]

[plugins]

[plugins.failure_policy]
storage = "quarantine"

[profiles]
"#,
    )
    .expect("server.toml should write");

    let error = ServerConfig::from_toml(&path)
        .expect_err("storage failure policy should reject quarantine");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("plugin-failure-policy-storage")
    ));
}

#[test]
fn server_toml_reject_non_flat_level_type() {
    let temp_dir = tempdir().expect("tempdir should be available");
    let path = temp_dir.path().join("server.toml");
    fs::write(
        &path,
        r#"
[bootstrap]
level_type = "DEFAULT"

[network]

[topology]

[plugins]

[plugins.failure_policy]

[profiles]
"#,
    )
    .expect("server.toml should write");

    let error = ServerConfig::from_toml(&path).expect_err("DEFAULT should be rejected");
    assert!(matches!(
        error,
        RuntimeError::Unsupported(message) if message.contains("only `flat`")
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
    config.topology.enabled_adapters = Some(vec![JE_1_8_X_ADAPTER_ID.to_string()]);
    assert_spawn_fails_with_message(config, "default-adapter").await
}

#[tokio::test]
async fn duplicate_enabled_adapters_fail_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.topology.enabled_adapters = Some(vec![
        JE_1_7_10_ADAPTER_ID.to_string(),
        JE_1_7_10_ADAPTER_ID.to_string(),
    ]);
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
