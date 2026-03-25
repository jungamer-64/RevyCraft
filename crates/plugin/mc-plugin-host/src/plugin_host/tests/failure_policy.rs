use super::*;

#[test]
fn protocol_runtime_failure_policy_matrix_controls_quarantine_and_fatal_behavior() {
    let cases = [
        (PluginFailureAction::Skip, false, false, "failing-runtime"),
        (PluginFailureAction::Quarantine, true, false, "quarantined"),
        (
            PluginFailureAction::FailFast,
            false,
            true,
            "failing-runtime",
        ),
    ];

    for (action, expect_quarantine, expect_fatal, expected_version_name) in cases {
        let entrypoints = failing_protocol_plugin::in_process_plugin_entrypoints();
        let host = build_test_plugin_host(
            TestPluginHostBuilder::new().protocol_raw(InProcessProtocolPlugin {
                plugin_id: failing_protocol_plugin::PLUGIN_ID.to_string(),
                manifest: entrypoints.manifest,
                api: entrypoints.api,
            }),
            PluginAbiRange::default(),
            PluginFailureMatrix {
                protocol: action,
                ..PluginFailureMatrix::default()
            },
        );
        let registries = host
            .load_protocol_plugin_set()
            .expect("failing protocol plugin should still register");

        let error = registries
            .protocols()
            .route_handshake(TransportKind::Tcp, &[0xde, 0xad, 0xbe, 0xef])
            .expect_err("runtime failure should surface from the protocol probe");
        assert!(matches!(
            error,
            mc_proto_common::ProtocolError::Plugin(message) if message.contains("protocol runtime failure")
        ));
        let status = host.status();
        assert_eq!(status.protocols.len(), 1);
        assert_eq!(status.protocols[0].failure_action, action);
        assert_eq!(
            status.protocols[0].active_quarantine_reason.is_some(),
            expect_quarantine
        );
        assert!(status.protocols[0].artifact_quarantine.is_none());
        assert_eq!(status.pending_fatal_error.is_some(), expect_fatal);
        assert_eq!(host.take_pending_fatal_error().is_some(), expect_fatal);
        let adapter = registries
            .protocols()
            .resolve_adapter(failing_protocol_plugin::PLUGIN_ID)
            .expect("protocol adapter should remain registered");
        assert_eq!(adapter.descriptor().version_name, expected_version_name);
    }
}

#[test]
fn gameplay_runtime_failure_policy_matrix_controls_noop_and_fatal_behavior() {
    use mc_core::{
        EntityId, GameplayCapabilitySet, GameplayCommand, GameplayProfileId, PlayerId,
        ProtocolCapabilitySet, SessionCapabilitySet,
    };

    let cases = [
        (PluginFailureAction::Skip, false, false),
        (PluginFailureAction::Quarantine, true, false),
        (PluginFailureAction::FailFast, false, true),
    ];

    for (action, expect_quarantine, expect_fatal) in cases {
        let entrypoints = failing_gameplay_plugin::in_process_plugin_entrypoints();
        let host = build_test_plugin_host(
            TestPluginHostBuilder::new().gameplay_raw(InProcessGameplayPlugin {
                plugin_id: "gameplay-failing".to_string(),
                manifest: entrypoints.manifest,
                api: entrypoints.api,
            }),
            PluginAbiRange::default(),
            PluginFailureMatrix {
                gameplay: action,
                ..PluginFailureMatrix::default()
            },
        );
        host.activate_gameplay_profiles(&RuntimeSelectionConfig {
            default_gameplay_profile: "failing".into(),
            ..runtime_selection_config()
        })
        .expect("failing gameplay profile should activate");

        let profile = host
            .resolve_gameplay_profile("failing")
            .expect("failing gameplay profile should resolve");
        let mut core = stub_server_core("world");
        let result = profile.handle_command(
            &mut core,
            &SessionCapabilitySet {
                protocol: ProtocolCapabilitySet::new(),
                gameplay: GameplayCapabilitySet::new(),
                gameplay_profile: GameplayProfileId::new("failing"),
                entity_id: Some(EntityId(9)),
                protocol_generation: None,
                gameplay_generation: None,
            },
            &GameplayCommand::SetHeldSlot {
                player_id: PlayerId(Uuid::from_u128(77)),
                slot: 0,
            },
            0,
        );
        match action {
            PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                assert!(
                    result.is_ok(),
                    "non-fatal gameplay policy should downgrade runtime failures to no-op"
                );
            }
            PluginFailureAction::FailFast => {
                assert!(
                    matches!(result, Err(RuntimeError::Config(message)) if message.contains("gameplay runtime failure"))
                );
            }
        }
        let status = host.status();
        assert_eq!(status.gameplay.len(), 1);
        assert_eq!(status.gameplay[0].failure_action, action);
        assert_eq!(
            status.gameplay[0].active_quarantine_reason.is_some(),
            expect_quarantine
        );
        assert_eq!(status.pending_fatal_error.is_some(), expect_fatal);
        assert_eq!(host.take_pending_fatal_error().is_some(), expect_fatal);
        if action == PluginFailureAction::Quarantine {
            assert!(
                profile
                    .handle_command(
                        &mut core,
                        &SessionCapabilitySet {
                            protocol: ProtocolCapabilitySet::new(),
                            gameplay: GameplayCapabilitySet::new(),
                            gameplay_profile: GameplayProfileId::new("failing"),
                            entity_id: Some(EntityId(9)),
                            protocol_generation: None,
                            gameplay_generation: None,
                        },
                        &GameplayCommand::SetHeldSlot {
                            player_id: PlayerId(Uuid::from_u128(77)),
                            slot: 1,
                        },
                        0,
                    )
                    .is_ok(),
                "quarantined gameplay profile should no-op future hooks"
            );
        }
    }
}

#[test]
fn auth_runtime_failure_policy_matrix_controls_fatal_behavior() {
    let cases = [
        (PluginFailureAction::Skip, false),
        (PluginFailureAction::FailFast, true),
    ];

    for (action, expect_fatal) in cases {
        let entrypoints = failing_auth_plugin::in_process_plugin_entrypoints();
        let host = build_test_plugin_host(
            TestPluginHostBuilder::new().auth_raw(InProcessAuthPlugin {
                plugin_id: "auth-failing".to_string(),
                manifest: entrypoints.manifest,
                api: entrypoints.api,
            }),
            PluginAbiRange::default(),
            PluginFailureMatrix {
                auth: action,
                ..PluginFailureMatrix::default()
            },
        );
        host.activate_auth_profile(failing_auth_plugin::PROFILE_ID)
            .expect("failing auth profile should activate");
        let profile = host
            .resolve_auth_profile(failing_auth_plugin::PROFILE_ID)
            .expect("failing auth profile should resolve");

        let error = profile
            .authenticate_offline("tester")
            .expect_err("failing auth should reject the current login attempt");
        assert!(matches!(
            error,
            RuntimeError::Config(message) if message.contains("auth runtime failure")
        ));
        let status = host.status();
        assert_eq!(status.auth.len(), 1);
        assert_eq!(status.auth[0].failure_action, action);
        assert!(status.auth[0].active_quarantine_reason.is_none());
        assert_eq!(status.pending_fatal_error.is_some(), expect_fatal);
        assert_eq!(host.take_pending_fatal_error().is_some(), expect_fatal);
    }
}
