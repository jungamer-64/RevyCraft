use super::{current_artifact_key, with_current_gameplay_query, with_gameplay_query};
use crate::PluginHostError as RuntimeError;
use crate::config::ServerConfig;
use crate::host::plugin_host_from_config;
use crate::runtime::{ProtocolReloadSession, RuntimeReloadContext};
use crate::test_support::{
    self, InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin,
    InProcessStoragePlugin, PluginAbiRange, PluginFailureAction, PluginFailureMatrix,
    TestPluginHost, TestPluginHostBuilder,
};
use mc_core::{
    BlockPos, BlockState, ConnectionId, CoreConfig, DimensionId, EntityId, GameplayQuery, PlayerId,
    ServerCore, WorldMeta,
};
use mc_plugin_api::abi::{
    CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, PluginAbiVersion, PluginKind, Utf8Slice,
};
use mc_plugin_api::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_api::manifest::PluginManifestV1;
use mc_plugin_auth_offline::in_process_auth_entrypoints as offline_auth_entrypoints;
use mc_plugin_gameplay_canonical::in_process_gameplay_entrypoints as canonical_gameplay_entrypoints;
use mc_plugin_gameplay_readonly::in_process_gameplay_entrypoints as readonly_gameplay_entrypoints;
use mc_plugin_proto_be_26_3::in_process_protocol_entrypoints as be_26_3_entrypoints;
use mc_plugin_proto_be_placeholder::in_process_protocol_entrypoints as be_placeholder_entrypoints;
use mc_plugin_proto_je_1_7_10::in_process_protocol_entrypoints;
use mc_plugin_proto_je_1_8_x::in_process_protocol_entrypoints as je_1_8_x_entrypoints;
use mc_plugin_proto_je_1_12_2::in_process_protocol_entrypoints as je_1_12_2_entrypoints;
use mc_plugin_storage_je_anvil_1_7_10::in_process_storage_entrypoints as storage_entrypoints;
use mc_proto_common::{ConnectionPhase, Edition, PacketWriter, TransportKind, WireFormatKind};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::tempdir;
use uuid::Uuid;

fn build_test_plugin_host(
    builder: TestPluginHostBuilder,
    abi_range: PluginAbiRange,
    failure_matrix: PluginFailureMatrix,
) -> TestPluginHost {
    builder
        .abi_range(abi_range)
        .failure_matrix(failure_matrix)
        .build()
}

mod entity_id_probe_gameplay_plugin {
    use mc_core::{CapabilitySet, CoreCommand, GameplayEffect, GameplayProfileId, PlayerSnapshot};
    use mc_plugin_api::codec::gameplay::{GameplayDescriptor, GameplaySessionSnapshot};
    use mc_plugin_sdk_rust::gameplay::{GameplayHost, RustGameplayPlugin, export_gameplay_plugin};
    use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
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
    use mc_plugin_api::abi::{
        ByteSlice, CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, OwnedBuffer, PluginErrorCode,
        PluginKind, Utf8Slice,
    };
    use mc_plugin_api::codec::protocol::{
        ProtocolRequest, ProtocolResponse, WireFrameDecodeResult, decode_protocol_request,
        encode_protocol_response,
    };
    use mc_plugin_api::host_api::ProtocolPluginApiV1;
    use mc_plugin_api::manifest::PluginManifestV1;
    use mc_plugin_sdk_rust::test_support::InProcessProtocolEntrypoints;
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
            ProtocolRequest::CapabilitySet => {
                Ok(ProtocolResponse::CapabilitySet(CapabilitySet::new()))
            }
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
            other => Err(format!(
                "unsupported protocol request in test plugin: {other:?}"
            )),
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
        static CAPABILITIES: OnceLock<&'static [CapabilityDescriptorV1]> = OnceLock::new();
        static API: OnceLock<ProtocolPluginApiV1> = OnceLock::new();
        InProcessProtocolEntrypoints {
            manifest: MANIFEST.get_or_init(|| PluginManifestV1 {
                plugin_id: Utf8Slice::from_static_str(PLUGIN_ID),
                display_name: Utf8Slice::from_static_str("Custom Wire Codec Protocol Plugin"),
                plugin_kind: PluginKind::Protocol,
                plugin_abi: CURRENT_PLUGIN_ABI,
                min_host_abi: CURRENT_PLUGIN_ABI,
                max_host_abi: CURRENT_PLUGIN_ABI,
                capabilities: CAPABILITIES
                    .get_or_init(|| {
                        Box::leak(
                            vec![CapabilityDescriptorV1 {
                                name: Utf8Slice::from_static_str("runtime.reload.protocol"),
                            }]
                            .into_boxed_slice(),
                        )
                    })
                    .as_ptr(),
                capabilities_len: 1,
            }),
            api: API.get_or_init(|| ProtocolPluginApiV1 {
                invoke,
                free_buffer,
            }),
        }
    }
}

mod failing_protocol_plugin {
    use mc_core::CapabilitySet;
    use mc_plugin_api::abi::{
        ByteSlice, CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, OwnedBuffer, PluginErrorCode,
        PluginKind, Utf8Slice,
    };
    use mc_plugin_api::codec::protocol::{
        ProtocolRequest, ProtocolResponse, decode_protocol_request, encode_protocol_response,
    };
    use mc_plugin_api::host_api::ProtocolPluginApiV1;
    use mc_plugin_api::manifest::PluginManifestV1;
    use mc_plugin_sdk_rust::test_support::InProcessProtocolEntrypoints;
    use mc_proto_common::{Edition, ProtocolDescriptor, TransportKind, WireFormatKind};
    use std::sync::OnceLock;

    pub const PLUGIN_ID: &str = "protocol-failing-runtime";

    fn descriptor() -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: PLUGIN_ID.to_string(),
            transport: TransportKind::Tcp,
            wire_format: WireFormatKind::MinecraftFramed,
            edition: Edition::Je,
            version_name: "failing-runtime".to_string(),
            protocol_number: 4242,
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
            ProtocolRequest::CapabilitySet => {
                let mut capabilities = CapabilitySet::new();
                let _ = capabilities.insert("runtime.reload.protocol");
                Ok(ProtocolResponse::CapabilitySet(capabilities))
            }
            ProtocolRequest::TryRoute { .. } => Err("protocol runtime failure".to_string()),
            other => Err(format!(
                "unsupported protocol request in failing test plugin: {other:?}"
            )),
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
        static CAPABILITIES: OnceLock<&'static [CapabilityDescriptorV1]> = OnceLock::new();
        static API: OnceLock<ProtocolPluginApiV1> = OnceLock::new();
        InProcessProtocolEntrypoints {
            manifest: MANIFEST.get_or_init(|| PluginManifestV1 {
                plugin_id: Utf8Slice::from_static_str(PLUGIN_ID),
                display_name: Utf8Slice::from_static_str("Failing Protocol Plugin"),
                plugin_kind: PluginKind::Protocol,
                plugin_abi: CURRENT_PLUGIN_ABI,
                min_host_abi: CURRENT_PLUGIN_ABI,
                max_host_abi: CURRENT_PLUGIN_ABI,
                capabilities: CAPABILITIES
                    .get_or_init(|| {
                        Box::leak(
                            vec![CapabilityDescriptorV1 {
                                name: Utf8Slice::from_static_str("runtime.reload.protocol"),
                            }]
                            .into_boxed_slice(),
                        )
                    })
                    .as_ptr(),
                capabilities_len: 1,
            }),
            api: API.get_or_init(|| ProtocolPluginApiV1 {
                invoke,
                free_buffer,
            }),
        }
    }
}

mod failing_gameplay_plugin {
    use mc_core::{CapabilitySet, CoreCommand, GameplayEffect, GameplayProfileId, PlayerSnapshot};
    use mc_plugin_api::codec::gameplay::{GameplayDescriptor, GameplaySessionSnapshot};
    use mc_plugin_sdk_rust::gameplay::{GameplayHost, RustGameplayPlugin, export_gameplay_plugin};
    use mc_plugin_sdk_rust::manifest::StaticPluginManifest;

    #[derive(Default)]
    pub struct FailingGameplayPlugin;

    impl RustGameplayPlugin for FailingGameplayPlugin {
        fn descriptor(&self) -> GameplayDescriptor {
            GameplayDescriptor {
                profile: GameplayProfileId::new("failing"),
            }
        }

        fn capability_set(&self) -> CapabilitySet {
            let mut capabilities = CapabilitySet::new();
            let _ = capabilities.insert("gameplay.profile.failing");
            let _ = capabilities.insert("runtime.reload.gameplay");
            capabilities
        }

        fn handle_command(
            &self,
            _host: &dyn GameplayHost,
            _session: &GameplaySessionSnapshot,
            _command: &CoreCommand,
        ) -> Result<GameplayEffect, String> {
            Err("gameplay runtime failure".to_string())
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
        "gameplay-failing",
        "Failing Gameplay Plugin",
        &["gameplay.profile:failing", "runtime.reload.gameplay"],
    );

    export_gameplay_plugin!(FailingGameplayPlugin, MANIFEST);
}

mod failing_auth_plugin {
    use mc_core::{CapabilitySet, PlayerId};
    use mc_plugin_api::codec::auth::{AuthDescriptor, AuthMode};
    use mc_plugin_sdk_rust::auth::{RustAuthPlugin, export_auth_plugin};
    use mc_plugin_sdk_rust::manifest::StaticPluginManifest;

    pub const PROFILE_ID: &str = "failing-auth";

    #[derive(Default)]
    pub struct FailingAuthPlugin;

    impl RustAuthPlugin for FailingAuthPlugin {
        fn descriptor(&self) -> AuthDescriptor {
            AuthDescriptor {
                auth_profile: PROFILE_ID.to_string(),
                mode: AuthMode::Offline,
            }
        }

        fn capability_set(&self) -> CapabilitySet {
            let mut capabilities = CapabilitySet::new();
            let _ = capabilities.insert("runtime.reload.auth");
            capabilities
        }

        fn authenticate_offline(&self, _username: &str) -> Result<PlayerId, String> {
            Err("auth runtime failure".to_string())
        }
    }

    const MANIFEST: StaticPluginManifest = StaticPluginManifest::auth(
        "auth-failing",
        "Failing Auth Plugin",
        &[
            "auth.profile:failing-auth",
            "auth.mode:offline",
            "runtime.reload.auth",
        ],
    );

    export_auth_plugin!(FailingAuthPlugin, MANIFEST);
}

fn manifest_with_abi(
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

fn manifest_without_reload_capability(plugin_id: &'static str) -> &'static PluginManifestV1 {
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

fn protocol_reload_context(sessions: Vec<ProtocolReloadSession>) -> RuntimeReloadContext {
    RuntimeReloadContext {
        protocol_sessions: sessions,
        gameplay_sessions: Vec::new(),
        snapshot: ServerCore::new(CoreConfig::default()).snapshot(),
        world_dir: PathBuf::from("."),
    }
}

fn protocol_reload_session(
    connection_id: u64,
    phase: ConnectionPhase,
    player_id: Option<PlayerId>,
    entity_id: Option<EntityId>,
) -> ProtocolReloadSession {
    ProtocolReloadSession {
        adapter_id: "je-1_7_10".to_string(),
        session: ProtocolSessionSnapshot {
            connection_id: ConnectionId(connection_id),
            phase,
            player_id,
            entity_id,
        },
    }
}

struct StubGameplayQuery {
    level_name: &'static str,
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

    fn can_edit_block(&self, _player_id: PlayerId, _position: BlockPos) -> bool {
        false
    }
}

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
    let registries =
        test_support::load_protocol_plugin_set(&host).expect("in-process plugin should load");

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("registered plugin adapter should resolve");
    let first_generation = adapter
        .plugin_generation_id()
        .expect("plugin-backed adapter should report generation");

    let next_generation = test_support::replace_in_process_protocol_plugin(
        &host,
        InProcessProtocolPlugin {
            plugin_id: "je-1_7_10".to_string(),
            manifest: entrypoints.manifest,
            api: entrypoints.api,
        },
    )
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

    let error = match plugin_host_from_config(&ServerConfig {
        plugins_dir: temp_dir.path().to_path_buf(),
        ..ServerConfig::default()
    }) {
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
    let registries =
        test_support::load_protocol_plugin_set(&host).expect("protocol plugins should load");

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
    let registries =
        test_support::load_protocol_plugin_set(&host).expect("protocol plugins should load");

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

    let entrypoints = custom_wire_codec_protocol_plugin::in_process_entrypoints();
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
    let registries = test_support::load_protocol_plugin_set(&host)
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
    let error = match test_support::load_protocol_plugin_set(&host) {
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
    let error = match test_support::load_protocol_plugin_set(&host) {
        Ok(_) => panic!("protocol plugin without runtime.reload.protocol should fail"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        RuntimeError::PluginFatal(message) if message.contains("runtime.reload.protocol")
    ));
}

#[test]
fn storage_and_auth_plugins_are_managed_without_quarantine() {
    let storage = storage_entrypoints();
    let auth = offline_auth_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .storage_raw(InProcessStoragePlugin {
                plugin_id: "storage-je-anvil-1_7_10".to_string(),
                manifest: storage.manifest,
                api: storage.api,
            })
            .auth_raw(InProcessAuthPlugin {
                plugin_id: "auth-offline".to_string(),
                manifest: auth.manifest,
                api: auth.api,
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    let _loaded_plugins = test_support::load_protocol_plugin_set(&host)
        .expect("storage/auth plugin kinds should register with the host");

    let status = host.status();
    assert!(
        status
            .storage
            .iter()
            .find(|plugin| plugin.plugin_id == "storage-je-anvil-1_7_10")
            .and_then(|plugin| plugin.active_quarantine_reason.as_ref())
            .is_none()
    );
    assert!(
        status
            .auth
            .iter()
            .find(|plugin| plugin.plugin_id == "auth-offline")
            .and_then(|plugin| plugin.active_quarantine_reason.as_ref())
            .is_none()
    );
}

#[test]
fn gameplay_profiles_activate_and_resolve() {
    let canonical = canonical_gameplay_entrypoints();
    let readonly = readonly_gameplay_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .gameplay_raw(InProcessGameplayPlugin {
                plugin_id: "gameplay-canonical".to_string(),
                manifest: canonical.manifest,
                api: canonical.api,
            })
            .gameplay_raw(InProcessGameplayPlugin {
                plugin_id: "gameplay-readonly".to_string(),
                manifest: readonly.manifest,
                api: readonly.api,
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    test_support::activate_gameplay_profiles(
        &host,
        &ServerConfig {
            default_gameplay_profile: "canonical".to_string(),
            gameplay_profile_map: std::iter::once((
                "je-1_7_10".to_string(),
                "readonly".to_string(),
            ))
            .collect(),
            ..ServerConfig::default()
        },
    )
    .expect("known gameplay profiles should activate");

    assert!(test_support::resolve_gameplay_profile(&host, "canonical").is_some());
    assert!(test_support::resolve_gameplay_profile(&host, "readonly").is_some());
}

#[test]
fn load_plugin_set_activates_runtime_profiles() {
    let protocol = in_process_protocol_entrypoints();
    let canonical = canonical_gameplay_entrypoints();
    let storage = storage_entrypoints();
    let auth = offline_auth_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .protocol_raw(InProcessProtocolPlugin {
                plugin_id: "je-1_7_10".to_string(),
                manifest: protocol.manifest,
                api: protocol.api,
            })
            .gameplay_raw(InProcessGameplayPlugin {
                plugin_id: "gameplay-canonical".to_string(),
                manifest: canonical.manifest,
                api: canonical.api,
            })
            .storage_raw(InProcessStoragePlugin {
                plugin_id: "storage-je-anvil-1_7_10".to_string(),
                manifest: storage.manifest,
                api: storage.api,
            })
            .auth_raw(InProcessAuthPlugin {
                plugin_id: "auth-offline".to_string(),
                manifest: auth.manifest,
                api: auth.api,
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    let registries = host
        .load_plugin_set(&ServerConfig::default())
        .expect("load_plugin_set should initialize runtime profiles");

    assert!(
        registries
            .protocols()
            .resolve_adapter("je-1_7_10")
            .is_some()
    );
    assert!(
        registries
            .resolve_storage_profile("je-anvil-1_7_10")
            .is_some()
    );
    assert!(test_support::resolve_gameplay_profile(&host, "canonical").is_some());
    assert!(test_support::resolve_storage_profile(&host, "je-anvil-1_7_10").is_some());
    assert!(test_support::resolve_auth_profile(&host, "offline-v1").is_some());
}

#[test]
fn gameplay_command_snapshot_preserves_entity_id() {
    use mc_core::{
        BlockPos, BlockState, CapabilitySet, CoreCommand, DimensionId, EntityId, GameplayProfileId,
        GameplayQuery, PlayerId, SessionCapabilitySet, WorldMeta,
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

    let probe = entity_id_probe_gameplay_plugin::in_process_gameplay_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-entity-aware".to_string(),
            manifest: probe.manifest,
            api: probe.api,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    test_support::activate_gameplay_profiles(
        &host,
        &ServerConfig {
            default_gameplay_profile: "entity-aware".to_string(),
            ..ServerConfig::default()
        },
    )
    .expect("entity-aware gameplay profile should activate");

    let profile = test_support::resolve_gameplay_profile(&host, "entity-aware")
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
        let entrypoints = failing_protocol_plugin::in_process_entrypoints();
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
        let registries = test_support::load_protocol_plugin_set(&host)
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
        assert_eq!(
            test_support::take_pending_fatal_error(&host).is_some(),
            expect_fatal
        );
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
        CapabilitySet, CoreCommand, EntityId, GameplayProfileId, PlayerId, SessionCapabilitySet,
    };

    let cases = [
        (PluginFailureAction::Skip, false, false),
        (PluginFailureAction::Quarantine, true, false),
        (PluginFailureAction::FailFast, false, true),
    ];

    for (action, expect_quarantine, expect_fatal) in cases {
        let entrypoints = failing_gameplay_plugin::in_process_gameplay_entrypoints();
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
        test_support::activate_gameplay_profiles(
            &host,
            &ServerConfig {
                default_gameplay_profile: "failing".to_string(),
                ..ServerConfig::default()
            },
        )
        .expect("failing gameplay profile should activate");

        let profile = test_support::resolve_gameplay_profile(&host, "failing")
            .expect("failing gameplay profile should resolve");
        let result = profile.handle_command(
            &StubGameplayQuery {
                level_name: "world",
            },
            &SessionCapabilitySet {
                protocol: CapabilitySet::new(),
                gameplay: CapabilitySet::new(),
                gameplay_profile: GameplayProfileId::new("failing"),
                entity_id: Some(EntityId(9)),
                protocol_generation: None,
                gameplay_generation: None,
            },
            &CoreCommand::SetHeldSlot {
                player_id: PlayerId(Uuid::from_u128(77)),
                slot: 0,
            },
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
                    matches!(result, Err(message) if message.contains("gameplay runtime failure"))
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
        assert_eq!(
            test_support::take_pending_fatal_error(&host).is_some(),
            expect_fatal
        );
        if action == PluginFailureAction::Quarantine {
            assert!(
                profile
                    .handle_command(
                        &StubGameplayQuery {
                            level_name: "world",
                        },
                        &SessionCapabilitySet {
                            protocol: CapabilitySet::new(),
                            gameplay: CapabilitySet::new(),
                            gameplay_profile: GameplayProfileId::new("failing"),
                            entity_id: Some(EntityId(9)),
                            protocol_generation: None,
                            gameplay_generation: None,
                        },
                        &CoreCommand::SetHeldSlot {
                            player_id: PlayerId(Uuid::from_u128(77)),
                            slot: 1,
                        },
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
        let entrypoints = failing_auth_plugin::in_process_auth_entrypoints();
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
        test_support::activate_auth_profile(&host, failing_auth_plugin::PROFILE_ID)
            .expect("failing auth profile should activate");
        let profile = test_support::resolve_auth_profile(&host, failing_auth_plugin::PROFILE_ID)
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
        assert_eq!(
            test_support::take_pending_fatal_error(&host).is_some(),
            expect_fatal
        );
    }
}

#[test]
fn unknown_gameplay_profile_fails_activation() {
    let canonical = canonical_gameplay_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new().gameplay_raw(InProcessGameplayPlugin {
            plugin_id: "gameplay-canonical".to_string(),
            manifest: canonical.manifest,
            api: canonical.api,
        }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    let error = test_support::activate_gameplay_profiles(
        &host,
        &ServerConfig {
            default_gameplay_profile: "readonly".to_string(),
            ..ServerConfig::default()
        },
    )
    .expect_err("unknown gameplay profile should fail fast");
    assert!(matches!(
        error,
        RuntimeError::Config(message) if message.contains("unknown gameplay profile")
    ));
}

#[test]
fn storage_and_auth_profiles_activate_and_resolve() {
    let storage = storage_entrypoints();
    let auth = offline_auth_entrypoints();
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new()
            .storage_raw(InProcessStoragePlugin {
                plugin_id: "storage-je-anvil-1_7_10".to_string(),
                manifest: storage.manifest,
                api: storage.api,
            })
            .auth_raw(InProcessAuthPlugin {
                plugin_id: "auth-offline".to_string(),
                manifest: auth.manifest,
                api: auth.api,
            }),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    test_support::activate_storage_profile(&host, "je-anvil-1_7_10")
        .expect("known storage profile should activate");
    test_support::activate_auth_profile(&host, "offline-v1")
        .expect("known auth profile should activate");

    assert!(test_support::resolve_storage_profile(&host, "je-anvil-1_7_10").is_some());
    assert!(test_support::resolve_auth_profile(&host, "offline-v1").is_some());
}

#[test]
fn unknown_storage_and_auth_profiles_fail_activation() {
    let host = build_test_plugin_host(
        TestPluginHostBuilder::new(),
        PluginAbiRange::default(),
        PluginFailureMatrix::default(),
    );
    let storage = test_support::activate_storage_profile(&host, "missing")
        .expect_err("unknown storage profile should fail fast");
    assert!(matches!(
        storage,
        RuntimeError::Config(message) if message.contains("unknown storage profile")
    ));

    let auth = test_support::activate_auth_profile(&host, "missing")
        .expect_err("unknown auth profile should fail fast");
    assert!(matches!(
        auth,
        RuntimeError::Config(message) if message.contains("unknown auth profile")
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn packaged_protocol_plugins_load_via_dlopen() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    crate::seed_packaged_plugins_from_test_harness(
        &dist_dir,
        &["je-1_7_10", "je-1_8_x", "je-1_12_2", "be-placeholder"],
    )?;

    let config = ServerConfig {
        plugins_dir: dist_dir,
        ..ServerConfig::default()
    };
    let host = TestPluginHost::from_packaged(
        plugin_host_from_config(&config)?.expect("packaged plugins should be discovered"),
    );
    let registries = test_support::load_protocol_plugin_set(&host)?;

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
fn packaged_protocol_reload_replaces_generation() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("plugin-host-reload");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir, &["je-1_7_10"])?;

    let config = ServerConfig {
        plugins_dir: dist_dir.clone(),
        ..ServerConfig::default()
    };
    let host = TestPluginHost::from_packaged(
        plugin_host_from_config(&config)?.expect("packaged plugins should be discovered"),
    );
    let registries = test_support::load_protocol_plugin_set(&host)?;

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

    let reloaded = test_support::reload_modified(&host)?;
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

#[cfg(target_os = "linux")]
#[test]
fn packaged_protocol_reload_with_context_migrates_protocol_sessions() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("plugin-host-protocol-migrate");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir, &["je-1_7_10"])?;
    package_single_protocol_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        "je-1_7_10",
        &dist_dir,
        &target_dir,
        "protocol-reload-v1",
    )?;

    let config = ServerConfig {
        plugins_dir: dist_dir.clone(),
        ..ServerConfig::default()
    };
    let host = TestPluginHost::from_packaged(
        plugin_host_from_config(&config)?.expect("packaged plugins should be discovered"),
    );
    let registries = test_support::load_protocol_plugin_set(&host)?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("packaged je-1_7_10 adapter should resolve");
    let before_generation = adapter
        .plugin_generation_id()
        .expect("packaged adapter should report plugin generation");
    assert!(
        adapter
            .capability_set()
            .contains("build-tag:protocol-reload-v1")
    );

    let player_id = PlayerId(Uuid::from_u128(7));
    let context = protocol_reload_context(vec![
        protocol_reload_session(3, ConnectionPhase::Login, None, None),
        protocol_reload_session(
            11,
            ConnectionPhase::Play,
            Some(player_id),
            Some(EntityId(41)),
        ),
    ]);

    std::thread::sleep(Duration::from_secs(1));
    package_single_protocol_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        "je-1_7_10",
        &dist_dir,
        &target_dir,
        "protocol-reload-v2",
    )?;

    let reloaded = test_support::reload_modified_with_context(&host, &context)?;
    assert!(
        reloaded.iter().any(|plugin_id| plugin_id == "je-1_7_10"),
        "protocol reload should report the migrated adapter"
    );

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("reloaded adapter should resolve");
    let next_generation = adapter
        .plugin_generation_id()
        .expect("reloaded adapter should report plugin generation");
    assert_ne!(before_generation, next_generation);
    assert!(
        adapter
            .capability_set()
            .contains("build-tag:protocol-reload-v2")
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn packaged_protocol_reload_with_context_is_all_or_nothing() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("plugin-host-protocol-all-or-nothing");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir, &["je-1_7_10"])?;
    package_single_protocol_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        "je-1_7_10",
        &dist_dir,
        &target_dir,
        "protocol-reload-v1",
    )?;

    let config = ServerConfig {
        plugins_dir: dist_dir.clone(),
        ..ServerConfig::default()
    };
    let host = TestPluginHost::from_packaged(
        plugin_host_from_config(&config)?.expect("packaged plugins should be discovered"),
    );
    let registries = test_support::load_protocol_plugin_set(&host)?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("packaged je-1_7_10 adapter should resolve");
    let before_generation = adapter
        .plugin_generation_id()
        .expect("packaged adapter should report plugin generation");

    let player_id = PlayerId(Uuid::from_u128(9));
    let context = protocol_reload_context(vec![
        protocol_reload_session(5, ConnectionPhase::Login, None, None),
        protocol_reload_session(
            17,
            ConnectionPhase::Play,
            Some(player_id),
            Some(EntityId(55)),
        ),
    ]);

    std::thread::sleep(Duration::from_secs(1));
    package_single_protocol_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        "je-1_7_10",
        &dist_dir,
        &target_dir,
        "protocol-reload-fail",
    )?;

    let reloaded = test_support::reload_modified_with_context(&host, &context)?;
    assert!(
        !reloaded.iter().any(|plugin_id| plugin_id == "je-1_7_10"),
        "failed protocol migration should keep the current generation"
    );

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("adapter should still resolve after failed reload");
    assert_eq!(adapter.plugin_generation_id(), Some(before_generation));
    assert!(
        adapter
            .capability_set()
            .contains("build-tag:protocol-reload-v1")
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn packaged_protocol_reload_rejects_incompatible_candidate() -> Result<(), RuntimeError> {
    let temp_dir = tempdir()?;
    let dist_dir = temp_dir.path().join("runtime").join("plugins");
    let target_dir = crate::packaged_plugin_test_target_dir("plugin-host-protocol-incompatible");
    crate::seed_packaged_plugins_from_test_harness(&dist_dir, &["je-1_7_10"])?;
    package_single_protocol_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        "je-1_7_10",
        &dist_dir,
        &target_dir,
        "protocol-reload-v1",
    )?;

    let config = ServerConfig {
        plugins_dir: dist_dir.clone(),
        ..ServerConfig::default()
    };
    let host = TestPluginHost::from_packaged(
        plugin_host_from_config(&config)?.expect("packaged plugins should be discovered"),
    );
    let registries = test_support::load_protocol_plugin_set(&host)?;

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("packaged je-1_7_10 adapter should resolve");
    let before_generation = adapter
        .plugin_generation_id()
        .expect("packaged adapter should report plugin generation");

    std::thread::sleep(Duration::from_secs(1));
    package_single_protocol_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        "je-1_7_10",
        &dist_dir,
        &target_dir,
        "protocol-reload-incompatible",
    )?;

    let reloaded = test_support::reload_modified(&host)?;
    assert!(
        !reloaded.iter().any(|plugin_id| plugin_id == "je-1_7_10"),
        "incompatible protocol candidate should be rejected"
    );

    let adapter = registries
        .protocols()
        .resolve_adapter("je-1_7_10")
        .expect("adapter should still resolve after incompatible reload");
    assert_eq!(adapter.plugin_generation_id(), Some(before_generation));
    assert!(
        adapter
            .capability_set()
            .contains("build-tag:protocol-reload-v1")
    );

    let status = host.status();
    let protocol = status
        .protocols
        .iter()
        .find(|plugin| plugin.plugin_id == "je-1_7_10")
        .expect("je-1_7_10 status snapshot should remain present");
    assert_eq!(protocol.generation_id, before_generation);
    assert!(protocol.loaded_at_ms > 0);
    assert_eq!(
        protocol.current_artifact.modified_at_ms,
        protocol.loaded_at_ms
    );
    assert!(protocol.artifact_quarantine.is_some());
    assert!(protocol.current_artifact.reason.is_none());

    std::thread::sleep(Duration::from_secs(1));
    package_single_protocol_plugin(
        "mc-plugin-proto-je-1_7_10-reload-test",
        "je-1_7_10",
        &dist_dir,
        &target_dir,
        "protocol-reload-v2",
    )?;
    let reloaded = test_support::reload_modified(&host)?;
    assert!(
        reloaded.iter().any(|plugin_id| plugin_id == "je-1_7_10"),
        "successful replacement should clear the quarantined artifact"
    );

    let status = host.status();
    let protocol = status
        .protocols
        .iter()
        .find(|plugin| plugin.plugin_id == "je-1_7_10")
        .expect("je-1_7_10 status snapshot should remain present");
    assert!(protocol.artifact_quarantine.is_none());
    assert!(protocol.generation_id > before_generation);
    assert!(protocol.loaded_at_ms > 0);
    Ok(())
}

#[test]
fn gameplay_query_tls_restores_previous_query_when_nested() {
    let outer = StubGameplayQuery {
        level_name: "outer",
    };
    let inner = StubGameplayQuery {
        level_name: "inner",
    };

    let observed = with_gameplay_query(&outer, || {
        let outer_name = with_current_gameplay_query(|query| Ok(query.world_meta().level_name))?;
        let inner_name = with_gameplay_query(&inner, || {
            with_current_gameplay_query(|query| Ok(query.world_meta().level_name))
        })?;
        let restored_name = with_current_gameplay_query(|query| Ok(query.world_meta().level_name))?;
        Ok((outer_name, inner_name, restored_name))
    })
    .expect("nested gameplay queries should succeed");

    assert_eq!(
        observed,
        (
            "outer".to_string(),
            "inner".to_string(),
            "outer".to_string()
        )
    );
}

#[test]
fn gameplay_query_tls_requires_an_active_query() {
    let error = with_current_gameplay_query(|query| Ok(query.world_meta().level_name))
        .expect_err("gameplay query access should fail outside callback scope");
    assert!(error.contains("without an active query"));
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
) -> Result<(), RuntimeError> {
    crate::install_packaged_plugin_from_test_cache(
        cargo_package,
        plugin_id,
        "protocol",
        dist_dir,
        target_dir,
        build_tag,
    )
}
