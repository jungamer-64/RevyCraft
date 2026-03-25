use super::*;
use crate::runtime::selection::SelectionResolver;
use mc_core::PlayerId;
use mc_plugin_api::codec::auth::AuthMode;
use mc_plugin_api::codec::gameplay::GameplaySessionSnapshot;
use mc_proto_common::ConnectionPhase;
use uuid::Uuid;

#[test]
fn selection_resolver_bootstrap_matches_reload_resolution() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into()]);
    config.admin.grpc.principals.insert(
        "ops".to_string(),
        crate::config::AdminGrpcPrincipalConfig {
            token_file: temp_dir.path().join("ops.token"),
            token: "ops-token".to_string(),
            permissions: vec![
                crate::config::AdminPermission::Status,
                crate::config::AdminPermission::ReloadConfig,
            ],
        },
    );

    let registries = plugin_test_registries_all()?;
    let bootstrap =
        SelectionResolver::resolve_bootstrap(&config, registries.loaded_plugins.clone())?;
    let reload = SelectionResolver::resolve(config.clone(), registries.loaded_plugins, &[])?;

    assert_eq!(bootstrap.selection.config, reload.config);
    assert_eq!(bootstrap.selection.auth_profile.mode()?, AuthMode::Offline);
    assert_eq!(
        bootstrap.selection.auth_profile.mode()?,
        reload.auth_profile.mode()?
    );
    assert!(bootstrap.selection.bedrock_auth_profile.is_none());
    assert!(reload.bedrock_auth_profile.is_none());
    assert_eq!(
        bootstrap
            .selection
            .admin_ui
            .as_ref()
            .map(|profile| profile.profile_id().clone()),
        reload
            .admin_ui
            .as_ref()
            .map(|profile| profile.profile_id().clone()),
    );
    assert_eq!(
        bootstrap
            .selection
            .remote_admin_subjects
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        reload
            .remote_admin_subjects
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
    );
    assert!(
        bootstrap
            .selection
            .loaded_plugins
            .resolve_gameplay_profile(config.profiles.default_gameplay.as_str())
            .is_some()
    );

    Ok(())
}

#[test]
fn selection_resolver_rejects_removing_active_gameplay_profile() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.profiles.default_gameplay = "readonly".into();
    config.profiles.gameplay_map.clear();

    let registries = plugin_test_registries_all()?;
    let error = match SelectionResolver::resolve(
        config,
        registries.loaded_plugins,
        &[GameplaySessionSnapshot {
            phase: ConnectionPhase::Play,
            player_id: Some(PlayerId(Uuid::nil())),
            entity_id: None,
            gameplay_profile: "canonical".into(),
        }],
    ) {
        Ok(_) => panic!("selection resolver should reject removing an in-use gameplay profile"),
        Err(error) => error,
    };

    assert!(
        matches!(error, RuntimeError::Config(message) if message.contains("cannot remove gameplay profile"))
    );
    Ok(())
}
