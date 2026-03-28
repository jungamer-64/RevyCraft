use super::*;

#[test]
fn protocol_registry_resolves_registered_adapter() -> Result<(), RuntimeError> {
    let registry = plugin_test_registries_tcp_only()?;
    let by_id = registry
        .protocols()
        .resolve_adapter(JE_5_ADAPTER_ID)
        .expect("registered adapter should resolve by id");
    let by_route = registry
        .protocols()
        .resolve_route(
            TransportKind::Tcp,
            Edition::Je,
            TestJavaProtocol::Je5.protocol_version(),
        )
        .expect("registered adapter should resolve by route");

    assert_eq!(by_id.descriptor().adapter_id, JE_5_ADAPTER_ID);
    assert_eq!(by_id.descriptor().transport, TransportKind::Tcp);
    assert_eq!(by_route.descriptor().version_name, "1.7.10");
    Ok(())
}

#[test]
fn handshake_probe_transport_kind_filters_routing() -> Result<(), RuntimeError> {
    let registry = plugin_test_registries_all()?;
    let tcp_route = registry
        .protocols()
        .route_handshake(TransportKind::Tcp, &raknet_unconnected_ping())
        .expect("tcp routing should not fail");
    let udp_route = registry
        .protocols()
        .route_handshake(TransportKind::Udp, &raknet_unconnected_ping())
        .expect("udp routing should not fail");

    assert!(tcp_route.is_none());
    assert!(udp_route.is_some());
    Ok(())
}

#[test]
fn listener_plan_includes_tcp_binding_and_registered_adapter() -> Result<(), RuntimeError> {
    let registries = plugin_test_registries_tcp_only()?;
    let plans = build_listener_plans(&ServerConfig::default(), registries.protocols())?;

    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].transport, TransportKind::Tcp);
    assert!(
        plans[0]
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == JE_5_ADAPTER_ID)
    );
    Ok(())
}

#[test]
fn listener_plan_includes_udp_binding_when_bedrock_is_enabled() -> Result<(), RuntimeError> {
    let registries = plugin_test_registries_all()?;
    let mut config = ServerConfig::default();
    config.topology.be_enabled = true;
    let plans = build_listener_plans(&config, registries.protocols())?;

    assert_eq!(plans.len(), 2);
    assert_eq!(plans[1].transport, TransportKind::Udp);
    assert!(
        plans[1]
            .adapter_ids
            .iter()
            .any(|adapter_id| adapter_id == BE_924_ADAPTER_ID)
    );
    let default_bedrock_adapter = registries
        .protocols()
        .resolve_adapter(BE_924_ADAPTER_ID)
        .expect("default bedrock adapter should resolve");
    let expected_bedrock_listener = default_bedrock_adapter
        .bedrock_listener_descriptor()
        .expect("bedrock adapter should expose listener metadata");
    let expected_descriptor = default_bedrock_adapter.descriptor();
    let bedrock_metadata = plans[1]
        .bedrock_bind_metadata
        .as_ref()
        .expect("udp listener plan should keep bedrock metadata");
    assert_eq!(
        bedrock_metadata.protocol_number,
        expected_descriptor.protocol_number
    );
    assert_eq!(
        bedrock_metadata.raknet_version,
        expected_bedrock_listener.raknet_version
    );
    assert_eq!(
        bedrock_metadata.game_version,
        expected_bedrock_listener.game_version
    );
    Ok(())
}

#[test]
fn plugin_host_preserves_wire_format_per_adapter() -> Result<(), RuntimeError> {
    let registries = plugin_test_registries_all()?;
    let je_adapter = registries
        .protocols()
        .resolve_adapter(JE_5_ADAPTER_ID)
        .expect("je adapter should resolve");
    let bedrock_adapter = registries
        .protocols()
        .resolve_adapter(BE_924_ADAPTER_ID)
        .expect("bedrock adapter should resolve");

    assert_eq!(
        je_adapter.descriptor().wire_format,
        WireFormatKind::MinecraftFramed
    );
    assert_eq!(
        bedrock_adapter.descriptor().wire_format,
        WireFormatKind::RawPacketStream
    );
    assert_eq!(
        je_adapter.wire_codec().encode_frame(&[1, 2, 3])?,
        vec![3, 1, 2, 3]
    );
    assert_eq!(
        bedrock_adapter.wire_codec().encode_frame(&[1, 2, 3])?,
        vec![1, 2, 3]
    );
    Ok(())
}

#[test]
fn default_wire_codec_requires_adapter_for_udp_sessions() {
    assert!(matches!(
        default_wire_codec(TransportKind::Udp),
        Err(RuntimeError::Config(message))
            if message.contains("udp sessions require an active protocol adapter")
    ));
}
