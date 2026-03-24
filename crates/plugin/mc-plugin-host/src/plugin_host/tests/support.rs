use super::*;

pub(super) const PACKAGED_PLUGIN_TEST_HARNESS_TAG: &str = "runtime-test-harness";

pub(super) fn seed_packaged_plugins(
    dist_dir: &Path,
    plugin_ids: &[&str],
) -> Result<(), RuntimeError> {
    PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .seed_subset(dist_dir, plugin_ids)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(super) fn build_test_plugin_host(
    builder: TestPluginHostBuilder,
    abi_range: PluginAbiRange,
    failure_matrix: PluginFailureMatrix,
) -> TestPluginHost {
    builder
        .abi_range(abi_range)
        .failure_matrix(failure_matrix)
        .build()
}

pub(super) fn manifest_with_abi(
    plugin_id: &'static str,
    plugin_abi: PluginAbiVersion,
) -> &'static PluginManifestV1 {
    let capabilities = Box::leak(
        vec![CapabilityDescriptorV1 {
            name: Utf8Slice::from_static_str("runtime.reload.protocol"),
        }]
        .into_boxed_slice(),
    );
    Box::leak(Box::new(PluginManifestV1 {
        plugin_id: Utf8Slice::from_static_str(plugin_id),
        display_name: Utf8Slice::from_static_str(plugin_id),
        plugin_kind: PluginKind::Protocol,
        plugin_abi,
        min_host_abi: CURRENT_PLUGIN_ABI,
        max_host_abi: CURRENT_PLUGIN_ABI,
        capabilities: capabilities.as_ptr(),
        capabilities_len: capabilities.len(),
    }))
}

pub(super) fn manifest_without_reload_capability(
    plugin_id: &'static str,
) -> &'static PluginManifestV1 {
    Box::leak(Box::new(PluginManifestV1 {
        plugin_id: Utf8Slice::from_static_str(plugin_id),
        display_name: Utf8Slice::from_static_str(plugin_id),
        plugin_kind: PluginKind::Protocol,
        plugin_abi: CURRENT_PLUGIN_ABI,
        min_host_abi: CURRENT_PLUGIN_ABI,
        max_host_abi: CURRENT_PLUGIN_ABI,
        capabilities: std::ptr::null(),
        capabilities_len: 0,
    }))
}

pub(super) fn protocol_reload_context(
    sessions: Vec<ProtocolReloadSession>,
) -> RuntimeReloadContext {
    RuntimeReloadContext {
        protocol_sessions: sessions,
        gameplay_sessions: Vec::new(),
        snapshot: ServerCore::new(CoreConfig::default()).snapshot(),
        world_dir: PathBuf::from("."),
    }
}

pub(super) fn protocol_reload_session(
    connection_id: u64,
    phase: ConnectionPhase,
    player_id: Option<PlayerId>,
    entity_id: Option<EntityId>,
) -> ProtocolReloadSession {
    ProtocolReloadSession {
        adapter_id: "je-5".to_string(),
        session: ProtocolSessionSnapshot {
            connection_id: ConnectionId(connection_id),
            phase,
            player_id,
            entity_id,
        },
    }
}

pub(super) struct StubGameplayQuery {
    pub(super) level_name: &'static str,
}

impl GameplayQuery for StubGameplayQuery {
    fn world_meta(&self) -> WorldMeta {
        WorldMeta {
            level_name: self.level_name.to_string(),
            seed: 0,
            spawn: BlockPos::new(0, 64, 0),
            dimension: DimensionId::Overworld,
            age: 0,
            time: 0,
            level_type: "FLAT".to_string(),
            game_mode: 0,
            difficulty: 1,
            max_players: 20,
        }
    }

    fn player_snapshot(&self, _player_id: PlayerId) -> Option<mc_core::PlayerSnapshot> {
        None
    }

    fn block_state(&self, _position: BlockPos) -> BlockState {
        BlockState::air()
    }

    fn block_entity(&self, _position: BlockPos) -> Option<mc_core::BlockEntityState> {
        None
    }

    fn can_edit_block(&self, _player_id: PlayerId, _position: BlockPos) -> bool {
        false
    }
}

pub(super) fn je_handshake_frame(protocol_version: i32) -> Vec<u8> {
    let mut writer = PacketWriter::default();
    writer.write_varint(0);
    writer.write_varint(protocol_version);
    writer
        .write_string("localhost")
        .expect("handshake host should encode");
    writer.write_u16(25565);
    writer.write_varint(2);
    writer.into_inner()
}

pub(super) fn raknet_unconnected_ping() -> Vec<u8> {
    let mut frame = Vec::with_capacity(33);
    frame.push(0x01);
    frame.extend_from_slice(&123_i64.to_be_bytes());
    frame.extend_from_slice(&[
        0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56,
        0x78,
    ]);
    frame.extend_from_slice(&456_i64.to_be_bytes());
    frame
}
