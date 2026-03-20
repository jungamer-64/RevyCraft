use super::{
    InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin, InProcessStoragePlugin,
    PluginAbiRange, PluginCatalog, PluginFailurePolicy, PluginHost,
};
use crate::config::ServerConfig;
use crate::host::plugin_host_from_config;
use crate::registry::RuntimeRegistries;
use mc_plugin_api::{
    CURRENT_PLUGIN_ABI, PluginAbiVersion, PluginKind, PluginManifestV1, Utf8Slice,
};
use mc_plugin_auth_offline::in_process_auth_entrypoints as offline_auth_entrypoints;
use mc_plugin_gameplay_canonical::in_process_gameplay_entrypoints as canonical_gameplay_entrypoints;
use mc_plugin_gameplay_readonly::in_process_gameplay_entrypoints as readonly_gameplay_entrypoints;
use mc_plugin_proto_be_26_3::in_process_protocol_entrypoints as be_26_3_entrypoints;
use mc_plugin_proto_be_placeholder::in_process_protocol_entrypoints as be_placeholder_entrypoints;
use mc_plugin_proto_je_1_7_10::in_process_protocol_entrypoints;
use mc_plugin_proto_je_1_8_x::in_process_protocol_entrypoints as je_1_8_x_entrypoints;
use mc_plugin_proto_je_1_12_2::in_process_protocol_entrypoints as je_1_12_2_entrypoints;
use mc_plugin_storage_je_anvil_1_7_10::in_process_storage_entrypoints as storage_entrypoints;
use mc_proto_common::{Edition, PacketWriter, TransportKind, WireFormatKind};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use uuid::Uuid;

mod entity_id_probe_gameplay_plugin {
    use mc_core::{CapabilitySet, CoreCommand, GameplayEffect, GameplayProfileId, PlayerSnapshot};
    use mc_plugin_api::{GameplayDescriptor, GameplaySessionSnapshot};
    use mc_plugin_sdk_rust::{
        GameplayHost, RustGameplayPlugin, StaticPluginManifest, export_gameplay_plugin,
    };
    use std::sync::{Mutex, OnceLock};

    #[derive(Default)]
    pub struct EntityIdProbeGameplayPlugin;

    fn recorded_session_slot() -> &'static Mutex<Option<GameplaySessionSnapshot>> {
        static RECORDED_SESSION: OnceLock<Mutex<Option<GameplaySessionSnapshot>>> = OnceLock::new();
        RECORDED_SESSION.get_or_init(|| Mutex::new(None))
    }

    pub fn take_recorded_session() -> Option<GameplaySessionSnapshot> {
        recorded_session_slot()
            .lock()
            .expect("recorded gameplay session mutex should not be poisoned")
            .take()
    }

    impl RustGameplayPlugin for EntityIdProbeGameplayPlugin {
        fn descriptor(&self) -> GameplayDescriptor {
            GameplayDescriptor {
                profile: GameplayProfileId::new("entity-aware"),
            }
        }

        fn capability_set(&self) -> CapabilitySet {
            let mut capabilities = CapabilitySet::new();
            let _ = capabilities.insert("gameplay.profile.entity-aware");
            let _ = capabilities.insert("runtime.reload.gameplay");
            capabilities
        }

        fn handle_command(
            &self,
            _host: &dyn GameplayHost,
            session: &GameplaySessionSnapshot,
            _command: &CoreCommand,
        ) -> Result<GameplayEffect, String> {
            *recorded_session_slot()
                .lock()
                .expect("recorded gameplay session mutex should not be poisoned") =
                Some(session.clone());
            Ok(GameplayEffect::default())
        }

        fn handle_player_join(
            &self,
            _host: &dyn GameplayHost,
            _session: &GameplaySessionSnapshot,
            _player: &PlayerSnapshot,
        ) -> Result<mc_core::GameplayJoinEffect, String> {
            Ok(mc_core::GameplayJoinEffect::default())
        }
    }

    const MANIFEST: StaticPluginManifest = StaticPluginManifest::gameplay(
        "gameplay-entity-aware",
        "Entity Aware Gameplay Plugin",
        &["gameplay.profile:entity-aware", "runtime.reload.gameplay"],
    );

    export_gameplay_plugin!(EntityIdProbeGameplayPlugin, MANIFEST);
}

mod custom_wire_codec_protocol_plugin {
    use mc_core::CapabilitySet;
    use mc_plugin_api::{
        ByteSlice, CURRENT_PLUGIN_ABI, OwnedBuffer, PluginErrorCode, PluginKind,
        PluginManifestV1, ProtocolPluginApiV1, ProtocolRequest, ProtocolResponse, Utf8Slice,
        WireFrameDecodeResult, decode_protocol_request, encode_protocol_response,
    };
    use mc_plugin_sdk_rust::InProcessProtocolEntrypoints;
    use mc_proto_common::{Edition, ProtocolDescriptor, TransportKind, WireFormatKind};
    use std::sync::OnceLock;

    const PLUGIN_ID: &str = "protocol-custom-wire";

    fn descriptor() -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: PLUGIN_ID.to_string(),
            transport: TransportKind::Tcp,
            wire_format: WireFormatKind::MinecraftFramed,
            edition: Edition::Je,
            version_name: "custom-wire".to_string(),
            protocol_number: 1234,
        }
    }

    fn write_buffer(output: *mut OwnedBuffer, mut bytes: Vec<u8>) {
        if output.is_null() {
            return;
        }
        unsafe {
            *output = OwnedBuffer {
                ptr: bytes.as_mut_ptr(),
                len: bytes.len(),
                cap: bytes.capacity(),
            };
        }
        std::mem::forget(bytes);
    }

    fn write_error(error_out: *mut OwnedBuffer, message: String) {
        write_buffer(error_out, message.into_bytes());
    }

    fn handle_request(request: ProtocolRequest) -> Result<ProtocolResponse, String> {
        match request {
            ProtocolRequest::Describe => Ok(ProtocolResponse::Descriptor(descriptor())),
            ProtocolRequest::DescribeBedrockListener => {
                Ok(ProtocolResponse::BedrockListenerDescriptor(None))
            }
            ProtocolRequest::CapabilitySet => Ok(ProtocolResponse::CapabilitySet(
                CapabilitySet::new(),
            )),
            ProtocolRequest::EncodeWireFrame { payload } => {
                let length = u8::try_from(payload.len())
                    .map_err(|_| "payload too large for test wire codec".to_string())?;
                let mut frame = vec![0xc0, length];
                frame.extend_from_slice(&payload);
                Ok(ProtocolResponse::Frame(frame))
            }
            ProtocolRequest::TryDecodeWireFrame { buffer } => {
                if buffer.is_empty() || buffer[0] != 0xc0 {
                    return Ok(ProtocolResponse::WireFrameDecodeResult(None));
                }
                if buffer.len() < 2 {
                    return Ok(ProtocolResponse::WireFrameDecodeResult(None));
                }
                let payload_len = usize::from(buffer[1]);
                let frame_len = 2 + payload_len;
                if buffer.len() < frame_len {
                    return Ok(ProtocolResponse::WireFrameDecodeResult(None));
                }
                Ok(ProtocolResponse::WireFrameDecodeResult(Some(
                    WireFrameDecodeResult {
                        frame: buffer[2..frame_len].to_vec(),
                        bytes_consumed: frame_len,
                    },
                )))
            }
            other => Err(format!("unsupported protocol request in test plugin: {other:?}")),
        }
    }

    unsafe extern "C" fn invoke(
        request: ByteSlice,
        output: *mut OwnedBuffer,
        error_out: *mut OwnedBuffer,
    ) -> PluginErrorCode {
        let request_bytes = unsafe { std::slice::from_raw_parts(request.ptr, request.len) };
        let request = match decode_protocol_request(request_bytes) {
            Ok(request) => request,
            Err(error) => {
                write_error(error_out, error.to_string());
                return PluginErrorCode::InvalidInput;
            }
        };
        let response = match handle_request(request.clone()) {
            Ok(response) => response,
            Err(message) => {
                write_error(error_out, message);
                return PluginErrorCode::Internal;
            }
        };
        match encode_protocol_response(&request, &response) {
            Ok(bytes) => {
                write_buffer(output, bytes);
                PluginErrorCode::Ok
            }
            Err(error) => {
                write_error(error_out, error.to_string());
                PluginErrorCode::Internal
            }
        }
    }

    unsafe extern "C" fn free_buffer(buffer: OwnedBuffer) {
        if buffer.ptr.is_null() {
            return;
        }
        let _ = unsafe { Vec::from_raw_parts(buffer.ptr, buffer.len, buffer.cap) };
    }

    pub fn in_process_entrypoints() -> InProcessProtocolEntrypoints {
        static MANIFEST: OnceLock<PluginManifestV1> = OnceLock::new();
        static API: OnceLock<ProtocolPluginApiV1> = OnceLock::new();
        InProcessProtocolEntrypoints {
            manifest: MANIFEST.get_or_init(|| PluginManifestV1 {
                plugin_id: Utf8Slice::from_static_str(PLUGIN_ID),
                display_name: Utf8Slice::from_static_str("Custom Wire Codec Protocol Plugin"),
                plugin_kind: PluginKind::Protocol,
                plugin_abi: CURRENT_PLUGIN_ABI,
                min_host_abi: CURRENT_PLUGIN_ABI,
                max_host_abi: CURRENT_PLUGIN_ABI,
                capabilities: std::ptr::null(),
                capabilities_len: 0,
            }),
            api: API.get_or_init(|| ProtocolPluginApiV1 {
                invoke,
                free_buffer,
            }),
        }
    }
}

fn manifest_with_abi(
    plugin_id: &'static str,
    plugin_abi: PluginAbiVersion,
) -> &'static PluginManifestV1 {
    Box::leak(Box::new(PluginManifestV1 {
        plugin_id: Utf8Slice::from_static_str(plugin_id),
        display_name: Utf8Slice::from_static_str(plugin_id),
        plugin_kind: PluginKind::Protocol,
        plugin_abi,
        min_host_abi: CURRENT_PLUGIN_ABI,
        max_host_abi: CURRENT_PLUGIN_ABI,
        capabilities: std::ptr::null(),
        capabilities_len: 0,
    }))
}

#[test]
fn in_process_protocol_plugin_swaps_generation() {
    let entrypoints = in_process_protocol_entrypoints();
    let mut catalog = PluginCatalog::default();
    catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
        plugin_id: "je-1_7_10".to_string(),
        manifest: entrypoints.manifest,
        api: entrypoints.api,
    });

    let host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    let mut registries = RuntimeRegistries::new();
    host.load_into_registries(&mut registries)
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
fn all_protocol_plugins_register_and_resolve() {
    let mut catalog = PluginCatalog::default();
    for (plugin_id, entrypoints) in [
        ("je-1_7_10", in_process_protocol_entrypoints()),
        ("je-1_8_x", je_1_8_x_entrypoints()),
        ("je-1_12_2", je_1_12_2_entrypoints()),
        ("be-placeholder", be_placeholder_entrypoints()),
    ] {
        catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
            plugin_id: plugin_id.to_string(),
            manifest: entrypoints.manifest,
            api: entrypoints.api,
        });
    }

    let host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    let mut registries = RuntimeRegistries::new();
    host.load_into_registries(&mut registries)
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
    let mut catalog = PluginCatalog::default();
    for (plugin_id, entrypoints) in [
        ("je-1_7_10", in_process_protocol_entrypoints()),
        ("be-26_3", be_26_3_entrypoints()),
        ("be-placeholder", be_placeholder_entrypoints()),
    ] {
        catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
            plugin_id: plugin_id.to_string(),
            manifest: entrypoints.manifest,
            api: entrypoints.api,
        });
    }

    let host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    let mut registries = RuntimeRegistries::new();
    host.load_into_registries(&mut registries)
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

    let mut catalog = PluginCatalog::default();
    let entrypoints = custom_wire_codec_protocol_plugin::in_process_entrypoints();
    catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
        plugin_id: "protocol-custom-wire".to_string(),
        manifest: entrypoints.manifest,
        api: entrypoints.api,
    });

    let host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    let mut registries = RuntimeRegistries::new();
    host.load_into_registries(&mut registries)
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
    assert!(buffer.is_empty(), "decoded frame should consume buffered bytes");
}

#[test]
fn abi_mismatch_is_rejected_before_registration() {
    let entrypoints = in_process_protocol_entrypoints();
    let mut catalog = PluginCatalog::default();
    catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
        plugin_id: "je-1_7_10".to_string(),
        manifest: manifest_with_abi("je-1_7_10", PluginAbiVersion { major: 9, minor: 0 }),
        api: entrypoints.api,
    });
    let host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    let mut registries = RuntimeRegistries::new();

    let error = host
        .load_into_registries(&mut registries)
        .expect_err("ABI mismatch should fail before registration");
    assert!(matches!(
        error,
        crate::RuntimeError::Config(message) if message.contains("ABI")
    ));
}

#[test]
fn storage_and_auth_plugins_are_managed_without_quarantine() {
    let mut catalog = PluginCatalog::default();
    let storage = storage_entrypoints();
    catalog.register_in_process_storage_plugin(InProcessStoragePlugin {
        plugin_id: "storage-je-anvil-1_7_10".to_string(),
        manifest: storage.manifest,
        api: storage.api,
    });
    let auth = offline_auth_entrypoints();
    catalog.register_in_process_auth_plugin(InProcessAuthPlugin {
        plugin_id: "auth-offline".to_string(),
        manifest: auth.manifest,
        api: auth.api,
    });
    let host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    let mut registries = RuntimeRegistries::new();
    host.load_into_registries(&mut registries)
        .expect("storage/auth plugin kinds should register with the host");

    assert!(host.quarantine_reason("storage-je-anvil-1_7_10").is_none());
    assert!(host.quarantine_reason("auth-offline").is_none());
}

#[test]
fn gameplay_profiles_activate_and_resolve() {
    let mut catalog = PluginCatalog::default();
    let canonical = canonical_gameplay_entrypoints();
    catalog.register_in_process_gameplay_plugin(InProcessGameplayPlugin {
        plugin_id: "gameplay-canonical".to_string(),
        manifest: canonical.manifest,
        api: canonical.api,
    });
    let readonly = readonly_gameplay_entrypoints();
    catalog.register_in_process_gameplay_plugin(InProcessGameplayPlugin {
        plugin_id: "gameplay-readonly".to_string(),
        manifest: readonly.manifest,
        api: readonly.api,
    });

    let host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    host.activate_gameplay_profiles(&ServerConfig {
        default_gameplay_profile: "canonical".to_string(),
        gameplay_profile_map: [("je-1_7_10".to_string(), "readonly".to_string())]
            .into_iter()
            .collect(),
        ..ServerConfig::default()
    })
    .expect("known gameplay profiles should activate");

    assert!(host.resolve_gameplay_profile("canonical").is_some());
    assert!(host.resolve_gameplay_profile("readonly").is_some());
}

#[test]
fn initialize_runtime_registries_activates_runtime_profiles() {
    let mut catalog = PluginCatalog::default();
    let protocol = in_process_protocol_entrypoints();
    catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
        plugin_id: "je-1_7_10".to_string(),
        manifest: protocol.manifest,
        api: protocol.api,
    });
    let canonical = canonical_gameplay_entrypoints();
    catalog.register_in_process_gameplay_plugin(InProcessGameplayPlugin {
        plugin_id: "gameplay-canonical".to_string(),
        manifest: canonical.manifest,
        api: canonical.api,
    });
    let storage = storage_entrypoints();
    catalog.register_in_process_storage_plugin(InProcessStoragePlugin {
        plugin_id: "storage-je-anvil-1_7_10".to_string(),
        manifest: storage.manifest,
        api: storage.api,
    });
    let auth = offline_auth_entrypoints();
    catalog.register_in_process_auth_plugin(InProcessAuthPlugin {
        plugin_id: "auth-offline".to_string(),
        manifest: auth.manifest,
        api: auth.api,
    });

    let host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    let mut registries = RuntimeRegistries::new();
    host.initialize_runtime_registries(&ServerConfig::default(), &mut registries)
        .expect("runtime registries should initialize with runtime profiles");

    assert!(registries.protocols().resolve_adapter("je-1_7_10").is_some());
    assert!(registries.plugin_host().is_some());
    assert!(registries.storage().resolve("je-anvil-1_7_10").is_some());
    assert!(host.resolve_gameplay_profile("canonical").is_some());
    assert!(host.resolve_storage_profile("je-anvil-1_7_10").is_some());
    assert!(host.resolve_auth_profile("offline-v1").is_some());
}

#[test]
fn gameplay_command_snapshot_preserves_entity_id() {
    use mc_core::{
        BlockPos, BlockState, CapabilitySet, CoreCommand, DimensionId, EntityId,
        GameplayPolicyResolver, GameplayProfileId, GameplayQuery, PlayerId, SessionCapabilitySet,
        WorldMeta,
    };

    struct NoopQuery;

    impl GameplayQuery for NoopQuery {
        fn world_meta(&self) -> WorldMeta {
            WorldMeta {
                level_name: "world".to_string(),
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

        fn can_edit_block(&self, _player_id: PlayerId, _position: BlockPos) -> bool {
            true
        }
    }

    let _ = entity_id_probe_gameplay_plugin::take_recorded_session();

    let mut catalog = PluginCatalog::default();
    let probe = entity_id_probe_gameplay_plugin::in_process_gameplay_entrypoints();
    catalog.register_in_process_gameplay_plugin(InProcessGameplayPlugin {
        plugin_id: "gameplay-entity-aware".to_string(),
        manifest: probe.manifest,
        api: probe.api,
    });

    let host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    host.activate_gameplay_profiles(&ServerConfig {
        default_gameplay_profile: "entity-aware".to_string(),
        ..ServerConfig::default()
    })
    .expect("entity-aware gameplay profile should activate");

    let profile = host
        .resolve_gameplay_profile("entity-aware")
        .expect("entity-aware gameplay profile should resolve");
    let player_id = PlayerId(Uuid::from_u128(7));
    profile
        .handle_command(
            &NoopQuery,
            &SessionCapabilitySet {
                protocol: CapabilitySet::new(),
                gameplay: CapabilitySet::new(),
                gameplay_profile: GameplayProfileId::new("entity-aware"),
                entity_id: Some(EntityId(41)),
                protocol_generation: None,
                gameplay_generation: None,
            },
            &CoreCommand::SetHeldSlot { player_id, slot: 0 },
        )
        .expect("gameplay command should succeed");

    let recorded = entity_id_probe_gameplay_plugin::take_recorded_session()
        .expect("gameplay plugin should receive a session snapshot");
    assert_eq!(recorded.player_id, Some(player_id));
    assert_eq!(recorded.entity_id, Some(EntityId(41)));
    assert_eq!(recorded.gameplay_profile.as_str(), "entity-aware");
}

#[test]
fn unknown_gameplay_profile_fails_activation() {
    let mut catalog = PluginCatalog::default();
    let canonical = canonical_gameplay_entrypoints();
    catalog.register_in_process_gameplay_plugin(InProcessGameplayPlugin {
        plugin_id: "gameplay-canonical".to_string(),
        manifest: canonical.manifest,
        api: canonical.api,
    });

    let host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    let error = host
        .activate_gameplay_profiles(&ServerConfig {
            default_gameplay_profile: "readonly".to_string(),
            ..ServerConfig::default()
        })
        .expect_err("unknown gameplay profile should fail fast");
    assert!(matches!(
        error,
        crate::RuntimeError::Config(message) if message.contains("unknown gameplay profile")
    ));
}

#[test]
fn storage_and_auth_profiles_activate_and_resolve() {
    let mut catalog = PluginCatalog::default();
    let storage = storage_entrypoints();
    catalog.register_in_process_storage_plugin(InProcessStoragePlugin {
        plugin_id: "storage-je-anvil-1_7_10".to_string(),
        manifest: storage.manifest,
        api: storage.api,
    });
    let auth = offline_auth_entrypoints();
    catalog.register_in_process_auth_plugin(InProcessAuthPlugin {
        plugin_id: "auth-offline".to_string(),
        manifest: auth.manifest,
        api: auth.api,
    });

    let host = Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    host.activate_storage_profile("je-anvil-1_7_10")
        .expect("known storage profile should activate");
    host.activate_auth_profile("offline-v1")
        .expect("known auth profile should activate");

    assert!(host.resolve_storage_profile("je-anvil-1_7_10").is_some());
    assert!(host.resolve_auth_profile("offline-v1").is_some());
}

#[test]
fn unknown_storage_and_auth_profiles_fail_activation() {
    let host = Arc::new(PluginHost::new(
        PluginCatalog::default(),
        PluginAbiRange::default(),
        PluginFailurePolicy::Quarantine,
    ));
    let storage = host
        .activate_storage_profile("missing")
        .expect_err("unknown storage profile should fail fast");
    assert!(matches!(
        storage,
        crate::RuntimeError::Config(message) if message.contains("unknown storage profile")
    ));

    let auth = host
        .activate_auth_profile("missing")
        .expect_err("unknown auth profile should fail fast");
    assert!(matches!(
        auth,
        crate::RuntimeError::Config(message) if message.contains("unknown auth profile")
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn packaged_protocol_plugins_load_via_dlopen() -> Result<(), crate::RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;

    let config = ServerConfig {
        plugins_dir: dist_dir,
        ..ServerConfig::default()
    };
    let host = plugin_host_from_config(&config)?.expect("packaged plugins should be discovered");
    let mut registries = RuntimeRegistries::new();
    host.load_into_registries(&mut registries)?;

    for adapter_id in ["je-1_7_10", "je-1_8_x", "je-1_12_2", "be-placeholder"] {
        let adapter = registries
            .protocols()
            .resolve_adapter(adapter_id)
            .expect("packaged plugin adapter should resolve");
        assert!(
            adapter.capability_set().contains(&format!(
                "build-tag:{}",
                crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
            )),
            "adapter `{adapter_id}` should expose build tag capability"
        );
    }

    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn packaged_protocol_reload_replaces_generation() -> Result<(), crate::RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("plugin-host-reload");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;

    let config = ServerConfig {
        plugins_dir: dist_dir.clone(),
        ..ServerConfig::default()
    };
    let host = plugin_host_from_config(&config)?.expect("packaged plugins should be discovered");
    let mut registries = RuntimeRegistries::new();
    host.load_into_registries(&mut registries)?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("packaged je-1_7_10 adapter should resolve");
    let first_generation = adapter
        .plugin_generation_id()
        .expect("packaged adapter should report plugin generation");
    assert!(adapter.capability_set().contains(&format!(
        "build-tag:{}",
        crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
    )));

    std::thread::sleep(Duration::from_secs(1));
    package_single_protocol_plugin(
        "mc-plugin-proto-je-1_7_10",
        "je-1_7_10",
        &dist_dir,
        &target_dir,
        "reload-v2",
    )?;

    let reloaded = host.reload_modified()?;
    assert_eq!(reloaded, vec!["je-1_7_10".to_string()]);

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("reloaded adapter should resolve");
    let next_generation = adapter
        .plugin_generation_id()
        .expect("reloaded adapter should report plugin generation");
    assert_ne!(first_generation, next_generation);
    assert!(adapter.capability_set().contains("build-tag:reload-v2"));
    Ok(())
}

fn je_handshake_frame(protocol_version: i32) -> Vec<u8> {
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

fn raknet_unconnected_ping() -> Vec<u8> {
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

#[cfg(target_os = "linux")]
fn package_single_protocol_plugin(
    cargo_package: &str,
    plugin_id: &str,
    dist_dir: &Path,
    target_dir: &Path,
    build_tag: &str,
) -> Result<(), crate::RuntimeError> {
    let _guard = crate::packaged_plugin_test_build_lock()
        .lock()
        .expect("packaged plugin build lock should not be poisoned");
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let status = Command::new(cargo)
        .current_dir(workspace_root())
        .env("CARGO_TARGET_DIR", target_dir)
        .env("REVY_PLUGIN_BUILD_TAG", build_tag)
        .arg("build")
        .arg("-p")
        .arg(cargo_package)
        .status()
        .map_err(|error| crate::RuntimeError::Config(error.to_string()))?;
    if !status.success() {
        return Err(crate::RuntimeError::Config(format!(
            "cargo build failed for `{cargo_package}`"
        )));
    }

    let artifact_name = dynamic_library_filename(cargo_package);
    let source = target_dir.join("debug").join(&artifact_name);
    let plugin_dir = dist_dir.join(plugin_id);
    fs::create_dir_all(&plugin_dir)?;
    let packaged_artifact = packaged_artifact_name(&artifact_name, build_tag);
    let destination = plugin_dir.join(&packaged_artifact);
    let staging = plugin_dir.join(format!(".{packaged_artifact}.tmp"));
    fs::copy(&source, &staging)?;
    if destination.exists() {
        fs::remove_file(&destination)?;
    }
    fs::rename(&staging, &destination)?;
    let manifest = format!(
        "[plugin]\nid = \"{plugin_id}\"\nkind = \"protocol\"\n\n[artifacts]\n\"{}-{}\" = \"{packaged_artifact}\"\n",
        env::consts::OS,
        env::consts::ARCH
    );
    fs::write(plugin_dir.join("plugin.toml"), manifest)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn dynamic_library_filename(package: &str) -> String {
    let crate_name = package.replace('-', "_");
    match env::consts::OS {
        "windows" => format!("{crate_name}.dll"),
        "macos" => format!("lib{crate_name}.dylib"),
        _ => format!("lib{crate_name}.so"),
    }
}

#[cfg(target_os = "linux")]
fn packaged_artifact_name(base_name: &str, build_tag: &str) -> String {
    if let Some((stem, extension)) = base_name.rsplit_once('.') {
        format!("{stem}-{build_tag}.{extension}")
    } else {
        format!("{base_name}-{build_tag}")
    }
}

#[cfg(target_os = "linux")]
fn workspace_root() -> PathBuf {
    crate::packaged_plugin_test_workspace_root()
}
