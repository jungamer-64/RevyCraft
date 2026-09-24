use super::*;

#[tokio::test]
async fn executable_precopy_refreshes_an_outpaced_journal_before_freeze() -> Result<(), RuntimeError>
{
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.plugins_dir = PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .dist_dir()
        .to_path_buf();
    let server = crate::runtime::ServerSupervisor::boot(ServerConfigSource::Inline(config))
        .await?
        .running;
    let codec = MinecraftWireCodec;
    let (stream, _) = connect_and_login_java_client(
        listener_addr(&server),
        &codec,
        TestJavaProtocol::Je5,
        "precopy-budget",
    )
    .await?;
    let player_id = server.session_status().await[0]
        .player_id
        .expect("logged-in player");
    let arena = Arc::new(
        revy_runtime_transfer::SharedTransferArena::create(
            64 * 1024 * 1024,
            1,
            revy_runtime_transfer::TransferId::from_bytes([11; 16]),
            [12; 32],
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?,
    );
    let preparing = server
        .runtime
        .prepare_executable_upgrade(Arc::clone(&arena))
        .await?
        .begin_preparing()
        .await?;
    let base_revision = preparing.core_snapshot_revision_value();
    let core = &server.runtime.authority.active().core;
    // More committed gameplay mutations than the bounded journal can retain. These are core
    // commands, not clock advancement or a fault-injection hook.
    for index in 0..=revy_voxel_core::CoreTransferDelta::MAX_COMMITS {
        let outcome = core
            .apply_command(
                revy_voxel_core::CoreCommand::Gameplay(
                    revy_voxel_core::GameplayCommand::SetHeldSlot {
                        player_id,
                        slot: (index % 8) as i16,
                    },
                ),
                crate::runtime::CoreInvocation::Internal,
                crate::runtime::now_ms(),
            )
            .await?;
        assert!(matches!(
            outcome,
            crate::runtime::CoreCommandOutcome::Events(_)
        ));
    }
    let update = preparing.prestage_core(base_revision).await?;
    assert_eq!(
        update.kind(),
        revy_runtime_transfer::ResourceKindV1::CoreSnapshot
    );
    assert!(update.revision() > base_revision);
    let copied = revy_voxel_core::CoreVersion::import_process_transfer(
        update.region().as_slice(),
        crate::runtime::selection::SelectionResolver::content_behavior(),
    )
    .map_err(|error| RuntimeError::Config(error.to_string()))?;
    assert_eq!(copied.revision().value(), update.revision());
    let used_before_delta = arena.used();
    let delta = preparing.prestage_core(update.revision()).await?;
    assert!(
        arena.used() - used_before_delta <= delta.region().length() + 7,
        "preparing retains only the encoded delta plus alignment, not its maximum capacity"
    );
    assert_eq!(
        delta.kind(),
        revy_runtime_transfer::ResourceKindV1::CoreJournal
    );
    let copied = copied
        .apply_process_transfer_delta(delta.region().as_slice())
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    assert_eq!(copied.revision().value(), delta.revision());
    assert!(preparing.prestage_core(u64::MAX).await.is_err());
    preparing.abort_preparation().await?;
    drop(stream);
    server.shutdown().await
}

#[tokio::test]
async fn executable_stage_precopies_core_and_exposes_only_bounded_final_delta()
-> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let mut config = loopback_server_config(temp_dir.path().join("world"));
    config.bootstrap.game_mode = 1;
    config.topology.be_enabled = true;
    config.topology.enabled_adapters = Some(vec![JE_5_ADAPTER_ID.into()]);
    config.topology.default_bedrock_adapter = BE_924_ADAPTER_ID.into();
    config.topology.enabled_bedrock_adapters = Some(vec![BE_924_ADAPTER_ID.into()]);
    config.profiles.bedrock_auth = BEDROCK_OFFLINE_AUTH_PROFILE_ID.into();
    config.bootstrap.plugins_dir = PackagedPluginHarness::shared()
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .dist_dir()
        .to_path_buf();
    let child_config_source = crate::config::ServerConfigSource::Inline(config.clone());
    let server = crate::runtime::ServerSupervisor::boot(child_config_source.clone())
        .await?
        .running;
    let addr = listener_addr(&server);
    let codec = MinecraftWireCodec;
    let mut bedrock = BedrockTestClient::connect(udp_listener_addr(&server)).await?;
    bedrock.login("xfer-bedrock").await?;
    let _ = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::StartGame, 32).await?;
    let _ = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::LevelChunk, 64).await?;
    let (mut stream, mut buffer) =
        connect_and_login_java_client(addr, &codec, TestJavaProtocol::Je5, "executable-delta")
            .await?;
    let arena = Arc::new(
        revy_runtime_transfer::SharedTransferArena::create(
            64 * 1024 * 1024,
            1,
            revy_runtime_transfer::TransferId::from_bytes([3; 16]),
            [5; 32],
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?,
    );
    let staged = server
        .runtime
        .prepare_executable_upgrade(Arc::clone(&arena))
        .await?;
    assert_eq!(
        server.runtime.authority.executable_upgrade_status(),
        Some(crate::RuntimeUpgradeStateView {
            role: crate::RuntimeUpgradeRole::Parent,
            phase: crate::RuntimeUpgradePhase::ParentStaging,
        })
    );
    let preparing = staged.begin_preparing().await?;
    let base_revision = preparing.core_snapshot_revision_value();
    let staged_directory_revision = preparing.staged_directory_revision();

    assert!(!preparing.plugin_artifacts().is_empty());
    assert!(
        preparing
            .plugin_artifacts()
            .windows(2)
            .all(|pair| pair[0].plugin_id < pair[1].plugin_id)
    );
    assert!(
        preparing
            .plugin_artifacts()
            .iter()
            .all(|artifact| artifact.sha256 != [0; 32])
    );
    let decoded_directory = crate::runtime::ExecutableSessionDirectory::decode(
        preparing.initial_directory().region().as_slice(),
    )?;
    assert_eq!(
        decoded_directory.revision(),
        preparing.staged_directory_revision()
    );
    assert_eq!(decoded_directory.sessions().len(), 2);
    let persisted_at_stage = preparing.persisted_core_revision_value();
    let core_snapshot = preparing.core_snapshot_region().as_slice().to_vec();
    let arena_descriptor = arena.descriptor();
    #[cfg(unix)]
    let mut child_arena = revy_runtime_transfer::SharedTransferArenaReader::from_descriptor(
        arena
            .duplicate_descriptor()
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
        arena_descriptor.capacity,
        arena_descriptor.used,
        arena_descriptor.generation,
    )
    .map_err(|error| RuntimeError::Config(error.to_string()))?;
    #[cfg(windows)]
    let mut child_arena = revy_runtime_transfer::SharedTransferArenaReader::open(
        &arena.mapping_name(),
        arena_descriptor.capacity,
        arena_descriptor.used,
        arena_descriptor.generation,
    )
    .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let bootstrap = revy_runtime_transfer::BootstrapV1 {
        validated_config_digest: preparing.validated_config_digest().to_vec(),
        plugin_artifacts: preparing
            .plugin_artifacts()
            .iter()
            .map(|artifact| revy_runtime_transfer::PluginArtifactV1 {
                plugin_id: artifact.plugin_id.clone(),
                sha256: artifact.sha256.to_vec(),
            })
            .collect(),
        arena: Some(arena_descriptor),
        resources: vec![
            transfer_resource(
                1,
                revy_runtime_transfer::ResourceKindV1::CoreSnapshot,
                preparing.core_snapshot_region(),
            ),
            transfer_resource(
                2,
                revy_runtime_transfer::ResourceKindV1::SessionDirectory,
                preparing.initial_directory().region(),
            ),
        ],
        native_handle_count: 0,
        core_snapshot_revision: preparing.core_snapshot_revision_value(),
        persisted_core_revision: preparing.persisted_core_revision_value(),
        latest_dirty_core_revision: preparing.latest_dirty_core_revision_value(),
        directory_revision: preparing.staged_directory_revision(),
        active_generation_id: preparing.active_generation_id_value(),
    };
    let mut child = crate::runtime::ServerSupervisor::prepare_executable_child(
        child_config_source,
        &bootstrap,
        &child_arena,
    )?;
    assert_eq!(child.core_revision().value(), base_revision);
    assert_eq!(child.directory_revision(), staged_directory_revision);
    write_packet(&mut stream, &codec, &held_item_change(5)).await?;
    let _ = read_until_held_item_change(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        5,
        16,
    )
    .await?;
    let current_directory = preparing.prestage_directory().await?;
    let decoded_current_directory =
        crate::runtime::ExecutableSessionDirectory::decode(current_directory.region().as_slice())?;
    assert_eq!(
        decoded_current_directory.revision(),
        preparing.current_directory_revision()
    );
    assert_eq!(decoded_current_directory.sessions().len(), 2);
    let network = preparing
        .prepare_network_transfer(
            revy_runtime_transfer::SocketTransferTarget::new(std::process::id())
                .map_err(|error| RuntimeError::Config(error.to_string()))?,
        )
        .await?;
    assert_eq!(
        network.directory_revision(),
        current_directory.directory().revision()
    );
    let mut imported_transports = network
        .listeners()
        .iter()
        .map(|resource| resource.binding().transport)
        .collect::<Vec<_>>();
    imported_transports.sort_by_key(|transport| match transport {
        TransportKind::Tcp => 0,
        TransportKind::Udp => 1,
    });
    assert_eq!(
        imported_transports,
        vec![TransportKind::Tcp, TransportKind::Udp]
    );
    let (network, child_socket_resources) = network.transfer_sockets()?;
    let child_native_sockets = child.prepare_native_socket_import(child_socket_resources)?;
    child.prestage(
        &revy_runtime_transfer::PrestageV1 {
            phase: revy_runtime_transfer::PrestagePhaseV1::Preparing as i32,
            core_revision: child.core_revision().value(),
            directory_revision: current_directory.directory().revision(),
            resources: vec![transfer_resource(
                3,
                revy_runtime_transfer::ResourceKindV1::SessionDirectory,
                current_directory.region(),
            )],
            arena_used: arena.descriptor().used,
        },
        &mut child_arena,
    )?;
    let core_update = preparing.prestage_core(base_revision).await?;
    child.prestage(
        &revy_runtime_transfer::PrestageV1 {
            phase: revy_runtime_transfer::PrestagePhaseV1::Preparing as i32,
            core_revision: core_update.revision(),
            directory_revision: current_directory.directory().revision(),
            resources: vec![transfer_resource(
                4,
                core_update.kind(),
                core_update.region(),
            )],
            arena_used: arena.descriptor().used,
        },
        &mut child_arena,
    )?;
    let final_base_revision = core_update.revision();
    let prepared_delta_bytes = core_update.region().as_slice().to_vec();
    assert_eq!(
        core_update.kind(),
        revy_runtime_transfer::ResourceKindV1::CoreJournal
    );
    assert!(final_base_revision > base_revision);
    write_packet(&mut stream, &codec, &held_item_change(6)).await?;
    let _ = read_until_held_item_change(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        6,
        16,
    )
    .await?;
    let frozen = preparing
        .freeze(network, current_directory, core_update)
        .await?;
    assert_eq!(frozen.network().sessions().len(), 2);
    let mut frozen_session_transports = frozen
        .network()
        .sessions()
        .iter()
        .map(crate::runtime::ExecutableSessionTransportResource::transport)
        .collect::<Vec<_>>();
    frozen_session_transports.sort_by_key(|transport| match transport {
        TransportKind::Tcp => 0,
        TransportKind::Udp => 1,
    });
    assert_eq!(
        frozen_session_transports,
        vec![TransportKind::Tcp, TransportKind::Udp]
    );
    let delta_bytes = frozen.core_delta().region().as_slice().to_vec();
    assert_eq!(
        frozen.core_delta().base_revision_value(),
        final_base_revision
    );
    assert!(frozen.core_delta().final_revision_value() > final_base_revision);
    assert_eq!(frozen.core_delta().encoded_len(), delta_bytes.len());
    assert!(frozen.core_delta().persisted_revision_value() >= persisted_at_stage);
    assert!(
        frozen.core_delta().persisted_revision_value()
            <= frozen.core_delta().final_revision_value()
    );
    // The held-slot change dirties a revision after pre-copy. A subsequent volatile-only
    // mutation may advance the core without changing the latest persistence-relevant revision.
    assert!(
        frozen
            .core_delta()
            .latest_dirty_revision_value()
            .is_some_and(|revision| {
                revision > final_base_revision
                    && revision <= frozen.core_delta().final_revision_value()
            })
    );
    assert_eq!(
        frozen.final_directory().directory().revision(),
        decoded_current_directory.revision()
    );
    let session_states = frozen.network().session_states();
    let mut final_resources = vec![
        transfer_resource(
            4,
            revy_runtime_transfer::ResourceKindV1::CoreJournal,
            frozen.core_delta().region(),
        ),
        transfer_resource(
            5,
            revy_runtime_transfer::ResourceKindV1::SessionDirectory,
            frozen.final_directory().region(),
        ),
    ];
    let raknet_router_state = frozen
        .network()
        .raknet_router_state()
        .expect("Bedrock listener should seal router-owned peer state");
    final_resources.push(transfer_resource(
        6,
        revy_runtime_transfer::ResourceKindV1::RakNetState,
        raknet_router_state,
    ));
    final_resources.extend(session_states.iter().enumerate().map(|(index, state)| {
        let mut resource = transfer_resource(
            u64::try_from(index).expect("session resource index fits u64") + 7,
            revy_runtime_transfer::ResourceKindV1::SessionState,
            state.region(),
        );
        resource.logical_id = Some(state.connection_id().0);
        resource
    }));
    let final_prestage = revy_runtime_transfer::PrestageV1 {
        phase: revy_runtime_transfer::PrestagePhaseV1::Frozen as i32,
        core_revision: frozen.core_delta().final_revision_value(),
        directory_revision: frozen.final_directory().directory().revision(),
        resources: final_resources,
        arena_used: arena.descriptor().used,
    };
    child.prestage(&final_prestage, &mut child_arena)?;
    let child = child.prepare_session_import(&final_prestage.resources, &child_arena)?;
    assert_eq!(child.session_count(), 2);
    let child = child.prepare_network_import(child_native_sockets).await?;
    assert_eq!(child.session_count(), 2);
    assert_eq!(child.queued_bedrock_peer_count(), 0);
    let child_commit = child.commit(&revy_runtime_transfer::CommitV1 {
        final_core_revision: frozen.core_delta().final_revision_value(),
        final_directory_revision: frozen.final_directory().directory().revision(),
        parent_epoch_revision: frozen.epoch_revision(),
        persisted_core_revision: frozen.core_delta().persisted_revision_value(),
        latest_dirty_core_revision: frozen.core_delta().latest_dirty_revision_value(),
        stage_duration_us: frozen.stage_duration_us(),
        prepare_duration_us: frozen.prepare_duration_us(),
        session_count: frozen.session_count() as u64,
        java_session_count: frozen.connection_mix().java as u64,
        bedrock_session_count: frozen.connection_mix().bedrock as u64,
    })?;
    assert_eq!(
        child_commit.core().revision().value(),
        frozen.core_delta().final_revision_value()
    );
    assert_eq!(
        child_commit.directory().revision(),
        frozen.final_directory().directory().revision()
    );
    // Compare the sealed delta with its exact source revision, not with a later tick
    // that may run as soon as abort reopens the data plane.
    let active = server.runtime.authority.active().core.version();
    frozen.abort().await?;

    let mut abandoned_delta = Vec::new();
    assert!(matches!(
        server
            .runtime
            .authority
            .active()
            .core
            .write_process_delta_since(active.revision(), &mut abandoned_delta)
            .await,
        Err(revy_voxel_core::CoreTransferError::InvalidState(_))
    ));
    assert!(abandoned_delta.is_empty());

    let imported = revy_voxel_core::CoreVersion::import_process_transfer(
        &core_snapshot,
        crate::runtime::selection::SelectionResolver::content_behavior(),
    )
    .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let replayed = imported
        .apply_process_transfer_delta(&prepared_delta_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))?
        .apply_process_transfer_delta(&delta_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    assert_eq!(replayed.revision(), active.revision());
    assert_eq!(
        replayed
            .prepare_process_transfer()
            .map_err(|error| RuntimeError::Config(error.to_string()))?
            .bytes(),
        active
            .prepare_process_transfer()
            .map_err(|error| RuntimeError::Config(error.to_string()))?
            .bytes()
    );

    write_packet(&mut stream, &codec, &held_item_change(6)).await?;
    let held_item = read_until_held_item_change(
        &mut stream,
        &codec,
        &mut buffer,
        TestJavaProtocol::Je5,
        6,
        16,
    )
    .await?;
    assert_eq!(
        held_item_from_packet_for_protocol(TestJavaProtocol::Je5, &held_item)?,
        6
    );
    bedrock
        .place_block(revy_voxel_semantic::BlockPos::new(2, 3, 0), 1)
        .await?;
    let _ = read_until_bedrock_packet(&mut bedrock, TestBedrockPacket::UpdateBlock, 32).await?;

    drop(stream);
    assert_eq!(server.runtime.authority.executable_upgrade_status(), None);
    server.shutdown().await
}

fn transfer_resource(
    resource_id: u64,
    kind: revy_runtime_transfer::ResourceKindV1,
    region: &revy_runtime_transfer::SealedArenaRegion,
) -> revy_runtime_transfer::ResourceDescriptorV1 {
    revy_runtime_transfer::ResourceDescriptorV1 {
        resource_id,
        kind: kind as i32,
        arena_offset: region.offset() as u64,
        arena_length: region.length() as u64,
        native_handle_index: None,
        logical_id: None,
    }
}
