use super::*;

pub(crate) mod failing_storage_plugin {
    use mc_core::{CapabilitySet, WorldSnapshot};
    use mc_plugin_api::codec::storage::StorageDescriptor;
    use mc_plugin_sdk_rust::export_plugin;
    use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
    use mc_plugin_sdk_rust::storage::RustStoragePlugin;
    use mc_proto_common::StorageError;
    use std::path::Path;

    pub const PLUGIN_ID: &str = "storage-failing-runtime";
    pub const PROFILE_ID: &str = "failing-storage";

    #[derive(Default)]
    pub struct FailingStoragePlugin;

    impl RustStoragePlugin for FailingStoragePlugin {
        fn descriptor(&self) -> StorageDescriptor {
            StorageDescriptor {
                storage_profile: PROFILE_ID.to_string(),
            }
        }

        fn capability_set(&self) -> CapabilitySet {
            let mut capabilities = CapabilitySet::new();
            let _ = capabilities.insert("runtime.reload.storage");
            capabilities
        }

        fn load_snapshot(&self, _world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
            Ok(None)
        }

        fn save_snapshot(
            &self,
            _world_dir: &Path,
            _snapshot: &WorldSnapshot,
        ) -> Result<(), StorageError> {
            Err(StorageError::Plugin("storage runtime failure".to_string()))
        }
    }

    const MANIFEST: StaticPluginManifest = StaticPluginManifest::storage(
        PLUGIN_ID,
        "Failing Storage Plugin",
        &["storage.profile:failing-storage", "runtime.reload.storage"],
    );

    export_plugin!(storage, FailingStoragePlugin, MANIFEST);
}

pub(crate) const ALL_PROTOCOL_PLUGIN_IDS: &[&str] = &[
    JE_1_7_10_ADAPTER_ID,
    JE_1_8_X_ADAPTER_ID,
    JE_1_12_2_ADAPTER_ID,
    BE_26_3_ADAPTER_ID,
    BE_PLACEHOLDER_ADAPTER_ID,
];
pub(crate) const TCP_ONLY_PROTOCOL_PLUGIN_IDS: &[&str] = &[JE_1_7_10_ADAPTER_ID];
pub(crate) const GAMEPLAY_PLUGIN_IDS: &[&str] = &["gameplay-canonical", "gameplay-readonly"];
pub(crate) const ADMIN_UI_PLUGIN_IDS: &[&str] = &["admin-ui-console"];
pub(crate) const STORAGE_AND_AUTH_PLUGIN_IDS: &[&str] = &[
    "storage-je-anvil-1_7_10",
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
        ADMIN_UI_PLUGIN_IDS
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
        config.profiles.auth = ONLINE_STUB_AUTH_PROFILE_ID.to_string();
    }
    let plugin_host = TestPluginHost::discover(&config.plugin_host_config())?.ok_or_else(|| {
        RuntimeError::Config("packaged protocol plugins should be discovered".to_string())
    })?;
    Ok(LoadedPluginTestEnvironment {
        loaded_plugins: plugin_host.load_plugin_set(&config.plugin_host_config())?,
        plugin_host: Some(plugin_host),
    })
}

pub(crate) fn plugin_test_registries_from_config(
    config: &ServerConfig,
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let plugin_host = TestPluginHost::discover(&config.plugin_host_config())?.ok_or_else(|| {
        RuntimeError::Config("packaged protocol plugins should be discovered".to_string())
    })?;
    Ok(LoadedPluginTestEnvironment {
        loaded_plugins: plugin_host.load_plugin_set(&config.plugin_host_config())?,
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
    plugin_ids.extend_from_slice(ADMIN_UI_PLUGIN_IDS);
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

pub(crate) fn register_in_process_protocol_adapter(
    builder: TestPluginHostBuilder,
    adapter_id: &str,
) -> Result<TestPluginHostBuilder, RuntimeError> {
    let plugin = match adapter_id {
        JE_1_7_10_ADAPTER_ID => InProcessProtocolPlugin {
            plugin_id: JE_1_7_10_ADAPTER_ID.to_string(),
            manifest: je_1_7_10_entrypoints().manifest,
            api: je_1_7_10_entrypoints().api,
        },
        JE_1_8_X_ADAPTER_ID => InProcessProtocolPlugin {
            plugin_id: JE_1_8_X_ADAPTER_ID.to_string(),
            manifest: je_1_8_x_entrypoints().manifest,
            api: je_1_8_x_entrypoints().api,
        },
        JE_1_12_2_ADAPTER_ID => InProcessProtocolPlugin {
            plugin_id: JE_1_12_2_ADAPTER_ID.to_string(),
            manifest: je_1_12_2_entrypoints().manifest,
            api: je_1_12_2_entrypoints().api,
        },
        BE_26_3_ADAPTER_ID => InProcessProtocolPlugin {
            plugin_id: BE_26_3_ADAPTER_ID.to_string(),
            manifest: be_26_3_entrypoints().manifest,
            api: be_26_3_entrypoints().api,
        },
        BE_PLACEHOLDER_ADAPTER_ID => InProcessProtocolPlugin {
            plugin_id: BE_PLACEHOLDER_ADAPTER_ID.to_string(),
            manifest: be_placeholder_entrypoints().manifest,
            api: be_placeholder_entrypoints().api,
        },
        other => {
            return Err(RuntimeError::Config(format!(
                "unknown in-process adapter `{other}`"
            )));
        }
    };
    Ok(builder.protocol_raw(plugin))
}

pub(crate) fn register_in_process_supporting_plugins(
    builder: TestPluginHostBuilder,
) -> TestPluginHostBuilder {
    builder
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
            plugin_id: ONLINE_STUB_AUTH_PLUGIN_ID.to_string(),
            manifest: online_stub_auth_entrypoints().manifest,
            api: online_stub_auth_entrypoints().api,
        })
}

pub(crate) fn in_process_online_auth_registries(
    allowlist: &[&str],
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let mut builder = TestPluginHostBuilder::new();
    for adapter_id in allowlist {
        builder = register_in_process_protocol_adapter(builder, adapter_id)?;
    }
    let plugin_host = register_in_process_supporting_plugins(builder)
        .abi_range(PluginAbiRange::default())
        .failure_matrix(PluginFailureMatrix::default())
        .build();
    let mut config = ServerConfig::default();
    config.profiles.auth = ONLINE_STUB_AUTH_PROFILE_ID.to_string();
    Ok(LoadedPluginTestEnvironment {
        loaded_plugins: plugin_host.load_plugin_set(&config.plugin_host_config())?,
        plugin_host: Some(plugin_host),
    })
}

pub(crate) fn in_process_failing_storage_registries(
    failure_action: PluginFailureAction,
) -> Result<LoadedPluginTestEnvironment, RuntimeError> {
    let builder =
        register_in_process_protocol_adapter(TestPluginHostBuilder::new(), JE_1_7_10_ADAPTER_ID)?;
    let plugin_host = builder
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
            plugin_id: failing_storage_plugin::PLUGIN_ID.to_string(),
            manifest: failing_storage_plugin::in_process_plugin_entrypoints().manifest,
            api: failing_storage_plugin::in_process_plugin_entrypoints().api,
        })
        .auth_raw(InProcessAuthPlugin {
            plugin_id: "auth-offline".to_string(),
            manifest: offline_auth_entrypoints().manifest,
            api: offline_auth_entrypoints().api,
        })
        .bootstrap_config(mc_plugin_host::config::BootstrapConfig {
            storage_profile: failing_storage_plugin::PROFILE_ID.to_string(),
            ..mc_plugin_host::config::BootstrapConfig::default()
        })
        .abi_range(PluginAbiRange::default())
        .failure_matrix(PluginFailureMatrix {
            storage: failure_action,
            ..PluginFailureMatrix::default()
        })
        .build();
    let mut config = ServerConfig::default();
    config.bootstrap.storage_profile = failing_storage_plugin::PROFILE_ID.to_string();
    Ok(LoadedPluginTestEnvironment {
        loaded_plugins: plugin_host.load_plugin_set(&config.plugin_host_config())?,
        plugin_host: Some(plugin_host),
    })
}

pub(crate) fn gameplay_profile_map(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(adapter_id, profile_id)| ((*adapter_id).to_string(), (*profile_id).to_string()))
        .collect()
}
