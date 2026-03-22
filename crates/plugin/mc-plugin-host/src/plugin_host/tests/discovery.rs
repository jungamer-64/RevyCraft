use super::*;

#[test]
fn in_process_protocol_plugin_swaps_generation() {
    let entrypoints = in_process_protocol_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().protocol_raw(InProcessProtocolPlugin {
            plugin_id: "je-1_7_10".to_string(),
            manifest: entrypoints.manifest,
            api: entrypoints.api,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix {
            protocol: PluginFailureAction::FailFast,
            ..PluginFailureMatrix::default()
        },
    );
    let registries = host
        .load_protocol_plugin_set()
        .expect("in-process plugin should load");

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("registered plugin adapter should resolve");
    let first_generation = adapter
        .plugin_generation_id()
        .expect("plugin-backed adapter should report generation");

    let next_generation = host
        .replace_in_process_protocol_plugin(InProcessProtocolPlugin {
            plugin_id: "je-1_7_10".to_string(),
            manifest: entrypoints.manifest,
            api: entrypoints.api,
        })
        .expect("replacing in-process plugin should succeed");

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("registered plugin adapter should resolve");
    assert_eq!(adapter.plugin_generation_id(), Some(next_generation));
    assert_ne!(first_generation, next_generation);
}

#[test]
fn discover_rejects_duplicate_plugin_ids() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    for directory in ["first", "second"] {
        let plugin_dir = temp_dir.path().join(directory);
        fs::create_dir_all(&plugin_dir)?;
        fs::write(
            plugin_dir.join("plugin.toml"),
            format!(
                "[plugin]\nid = \"duplicate-plugin\"\nkind = \"protocol\"\n\n[artifacts]\n\"{}\" = \"libduplicate.so\"\n",
                current_artifact_key()
            ),
        )?;
    }

    let error = match plugin_host_from_config(
        &ServerConfig {
            plugins_dir: temp_dir.path().to_path_buf(),
            ..ServerConfig::default()
        }
        .bootstrap_config(),
    ) {
        Ok(_) => panic!("duplicate plugin ids should fail discovery"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("duplicate plugin id `duplicate-plugin`")
    ));
    Ok(())
}

#[test]
fn all_protocol_plugins_register_and_resolve() {
    let mut builder = TestPluginHostBuilder::new();
    for (plugin_id, entrypoints) in [
        ("je-1_7_10", in_process_protocol_entrypoints()),
        ("je-1_8_x", je_1_8_x_entrypoints()),
        ("je-1_12_2", je_1_12_2_entrypoints()),
        ("be-placeholder", be_placeholder_entrypoints()),
    ] {
        builder = builder.protocol_raw(InProcessProtocolPlugin {
            plugin_id: plugin_id.to_string(),
            manifest: entrypoints.manifest,
            api: entrypoints.api,
        });
    }

    let host = build_test_plugin_host(
        builder,
        PluginAbiRange::default(),
        PluginFailureMatrix {
            protocol: PluginFailureAction::FailFast,
            ..PluginFailureMatrix::default()
        },
    );
    let registries = host
        .load_protocol_plugin_set()
        .expect("protocol plugins should load");

    for adapter_id in ["je-1_7_10", "je-1_8_x", "je-1_12_2", "be-placeholder"] {
        assert!(
            registries.protocols().resolve_adapter(adapter_id).is_some(),
            "adapter `{adapter_id}` should resolve"
        );
    }

    let je_handshake = je_handshake_frame(340);
    let je_intent = registries
        .protocols()
        .route_handshake(TransportKind::Tcp, &je_handshake)
        .expect("tcp probe should not fail")
        .expect("tcp handshake should resolve");
    assert_eq!(je_intent.edition, Edition::Je);
    assert_eq!(je_intent.protocol_number, 340);

    let be_intent = registries
        .protocols()
        .route_handshake(TransportKind::Udp, &raknet_unconnected_ping())
        .expect("udp probe should not fail")
        .expect("udp datagram should resolve");
    assert_eq!(be_intent.edition, Edition::Be);
}

#[test]
fn protocol_plugins_preserve_wire_format_and_optional_bedrock_listener_metadata() {
    let mut builder = TestPluginHostBuilder::new();
    for (plugin_id, entrypoints) in [
        ("je-1_7_10", in_process_protocol_entrypoints()),
        ("be-26_3", be_26_3_entrypoints()),
        ("be-placeholder", be_placeholder_entrypoints()),
    ] {
        builder = builder.protocol_raw(InProcessProtocolPlugin {
            plugin_id: plugin_id.to_string(),
            manifest: entrypoints.manifest,
            api: entrypoints.api,
        });
    }

    let host = build_test_plugin_host(
        builder,
        PluginAbiRange::default(),
        PluginFailureMatrix {
            protocol: PluginFailureAction::FailFast,
            ..PluginFailureMatrix::default()
        },
    );
    let registries = host
        .load_protocol_plugin_set()
        .expect("protocol plugins should load");

    let je_adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("je adapter should resolve");
    assert_eq!(
        je_adapter.descriptor().wire_format,
        WireFormatKind::MinecraftFramed
    );
    assert!(je_adapter.bedrock_listener_descriptor().is_none());

    let bedrock_adapter = registries
        .protocols()
        .resolve_adapter("be-26_3")
        .expect("bedrock adapter should resolve");
    assert_eq!(
        bedrock_adapter.descriptor().wire_format,
        WireFormatKind::RawPacketStream
    );
    assert!(bedrock_adapter.bedrock_listener_descriptor().is_some());

    let placeholder_adapter = registries
        .protocols()
        .resolve_adapter("be-placeholder")
        .expect("placeholder adapter should resolve");
    assert_eq!(
        placeholder_adapter.descriptor().wire_format,
        WireFormatKind::RawPacketStream
    );
    assert!(placeholder_adapter.bedrock_listener_descriptor().is_none());
}

#[test]
fn protocol_plugins_can_override_host_wire_codec_framing() {
    use bytes::BytesMut;

    let entrypoints = custom_wire_codec_protocol_plugin::in_process_plugin_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().protocol_raw(InProcessProtocolPlugin {
            plugin_id: "protocol-custom-wire".to_string(),
            manifest: entrypoints.manifest,
            api: entrypoints.api,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix {
            protocol: PluginFailureAction::FailFast,
            ..PluginFailureMatrix::default()
        },
    );
    let registries = host
        .load_protocol_plugin_set()
        .expect("custom wire codec plugin should load");

    let adapter = registries
        .protocols()
        .resolve_adapter("protocol-custom-wire")
        .expect("custom wire codec adapter should resolve");
    assert_eq!(
        adapter.descriptor().wire_format,
        WireFormatKind::MinecraftFramed,
        "descriptor stays stable even when framing is plugin-defined"
    );

    let encoded = adapter
        .wire_codec()
        .encode_frame(&[0xaa, 0xbb, 0xcc])
        .expect("custom wire frame should encode");
    assert_eq!(encoded, vec![0xc0, 3, 0xaa, 0xbb, 0xcc]);

    let mut buffer = BytesMut::from(&encoded[..]);
    let decoded = adapter
        .wire_codec()
        .try_decode_frame(&mut buffer)
        .expect("custom wire frame should decode");
    assert_eq!(decoded, Some(vec![0xaa, 0xbb, 0xcc]));
    assert!(
        buffer.is_empty(),
        "decoded frame should consume buffered bytes"
    );
}

#[test]
fn abi_mismatch_is_rejected_before_registration() {
    let entrypoints = in_process_protocol_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().protocol_raw(InProcessProtocolPlugin {
            plugin_id: "je-1_7_10".to_string(),
            manifest: manifest_with_abi("je-1_7_10", PluginAbiVersion { major: 9, minor: 0 }),
            api: entrypoints.api,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix {
            protocol: PluginFailureAction::FailFast,
            ..PluginFailureMatrix::default()
        },
    );
    let error = match host.load_protocol_plugin_set() {
        Ok(_) => panic!("ABI mismatch should fail before registration"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        RuntimeError::PluginFatal(message) if message.contains("ABI")
    ));
}

#[test]
fn protocol_plugins_require_reload_manifest_capability() {
    let entrypoints = in_process_protocol_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().protocol_raw(InProcessProtocolPlugin {
            plugin_id: "je-1_7_10".to_string(),
            manifest: manifest_without_reload_capability("je-1_7_10"),
            api: entrypoints.api,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix {
            protocol: PluginFailureAction::FailFast,
            ..PluginFailureMatrix::default()
        },
    );
    let error = match host.load_protocol_plugin_set() {
        Ok(_) => panic!("protocol plugin without runtime.reload.protocol should fail"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        RuntimeError::PluginFatal(message) if message.contains("runtime.reload.protocol")
    ));
}
