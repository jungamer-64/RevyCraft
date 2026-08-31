use super::*;

const FAILING_STORAGE_PLUGIN_ID: &str = "storage-failing-runtime";
pub(crate) const FAILING_STORAGE_PROFILE_ID: &str = "failing-storage";
const FAILING_STORAGE_CARGO_PACKAGE: &str = "mc-plugin-fixture-storage-failing";

pub(crate) const ALL_PROTOCOL_PLUGIN_IDS: &[&str] = &[
    JE_5_ADAPTER_ID,
    JE_47_ADAPTER_ID,
    JE_340_ADAPTER_ID,
    JE_404_ADAPTER_ID,
    JE_775_ADAPTER_ID,
    BE_924_ADAPTER_ID,
    BE_PLACEHOLDER_ADAPTER_ID,
];
pub(crate) const TCP_ONLY_PROTOCOL_PLUGIN_IDS: &[&str] = &[JE_5_ADAPTER_ID];
pub(crate) const GAMEPLAY_PLUGIN_IDS: &[&str] = &["gameplay-canonical", "gameplay-readonly"];
pub(crate) const GRPC_ADMIN_SURFACE_PLUGIN_IDS: &[&str] = &["admin-grpc"];
pub(crate) const CONSOLE_ADMIN_SURFACE_PLUGIN_IDS: &[&str] = &["admin-console"];
pub(crate) const STORAGE_AND_AUTH_PLUGIN_IDS: &[&str] = &[
    "storage-je-anvil-1_7_10",
    "auth-offline",
    "auth-bedrock-offline",
    "auth-bedrock-xbl",
];
pub(crate) const STORAGE_1_18_2_AND_AUTH_PLUGIN_IDS: &[&str] = &[
    JE_1_18_2_STORAGE_PLUGIN_ID,
    "auth-offline",
    "auth-bedrock-offline",
    "auth-bedrock-xbl",
];
pub(crate) const STORAGE_26_1_AND_AUTH_PLUGIN_IDS: &[&str] = &[
    JE_26_1_STORAGE_PLUGIN_ID,
    "auth-offline",
    "auth-bedrock-offline",
    "auth-bedrock-xbl",
];
pub(crate) const PACKAGED_PLUGIN_TEST_HARNESS_TAG: &str = "runtime-test-harness";

pub(crate) fn plugin_test_registries_with_allowlist(
    allowlist: &[&str],
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let dist_dir = PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .dist_dir()
        .to_path_buf();
    plugin_test_registries_from_dist(dist_dir, allowlist)
}

pub(crate) fn plugin_allowlist_with_supporting_plugins(
    allowlist: &[&str],
    supporting_plugin_ids: &[&str],
) -> Vec<String> {
    let mut plugin_allowlist = allowlist
        .iter()
        .map(|entry| (*entry).to_string())
        .collect::<Vec<_>>();
    plugin_allowlist.extend(
        GAMEPLAY_PLUGIN_IDS
            .iter()
            .map(|plugin_id| (*plugin_id).to_string()),
    );
    plugin_allowlist.extend(
        supporting_plugin_ids
            .iter()
            .map(|plugin_id| (*plugin_id).to_string()),
    );
    plugin_allowlist.extend(
        GRPC_ADMIN_SURFACE_PLUGIN_IDS
            .iter()
            .map(|plugin_id| (*plugin_id).to_string()),
    );
    plugin_allowlist.extend(
        CONSOLE_ADMIN_SURFACE_PLUGIN_IDS
            .iter()
            .map(|plugin_id| (*plugin_id).to_string()),
    );
    plugin_allowlist
}

pub(crate) fn plugin_test_registries_from_dist(
    dist_dir: PathBuf,
    allowlist: &[&str],
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    plugin_test_registries_from_dist_with_supporting_plugins(
        dist_dir,
        allowlist,
        STORAGE_AND_AUTH_PLUGIN_IDS,
    )
}

pub(crate) fn plugin_test_registries_from_dist_with_supporting_plugins(
    dist_dir: PathBuf,
    allowlist: &[&str],
    supporting_plugin_ids: &[&str],
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let mut config = ServerConfig::default();
    config.bootstrap.plugins_dir = dist_dir.clone();
    config.plugins.allowlist = Some(plugin_allowlist_with_supporting_plugins(
        allowlist,
        supporting_plugin_ids,
    ));
    if supporting_plugin_ids.contains(&ONLINE_STUB_AUTH_PLUGIN_ID) {
        config.profiles.auth = ONLINE_STUB_AUTH_PROFILE_ID.into();
    }
    let bootstrap = plugin_host_bootstrap_test_config(&config);
    let runtime_selection = plugin_host_runtime_selection_test_config(&config);
    let plugin_host =
        mc_plugin_host::host::plugin_host_from_config(&bootstrap)?.ok_or_else(|| {
            RuntimeError::Config("packaged protocol plugins should be discovered".to_string())
        })?;
    Ok(LoadedPluginTestEnvironment {
        loaded_plugins: plugin_host.load_plugin_set(&runtime_selection)?,
        plugin_host: Some(plugin_host),
    })
}

pub(crate) fn plugin_test_registries_from_config(
    config: &ServerConfig,
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let bootstrap = plugin_host_bootstrap_test_config(config);
    let runtime_selection = plugin_host_runtime_selection_test_config(config);
    let plugin_host =
        mc_plugin_host::host::plugin_host_from_config(&bootstrap)?.ok_or_else(|| {
            RuntimeError::Config("packaged protocol plugins should be discovered".to_string())
        })?;
    Ok(LoadedPluginTestEnvironment {
        loaded_plugins: plugin_host.load_plugin_set(&runtime_selection)?,
        plugin_host: Some(plugin_host),
    })
}

pub(crate) fn seed_runtime_plugins(
    dist_dir: &Path,
    allowlist: &[&str],
    supporting_plugin_ids: &[&str],
) -> Result<(), RuntimeError> {
    let mut plugin_ids = Vec::new();
    plugin_ids.extend_from_slice(allowlist);
    plugin_ids.extend_from_slice(GAMEPLAY_PLUGIN_IDS);
    plugin_ids.extend_from_slice(supporting_plugin_ids);
    plugin_ids.extend_from_slice(GRPC_ADMIN_SURFACE_PLUGIN_IDS);
    plugin_ids.extend_from_slice(CONSOLE_ADMIN_SURFACE_PLUGIN_IDS);
    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .seed_subset(dist_dir, &plugin_ids)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn plugin_test_registries_tcp_only() -> Result<LoadedPluginTestEnvironment, RuntimeError>
{
    plugin_test_registries_with_allowlist(TCP_ONLY_PROTOCOL_PLUGIN_IDS)
}

pub(crate) fn plugin_test_registries_all() -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    plugin_test_registries_with_allowlist(ALL_PROTOCOL_PLUGIN_IDS)
}

pub(crate) fn packaged_online_auth_registries(
    allowlist: &[&str],
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    plugin_test_registries_from_dist_with_supporting_plugins(
        PackagedPluginHarness::shared()
            .map_err(|error| RuntimeError::Config(error.to_string()))?
            .dist_dir()
            .to_path_buf(),
        allowlist,
        &["storage-je-anvil-1_7_10", ONLINE_STUB_AUTH_PLUGIN_ID],
    )
}

pub(crate) fn packaged_default_registries(
    allowlist: &[&str],
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    plugin_test_registries_with_allowlist(allowlist)
}

pub(crate) fn packaged_failing_storage_registries(
    failure_action: PluginFailureAction,
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let harness =
        PackagedPluginHarness::shared().map_err(|error| RuntimeError::Config(error.to_string()))?;
    let scope = format!(
        "failing-storage-{}",
        uuid::Uuid::new_v3(
            &uuid::Uuid::NAMESPACE_OID,
            format!("{:?}", failure_action).as_bytes()
        )
    );
    let dist_dir = workspace_test_temp_root()
        .join(&scope)
        .join("runtime")
        .join("plugins");
    let supporting_plugin_ids = &["auth-offline", "auth-bedrock-offline", "auth-bedrock-xbl"];
    seed_runtime_plugins(&dist_dir, &[JE_5_ADAPTER_ID], supporting_plugin_ids)?;
    harness
        .install_storage_plugin(
            FAILING_STORAGE_CARGO_PACKAGE,
            FAILING_STORAGE_PLUGIN_ID,
            &dist_dir,
            &harness.scoped_target_dir(&scope),
            PACKAGED_PLUGIN_TEST_HARNESS_TAG,
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?;

    let mut config = ServerConfig::default();
    config.bootstrap.plugins_dir = dist_dir;
    config.bootstrap.storage_profile = FAILING_STORAGE_PROFILE_ID.into();
    config.plugins.failure_policy.storage = failure_action;
    let mut allowlist =
        plugin_allowlist_with_supporting_plugins(&[JE_5_ADAPTER_ID], supporting_plugin_ids);
    allowlist.push(FAILING_STORAGE_PLUGIN_ID.to_string());
    config.plugins.allowlist = Some(allowlist);
    plugin_test_registries_from_config(&config)
}

pub(crate) fn gameplay_profile_map(
    entries: &[(&str, &str)],
) -> HashMap<revy_voxel_core::AdapterId, revy_voxel_core::GameplayProfileId> {
    entries
        .iter()
        .map(|(adapter_id, profile_id)| ((*adapter_id).into(), (*profile_id).into()))
        .collect()
}
