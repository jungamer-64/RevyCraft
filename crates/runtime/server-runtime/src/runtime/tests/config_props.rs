use super::*;

fn loopback_server_config(world_dir: PathBuf) -> ServerConfig {
    ServerConfig {
        server_ip: Some("127.0.0.1".parse().expect("loopback should parse")),
        server_port: 0,
        world_dir,
        ..ServerConfig::default()
    }
}

async fn assert_spawn_fails_with_message(
    config: ServerConfig,
    expected_fragment: &str,
) -> Result<(), RuntimeError> {
    let result = spawn_server(config, plugin_test_registries_all()?).await;
    let Err(error) = result else {
        panic!("spawn_server should have failed");
    };
    assert!(
        matches!(error, RuntimeError::Config(ref message) if message.contains(expected_fragment)),
        "unexpected error: {error:?}"
    );
    Ok(())
}

#[test]
fn server_properties_accept_flat_level_type() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(
        &path,
        "level-name=flatland\nlevel-type=FLAT\nbe-enabled=true\nonline-mode=false\ndefault-adapter=je-1_7_10\nstorage-profile=je-anvil-1_7_10\nauth-profile=offline-v1\n",
    )?;

    let config = ServerConfig::from_properties(&path)?;

    assert_eq!(config.level_name, "flatland");
    assert_eq!(config.level_type, LevelType::Flat);
    assert!(config.be_enabled);
    assert_eq!(config.default_adapter, JE_1_7_10_ADAPTER_ID);
    assert_eq!(config.default_bedrock_adapter, BE_26_3_ADAPTER_ID);
    assert_eq!(config.storage_profile, JE_1_7_10_STORAGE_PROFILE_ID);
    assert_eq!(config.auth_profile, OFFLINE_AUTH_PROFILE_ID);
    assert_eq!(config.bedrock_auth_profile, BEDROCK_OFFLINE_AUTH_PROFILE_ID);
    assert_eq!(config.world_dir, temp_dir.path().join("flatland"));
    Ok(())
}

#[test]
fn server_properties_use_default_adapter_and_storage_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(&path, "level-name=flatland\nlevel-type=FLAT\n")?;

    let config = ServerConfig::from_properties(&path)?;

    assert!(!config.be_enabled);
    assert_eq!(config.default_adapter, JE_1_7_10_ADAPTER_ID);
    assert_eq!(config.default_bedrock_adapter, BE_26_3_ADAPTER_ID);
    assert_eq!(config.storage_profile, JE_1_7_10_STORAGE_PROFILE_ID);
    assert_eq!(config.auth_profile, OFFLINE_AUTH_PROFILE_ID);
    assert_eq!(config.bedrock_auth_profile, BEDROCK_OFFLINE_AUTH_PROFILE_ID);
    Ok(())
}

#[test]
fn default_config_uses_relative_runtime_paths() {
    let config = ServerConfig::default();
    assert_eq!(config.plugins_dir, PathBuf::from("runtime").join("plugins"));
    assert_eq!(config.world_dir, PathBuf::from("runtime").join("world"));
}

#[test]
fn server_properties_parse_enabled_adapters() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(&path, "enabled-adapters=je-1_7_10, je-1_8_x,je-1_12_2\n")?;

    let config = ServerConfig::from_properties(&path)?;
    assert_eq!(
        config.enabled_adapters,
        Some(vec![
            JE_1_7_10_ADAPTER_ID.to_string(),
            JE_1_8_X_ADAPTER_ID.to_string(),
            JE_1_12_2_ADAPTER_ID.to_string(),
        ])
    );
    Ok(())
}

#[test]
fn server_properties_parse_bedrock_adapter_and_auth_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(
        &path,
        "be-enabled=true\ndefault-bedrock-adapter=be-26_3\nenabled-bedrock-adapters=be-26_3,be-placeholder\nbedrock-auth-profile=bedrock-xbl-v1\n",
    )?;

    let config = ServerConfig::from_properties(&path)?;
    assert!(config.be_enabled);
    assert_eq!(config.default_bedrock_adapter, BE_26_3_ADAPTER_ID);
    assert_eq!(
        config.enabled_bedrock_adapters,
        Some(vec![
            BE_26_3_ADAPTER_ID.to_string(),
            BE_PLACEHOLDER_ADAPTER_ID.to_string(),
        ])
    );
    assert_eq!(config.bedrock_auth_profile, "bedrock-xbl-v1");
    Ok(())
}

#[test]
fn server_properties_parse_gameplay_profile_configuration() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(
        &path,
        "default-gameplay-profile=canonical\ngameplay-profile-map=je-1_7_10:readonly,je-1_12_2:canonical\n",
    )?;

    let config = ServerConfig::from_properties(&path)?;
    assert_eq!(config.default_gameplay_profile, "canonical");
    assert_eq!(
        config.gameplay_profile_map,
        gameplay_profile_map(&[
            (JE_1_7_10_ADAPTER_ID, "readonly"),
            (JE_1_12_2_ADAPTER_ID, "canonical"),
        ])
    );
    Ok(())
}

#[test]
fn server_properties_parse_auth_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(&path, "auth-profile=offline-v1\n")?;

    let config = ServerConfig::from_properties(&path)?;
    assert_eq!(config.auth_profile, OFFLINE_AUTH_PROFILE_ID);
    Ok(())
}

#[test]
fn plugin_host_config_copies_all_plugin_host_fields() {
    let config = ServerConfig {
        be_enabled: true,
        storage_profile: "custom-storage".to_string(),
        auth_profile: "custom-auth".to_string(),
        bedrock_auth_profile: "custom-bedrock-auth".to_string(),
        default_gameplay_profile: "readonly".to_string(),
        gameplay_profile_map: gameplay_profile_map(&[
            (JE_1_7_10_ADAPTER_ID, "readonly"),
            (BE_26_3_ADAPTER_ID, "canonical"),
        ]),
        plugins_dir: PathBuf::from("custom").join("plugins"),
        plugin_allowlist: Some(vec![
            "je-1_7_10".to_string(),
            "auth-mojang-online".to_string(),
            "auth-bedrock-xbl".to_string(),
        ]),
        plugin_failure_policy_protocol: PluginFailureAction::Skip,
        plugin_failure_policy_gameplay: PluginFailureAction::FailFast,
        plugin_failure_policy_storage: PluginFailureAction::Skip,
        plugin_failure_policy_auth: PluginFailureAction::FailFast,
        plugin_abi_min: mc_plugin_api::abi::PluginAbiVersion { major: 1, minor: 9 },
        plugin_abi_max: mc_plugin_api::abi::PluginAbiVersion { major: 2, minor: 1 },
        ..ServerConfig::default()
    };

    let plugin_host_config = config.plugin_host_config();

    assert_eq!(plugin_host_config.be_enabled, config.be_enabled);
    assert_eq!(plugin_host_config.storage_profile, config.storage_profile);
    assert_eq!(plugin_host_config.auth_profile, config.auth_profile);
    assert_eq!(
        plugin_host_config.bedrock_auth_profile,
        config.bedrock_auth_profile
    );
    assert_eq!(
        plugin_host_config.default_gameplay_profile,
        config.default_gameplay_profile
    );
    assert_eq!(
        plugin_host_config.gameplay_profile_map,
        config.gameplay_profile_map
    );
    assert_eq!(plugin_host_config.plugins_dir, config.plugins_dir);
    assert_eq!(plugin_host_config.plugin_allowlist, config.plugin_allowlist);
    assert_eq!(
        plugin_host_config.plugin_failure_policy_protocol,
        config.plugin_failure_policy_protocol
    );
    assert_eq!(
        plugin_host_config.plugin_failure_policy_gameplay,
        config.plugin_failure_policy_gameplay
    );
    assert_eq!(
        plugin_host_config.plugin_failure_policy_storage,
        config.plugin_failure_policy_storage
    );
    assert_eq!(
        plugin_host_config.plugin_failure_policy_auth,
        config.plugin_failure_policy_auth
    );
    assert_eq!(plugin_host_config.plugin_abi_min, config.plugin_abi_min);
    assert_eq!(plugin_host_config.plugin_abi_max, config.plugin_abi_max);
}

#[test]
fn server_properties_parse_topology_reload_settings() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(
        &path,
        "topology-reload-watch=true\ntopology-drain-grace-secs=45\n",
    )?;

    let config = ServerConfig::from_properties(&path)?;
    assert!(config.topology_reload_watch);
    assert_eq!(config.topology_drain_grace_secs, 45);
    Ok(())
}

#[test]
fn server_properties_parse_per_kind_failure_policies() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(
        &path,
        "plugin-failure-policy-protocol=skip\nplugin-failure-policy-gameplay=fail-fast\nplugin-failure-policy-storage=skip\nplugin-failure-policy-auth=fail-fast\n",
    )?;

    let config = ServerConfig::from_properties(&path)?;
    assert_eq!(
        config.plugin_failure_policy_protocol,
        PluginFailureAction::Skip
    );
    assert_eq!(
        config.plugin_failure_policy_gameplay,
        PluginFailureAction::FailFast
    );
    assert_eq!(
        config.plugin_failure_policy_storage,
        PluginFailureAction::Skip
    );
    assert_eq!(
        config.plugin_failure_policy_auth,
        PluginFailureAction::FailFast
    );
    Ok(())
}

#[test]
fn server_properties_use_balanced_failure_policy_defaults() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(&path, "level-name=flatland\n")?;

    let config = ServerConfig::from_properties(&path)?;
    assert_eq!(
        config.plugin_failure_policy_protocol,
        PluginFailureAction::Quarantine
    );
    assert_eq!(
        config.plugin_failure_policy_gameplay,
        PluginFailureAction::Quarantine
    );
    assert_eq!(
        config.plugin_failure_policy_storage,
        PluginFailureAction::FailFast
    );
    assert_eq!(config.plugin_failure_policy_auth, PluginFailureAction::Skip);
    Ok(())
}

#[test]
fn server_properties_reject_legacy_failure_policy_key() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(&path, "plugin-failure-policy=quarantine\n")?;

    let error =
        ServerConfig::from_properties(&path).expect_err("legacy failure policy key should fail");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("plugin-failure-policy is no longer supported")
    ));
    Ok(())
}

#[test]
fn server_properties_reject_invalid_failure_policy_for_kind() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(&path, "plugin-failure-policy-storage=quarantine\n")?;

    let error = ServerConfig::from_properties(&path)
        .expect_err("storage failure policy should reject quarantine");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("plugin-failure-policy-storage")
    ));
    Ok(())
}

#[test]
fn server_properties_reject_non_flat_level_type() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let path = temp_dir.path().join("server.properties");
    fs::write(&path, "level-type=DEFAULT\n")?;

    let error = ServerConfig::from_properties(&path).expect_err("DEFAULT should be rejected");
    assert!(matches!(error, RuntimeError::Unsupported(message) if message.contains("only FLAT")));
    Ok(())
}

#[test]
fn be_enabled_requires_udp_adapter() {
    let registry =
        plugin_test_registries_tcp_only().expect("tcp-only plugin registry should be available");
    let error = build_listener_plans(
        &ServerConfig {
            be_enabled: true,
            ..ServerConfig::default()
        },
        registry.protocols(),
    )
    .expect_err("be-enabled should require udp adapter");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("be-enabled=true")
    ));
}

#[tokio::test]
async fn enabled_adapters_must_include_default_adapter() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    assert_spawn_fails_with_message(
        ServerConfig {
            enabled_adapters: Some(vec![JE_1_8_X_ADAPTER_ID.to_string()]),
            ..loopback_server_config(temp_dir.path().join("world"))
        },
        "default-adapter",
    )
    .await
}

#[tokio::test]
async fn duplicate_enabled_adapters_fail_fast() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    assert_spawn_fails_with_message(
        ServerConfig {
            enabled_adapters: Some(vec![
                JE_1_7_10_ADAPTER_ID.to_string(),
                JE_1_7_10_ADAPTER_ID.to_string(),
            ]),
            ..loopback_server_config(temp_dir.path().join("world"))
        },
        "duplicate adapter",
    )
    .await
}
