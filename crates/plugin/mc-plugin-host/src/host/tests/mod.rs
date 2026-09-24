use crate::config::{BootstrapConfig, RuntimeSelectionConfig};
use crate::host::plugin_host_from_config;
use crate::runtime::RuntimePluginHost;
use mc_plugin_test_support::PackagedPluginHarness;

#[test]
fn independently_loaded_protocols_keep_session_state_isolated()
-> Result<(), Box<dyn std::error::Error>> {
    use mc_proto_common::{ConnectionPhase, PlayEncodingContext, ProtocolSessionSnapshot};
    use revy_voxel_semantic::{
        ConnectionId, CoreEvent, EntityId, InventoryTransactionContext, PlayerId,
    };

    let harness = PackagedPluginHarness::shared()?;
    let bootstrap = BootstrapConfig {
        plugins_dir: harness.dist_dir().to_path_buf(),
        ..BootstrapConfig::default()
    };
    let first_host = plugin_host_from_config(&bootstrap)?.ok_or("empty catalog")?;
    let second_host = plugin_host_from_config(&bootstrap)?.ok_or("empty catalog")?;
    let first = first_host.load_plugin_set(&RuntimeSelectionConfig::default())?;
    let second = second_host.load_plugin_set(&RuntimeSelectionConfig::default())?;
    let first = first
        .protocols()
        .resolve_adapter("je-340")
        .ok_or("missing adapter")?;
    let second = second
        .protocols()
        .resolve_adapter("je-340")
        .ok_or("missing adapter")?;
    let player_id = PlayerId(uuid::Uuid::from_u128(1));
    let session = ProtocolSessionSnapshot {
        connection_id: ConnectionId(987654),
        phase: ConnectionPhase::Play,
        player_id: Some(player_id),
        entity_id: Some(EntityId(1)),
    };
    let initial = second.export_session_state(&session)?;
    first.encode_play_event(
        &CoreEvent::InventoryTransactionProcessed {
            transaction: InventoryTransactionContext {
                window_id: 1,
                action_number: 10,
            },
            accepted: false,
        },
        &session,
        &PlayEncodingContext {
            player_id,
            entity_id: EntityId(1),
        },
    )?;
    let changed = first.export_session_state(&session)?;
    assert_ne!(changed, initial);
    assert_eq!(second.export_session_state(&session)?, initial);
    second.import_session_state(&session, &changed)?;
    first.session_closed(&session)?;
    assert_eq!(second.export_session_state(&session)?, changed);
    second.session_closed(&session)?;
    Ok(())
}

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
    let artifacts = host.exact_plugin_artifacts()?;
    assert!(!artifacts.is_empty());
    assert!(
        artifacts
            .windows(2)
            .all(|pair| pair[0].plugin_id < pair[1].plugin_id)
    );
    assert!(artifacts.iter().all(|artifact| artifact.sha256 != [0; 32]));
    Ok(())
}
