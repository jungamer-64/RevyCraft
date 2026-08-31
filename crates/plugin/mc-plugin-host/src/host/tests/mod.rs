use crate::config::{BootstrapConfig, RuntimeSelectionConfig};
use crate::host::plugin_host_from_config;
use mc_plugin_test_support::PackagedPluginHarness;

#[test]
fn packaged_abi9_plugins_load_through_the_production_host() -> Result<(), Box<dyn std::error::Error>>
{
    let harness = PackagedPluginHarness::shared()?;
    let bootstrap = BootstrapConfig {
        plugins_dir: harness.dist_dir().to_path_buf(),
        ..BootstrapConfig::default()
    };
    let host = plugin_host_from_config(&bootstrap)?.ok_or("packaged plugin catalog was empty")?;
    let loaded = host.load_plugin_set(&RuntimeSelectionConfig::default())?;

    assert!(!host.status().protocols.is_empty());
    assert!(loaded.protocols().resolve_adapter("je-5").is_some());
    assert!(loaded.resolve_gameplay_profile("canonical").is_some());
    assert!(loaded.resolve_storage_profile("je-anvil-1_7_10").is_some());
    assert!(loaded.resolve_auth_profile("offline-v1").is_some());
    assert!(loaded.resolve_admin_surface_profile("console-v1").is_some());
    Ok(())
}
