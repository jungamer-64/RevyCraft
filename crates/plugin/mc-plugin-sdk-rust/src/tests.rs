use super::{__macro_support, admin_ui, capabilities, gameplay, manifest, protocol};
use bytes::BytesMut;
use mc_core::{
    BlockPos, CapabilityAnnouncement, CoreCommand, CoreEvent, DimensionId, GameplayCapability,
    GameplayProfileId, PlayerId, PlayerSnapshot, ProtocolCapability, ProtocolCapabilitySet,
    WorldMeta,
};
use mc_plugin_api::abi::{ByteSlice, CURRENT_PLUGIN_ABI, OwnedBuffer, PluginErrorCode};
use mc_plugin_api::codec::admin_ui::{
    AdminPermission, AdminPrincipal, AdminRequest, AdminResponse, AdminUiDescriptor, AdminUiInput,
    AdminUiOutput, decode_admin_ui_output, encode_admin_ui_input,
};
use mc_plugin_api::codec::gameplay::{
    GameplayDescriptor, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
    decode_gameplay_response, encode_gameplay_request, host_blob::encode_world_meta,
};
use mc_plugin_api::codec::protocol::{ProtocolRequest, ProtocolResponse, WireFrameDecodeResult};
use mc_plugin_api::host_api::{GameplayHostApiV2, GameplayPluginApiV3, HostApiTableV1};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeIntent, HandshakeProbe, LoginRequest, PlayEncodingContext,
    ProtocolAdapter, ProtocolDescriptor, ProtocolError, ServerListStatus, SessionAdapter,
    StatusRequest, TransportKind, WireCodec, WireFormatKind,
};
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

#[repr(C)]
struct TestHostContext {
    level_name: &'static str,
}

fn write_test_buffer(output: *mut OwnedBuffer, mut bytes: Vec<u8>) {
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

unsafe extern "C" fn host_read_world_meta(
    context: *mut c_void,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let Some(context) = (unsafe { (context as *const TestHostContext).as_ref() }) else {
        write_test_buffer(error_out, b"missing host context".to_vec());
        return PluginErrorCode::InvalidInput;
    };
    let bytes = encode_world_meta(&WorldMeta {
        level_name: context.level_name.to_string(),
        seed: 0,
        spawn: BlockPos::new(0, 64, 0),
        dimension: DimensionId::Overworld,
        age: 0,
        time: 0,
        level_type: "FLAT".to_string(),
        game_mode: 0,
        difficulty: 1,
        max_players: 20,
    })
    .expect("test world meta should encode");
    write_test_buffer(output, bytes);
    PluginErrorCode::Ok
}

fn gameplay_host_api_for(context: &TestHostContext) -> GameplayHostApiV2 {
    GameplayHostApiV2 {
        abi: CURRENT_PLUGIN_ABI,
        context: std::ptr::from_ref(context).cast_mut().cast(),
        log: None,
        read_player_snapshot: None,
        read_world_meta: Some(host_read_world_meta),
        read_block_state: None,
        read_block_entity: None,
        can_edit_block: None,
        set_player_pose: None,
        set_selected_hotbar_slot: None,
        set_inventory_slot: None,
        clear_mining: None,
        begin_mining: None,
        open_chest: None,
        open_furnace: None,
        set_block: None,
        spawn_dropped_item: None,
        emit_event: None,
    }
}

fn admin_ui_host_api_for(context: &TestHostContext) -> HostApiTableV1 {
    HostApiTableV1 {
        abi: CURRENT_PLUGIN_ABI,
        context: std::ptr::from_ref(context).cast_mut().cast(),
        log: None,
        read_player_snapshot: None,
        read_world_meta: Some(host_read_world_meta),
        read_block_state: None,
        read_block_entity: None,
        can_edit_block: None,
    }
}

#[derive(Default)]
struct DirectProbePlugin;

impl gameplay::RustGameplayPlugin for DirectProbePlugin {
    fn descriptor(&self) -> GameplayDescriptor {
        GameplayDescriptor {
            profile: GameplayProfileId::new("probe"),
        }
    }

    fn handle_tick(
        &self,
        host: &dyn gameplay::GameplayHost,
        _session: &GameplaySessionSnapshot,
        _now_ms: u64,
    ) -> Result<(), String> {
        let world_meta = host.read_world_meta()?;
        if world_meta.level_name.is_empty() {
            return Err("world meta should not be empty".to_string());
        }
        Ok(())
    }
}

struct TestProtocolWireCodec;

impl WireCodec for TestProtocolWireCodec {
    fn encode_frame(&self, payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
        let length = u8::try_from(payload.len())
            .map_err(|_| ProtocolError::InvalidPacket("test frame too large"))?;
        let mut frame = vec![length];
        frame.extend_from_slice(payload);
        Ok(frame)
    }

    fn try_decode_frame(&self, buffer: &mut BytesMut) -> Result<Option<Vec<u8>>, ProtocolError> {
        let Some(length) = buffer.first().copied() else {
            return Ok(None);
        };
        let frame_len = 1 + usize::from(length);
        if buffer.len() < frame_len {
            return Ok(None);
        }
        let frame = buffer[1..frame_len].to_vec();
        let _ = buffer.split_to(frame_len);
        Ok(Some(frame))
    }
}

#[derive(Default)]
struct DirectProtocolPlugin;

impl HandshakeProbe for DirectProtocolPlugin {
    fn transport_kind(&self) -> TransportKind {
        TransportKind::Tcp
    }

    fn try_route(&self, _frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        Ok(None)
    }
}

impl SessionAdapter for DirectProtocolPlugin {
    fn wire_codec(&self) -> &dyn WireCodec {
        static CODEC: TestProtocolWireCodec = TestProtocolWireCodec;
        &CODEC
    }

    fn decode_status(&self, _frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        Err(ProtocolError::InvalidPacket("unused test protocol method"))
    }

    fn decode_login(&self, _frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        Err(ProtocolError::InvalidPacket("unused test protocol method"))
    }

    fn encode_status_response(&self, _status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        Err(ProtocolError::InvalidPacket("unused test protocol method"))
    }

    fn encode_status_pong(&self, _payload: i64) -> Result<Vec<u8>, ProtocolError> {
        Err(ProtocolError::InvalidPacket("unused test protocol method"))
    }

    fn encode_disconnect(
        &self,
        _phase: ConnectionPhase,
        _reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(ProtocolError::InvalidPacket("unused test protocol method"))
    }

    fn encode_encryption_request(
        &self,
        _server_id: &str,
        _public_key_der: &[u8],
        _verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(ProtocolError::InvalidPacket("unused test protocol method"))
    }

    fn encode_network_settings(
        &self,
        _compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        Err(ProtocolError::InvalidPacket("unused test protocol method"))
    }

    fn encode_login_success(&self, _player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
        Err(ProtocolError::InvalidPacket("unused test protocol method"))
    }
}

impl mc_proto_common::PlaySyncAdapter for DirectProtocolPlugin {
    fn decode_play(
        &self,
        _session: &mc_proto_common::ProtocolSessionSnapshot,
        _frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError> {
        Err(ProtocolError::InvalidPacket("unused test protocol method"))
    }

    fn encode_play_event(
        &self,
        _event: &CoreEvent,
        _session: &mc_proto_common::ProtocolSessionSnapshot,
        _context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        Err(ProtocolError::InvalidPacket("unused test protocol method"))
    }
}

impl ProtocolAdapter for DirectProtocolPlugin {
    fn descriptor(&self) -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: "direct-probe".to_string(),
            transport: TransportKind::Tcp,
            wire_format: WireFormatKind::MinecraftFramed,
            edition: Edition::Je,
            version_name: "test".to_string(),
            protocol_number: 0,
        }
    }
}

impl protocol::RustProtocolPlugin for DirectProtocolPlugin {}

fn utf8_slice_to_string(slice: mc_plugin_api::abi::Utf8Slice) -> String {
    let bytes = unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) };
    std::str::from_utf8(bytes)
        .expect("manifest utf8 slice should be valid")
        .to_string()
}

fn manifest_capability_names(manifest: &mc_plugin_api::manifest::PluginManifestV1) -> Vec<String> {
    if manifest.capabilities.is_null() {
        return Vec::new();
    }
    let descriptors =
        unsafe { std::slice::from_raw_parts(manifest.capabilities, manifest.capabilities_len) };
    descriptors
        .iter()
        .map(|descriptor| utf8_slice_to_string(descriptor.name))
        .collect()
}

fn test_player_id() -> PlayerId {
    PlayerId(unsafe { std::mem::zeroed() })
}

fn gameplay_session(profile: &str, player_id: Option<PlayerId>) -> GameplaySessionSnapshot {
    GameplaySessionSnapshot {
        phase: ConnectionPhase::Play,
        player_id,
        entity_id: None,
        protocol: ProtocolCapabilitySet::new(),
        gameplay_profile: GameplayProfileId::new(profile),
        protocol_generation: None,
        gameplay_generation: None,
    }
}

#[test]
fn direct_protocol_requests_route_wire_codec_ops_through_plugin_codec() {
    assert_eq!(
        __macro_support::handle_protocol_request(
            &DirectProtocolPlugin,
            ProtocolRequest::EncodeWireFrame {
                payload: vec![0xaa, 0xbb, 0xcc],
            },
        )
        .expect("wire frame should encode"),
        ProtocolResponse::Frame(vec![3, 0xaa, 0xbb, 0xcc])
    );

    assert_eq!(
        __macro_support::handle_protocol_request(
            &DirectProtocolPlugin,
            ProtocolRequest::TryDecodeWireFrame {
                buffer: vec![3, 0xaa, 0xbb, 0xcc, 0xff],
            },
        )
        .expect("wire frame should decode"),
        ProtocolResponse::WireFrameDecodeResult(Some(WireFrameDecodeResult {
            frame: vec![0xaa, 0xbb, 0xcc],
            bytes_consumed: 4,
        }))
    );

    assert_eq!(
        __macro_support::handle_protocol_request(
            &DirectProtocolPlugin,
            ProtocolRequest::TryDecodeWireFrame {
                buffer: vec![3, 0xaa]
            },
        )
        .expect("incomplete frame should stay buffered"),
        ProtocolResponse::WireFrameDecodeResult(None)
    );
}

#[test]
fn direct_gameplay_requests_require_host_api_for_host_callbacks() {
    let request = GameplayRequest::HandleTick {
        session: gameplay_session("probe", None),
        now_ms: 0,
    };
    let error =
        __macro_support::handle_gameplay_request_with_host_api(&DirectProbePlugin, request, None)
            .expect_err("host callbacks should require configured host api");
    assert!(error.contains("gameplay host api is not configured"));
}

#[test]
fn capability_helpers_add_build_tags_without_changing_base_names() {
    let mut announcement = CapabilityAnnouncement::new(capabilities::protocol_capabilities(&[
        ProtocolCapability::RuntimeReload,
        ProtocolCapability::Je,
    ]));
    announcement.build_tag = Some("protocol-reload-v2".into());

    assert!(announcement.contains(ProtocolCapability::RuntimeReload));
    assert!(announcement.contains(ProtocolCapability::Je));
    assert_eq!(
        announcement
            .build_tag
            .as_ref()
            .map(mc_core::PluginBuildTag::as_str),
        Some("protocol-reload-v2")
    );
}

#[test]
fn build_tag_contains_uses_the_pure_helper_logic() {
    assert!(capabilities::build_tag_contains_in(
        Some("protocol-reload-fail-v2"),
        "reload-fail",
    ));
    assert!(!capabilities::build_tag_contains_in(
        Some("protocol-reload-v2"),
        "reload-fail",
    ));
    assert!(!capabilities::build_tag_contains_in(None, "reload-fail"));
}

#[allow(unexpected_cfgs)]
mod plugin_a {
    use super::*;
    use crate::export_plugin;

    #[derive(Default)]
    pub struct PluginA;

    fn recorded_slot() -> &'static Mutex<Option<String>> {
        static RECORDED: OnceLock<Mutex<Option<String>>> = OnceLock::new();
        RECORDED.get_or_init(|| Mutex::new(None))
    }

    pub fn take_recorded_level_name() -> Option<String> {
        recorded_slot()
            .lock()
            .expect("recorded level name mutex should not be poisoned")
            .take()
    }

    impl gameplay::RustGameplayPlugin for PluginA {
        fn descriptor(&self) -> GameplayDescriptor {
            gameplay::gameplay_descriptor("plugin-a")
        }

        fn capability_set(&self) -> mc_core::GameplayCapabilitySet {
            capabilities::gameplay_capabilities(&[GameplayCapability::RuntimeReload])
        }

        fn handle_tick(
            &self,
            host: &dyn gameplay::GameplayHost,
            _session: &GameplaySessionSnapshot,
            _now_ms: u64,
        ) -> Result<(), String> {
            *recorded_slot()
                .lock()
                .expect("recorded level name mutex should not be poisoned") =
                Some(host.read_world_meta()?.level_name);
            Ok(())
        }
    }

    const MANIFEST: manifest::StaticPluginManifest =
        manifest::StaticPluginManifest::gameplay("plugin-a", "Plugin A", "plugin-a");

    export_plugin!(gameplay, PluginA, MANIFEST);
}

#[allow(unexpected_cfgs)]
mod plugin_b {
    use super::*;
    use crate::export_plugin;

    #[derive(Default)]
    pub struct PluginB;

    fn recorded_slot() -> &'static Mutex<Option<String>> {
        static RECORDED: OnceLock<Mutex<Option<String>>> = OnceLock::new();
        RECORDED.get_or_init(|| Mutex::new(None))
    }

    pub fn take_recorded_level_name() -> Option<String> {
        recorded_slot()
            .lock()
            .expect("recorded level name mutex should not be poisoned")
            .take()
    }

    impl gameplay::RustGameplayPlugin for PluginB {
        fn descriptor(&self) -> GameplayDescriptor {
            gameplay::gameplay_descriptor("plugin-b")
        }

        fn capability_set(&self) -> mc_core::GameplayCapabilitySet {
            capabilities::gameplay_capabilities(&[GameplayCapability::RuntimeReload])
        }

        fn handle_tick(
            &self,
            host: &dyn gameplay::GameplayHost,
            _session: &GameplaySessionSnapshot,
            _now_ms: u64,
        ) -> Result<(), String> {
            *recorded_slot()
                .lock()
                .expect("recorded level name mutex should not be poisoned") =
                Some(host.read_world_meta()?.level_name);
            Ok(())
        }
    }

    const MANIFEST: manifest::StaticPluginManifest =
        manifest::StaticPluginManifest::gameplay("plugin-b", "Plugin B", "plugin-b");

    export_plugin!(gameplay, PluginB, MANIFEST);
}

#[allow(unexpected_cfgs)]
mod console_admin_ui_plugin {
    use super::*;
    use crate::export_plugin;

    #[derive(Default)]
    pub struct ConsoleAdminUiPlugin;

    impl admin_ui::RustAdminUiPlugin for ConsoleAdminUiPlugin {
        fn descriptor(&self) -> AdminUiDescriptor {
            AdminUiDescriptor {
                ui_profile: "console-v1".into(),
            }
        }

        fn parse_line(&self, line: &str) -> Result<AdminRequest, String> {
            match line.trim() {
                "status" => Ok(AdminRequest::Status),
                "help" => Ok(AdminRequest::Help),
                other => Err(format!("unknown command `{other}`")),
            }
        }

        fn render_response(&self, response: &AdminResponse) -> Result<String, String> {
            Ok(match response {
                AdminResponse::Status(_) => "status".to_string(),
                AdminResponse::Help => "help".to_string(),
                AdminResponse::PermissionDenied {
                    principal,
                    permission,
                } => format!(
                    "permission denied: principal={} permission={}",
                    principal.as_str(),
                    permission.as_str()
                ),
                other => format!("{other:?}"),
            })
        }
    }

    const MANIFEST: manifest::StaticPluginManifest = manifest::StaticPluginManifest::admin_ui(
        "admin-ui-test",
        "Admin UI Test Plugin",
        "console-v1",
    );

    export_plugin!(admin_ui, ConsoleAdminUiPlugin, MANIFEST);
}

#[allow(unexpected_cfgs)]
mod declared_protocol_plugin {
    use super::*;
    use crate::protocol::declare_protocol_plugin;

    #[derive(Default)]
    struct DeclaredProtocolAdapter;

    impl HandshakeProbe for DeclaredProtocolAdapter {
        fn transport_kind(&self) -> TransportKind {
            TransportKind::Tcp
        }

        fn adapter_id(&self) -> Option<&'static str> {
            Some("declared-probe")
        }

        fn try_route(&self, _frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
            Ok(None)
        }
    }

    impl SessionAdapter for DeclaredProtocolAdapter {
        fn wire_codec(&self) -> &dyn WireCodec {
            static CODEC: TestProtocolWireCodec = TestProtocolWireCodec;
            &CODEC
        }

        fn decode_status(&self, _frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused declared protocol method",
            ))
        }

        fn decode_login(&self, _frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused declared protocol method",
            ))
        }

        fn encode_status_response(
            &self,
            _status: &ServerListStatus,
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused declared protocol method",
            ))
        }

        fn encode_status_pong(&self, _payload: i64) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused declared protocol method",
            ))
        }

        fn encode_disconnect(
            &self,
            _phase: ConnectionPhase,
            _reason: &str,
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused declared protocol method",
            ))
        }

        fn encode_encryption_request(
            &self,
            _server_id: &str,
            _public_key_der: &[u8],
            _verify_token: &[u8],
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused declared protocol method",
            ))
        }

        fn encode_network_settings(
            &self,
            _compression_threshold: u16,
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused declared protocol method",
            ))
        }

        fn encode_login_success(&self, _player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused declared protocol method",
            ))
        }
    }

    impl mc_proto_common::PlaySyncAdapter for DeclaredProtocolAdapter {
        fn decode_play(
            &self,
            _session: &mc_proto_common::ProtocolSessionSnapshot,
            _frame: &[u8],
        ) -> Result<Option<CoreCommand>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused declared protocol method",
            ))
        }

        fn encode_play_event(
            &self,
            _event: &CoreEvent,
            _session: &mc_proto_common::ProtocolSessionSnapshot,
            _context: &PlayEncodingContext,
        ) -> Result<Vec<Vec<u8>>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused declared protocol method",
            ))
        }
    }

    impl ProtocolAdapter for DeclaredProtocolAdapter {
        fn descriptor(&self) -> ProtocolDescriptor {
            ProtocolDescriptor {
                adapter_id: "declared-probe".to_string(),
                transport: TransportKind::Tcp,
                wire_format: WireFormatKind::MinecraftFramed,
                edition: Edition::Je,
                version_name: "test".to_string(),
                protocol_number: 0,
            }
        }
    }

    declare_protocol_plugin!(
        DeclaredProtocolPlugin,
        DeclaredProtocolAdapter,
        "declared-probe",
        "Declared Probe Protocol Plugin",
        &[ProtocolCapability::RuntimeReload],
    );
}

unsafe fn invoke_gameplay(
    api: &GameplayPluginApiV3,
    host_api: Option<&GameplayHostApiV2>,
    request: &GameplayRequest,
) -> GameplayResponse {
    let payload = encode_gameplay_request(request).expect("gameplay request should encode");
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
            ByteSlice {
                ptr: payload.as_ptr(),
                len: payload.len(),
            },
            host_api.map_or(std::ptr::null(), std::ptr::from_ref),
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        let message = if error.ptr.is_null() {
            format!("invoke failed with status {status:?}")
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
            unsafe {
                (api.free_buffer)(error);
            }
            String::from_utf8(bytes).expect("plugin error should be utf-8")
        };
        panic!("{message}");
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_gameplay_response(request, &bytes).expect("gameplay response should decode")
}

unsafe fn invoke_admin_ui(
    api: &mc_plugin_api::host_api::AdminUiPluginApiV1,
    host_api: Option<&HostApiTableV1>,
    request: &AdminUiInput,
) -> AdminUiOutput {
    let payload = encode_admin_ui_input(request).expect("admin-ui request should encode");
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
            ByteSlice {
                ptr: payload.as_ptr(),
                len: payload.len(),
            },
            host_api.map_or(std::ptr::null(), std::ptr::from_ref),
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        let message = if error.ptr.is_null() {
            format!("invoke failed with status {status:?}")
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
            unsafe {
                (api.free_buffer)(error);
            }
            String::from_utf8(bytes).expect("plugin error should be utf-8")
        };
        panic!("{message}");
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_admin_ui_output(request, &bytes).expect("admin-ui response should decode")
}

#[test]
fn exported_gameplay_plugins_keep_host_api_slots_isolated() {
    let context_a = TestHostContext {
        level_name: "host-a",
    };
    let context_b = TestHostContext {
        level_name: "host-b",
    };
    let host_api_a = gameplay_host_api_for(&context_a);
    let host_api_b = gameplay_host_api_for(&context_b);

    let entrypoints_a = plugin_a::in_process_plugin_entrypoints();
    let entrypoints_b = plugin_b::in_process_plugin_entrypoints();

    let request_a = GameplayRequest::HandleTick {
        session: gameplay_session("plugin-a", Some(test_player_id())),
        now_ms: 1,
    };
    let request_b = GameplayRequest::HandleTick {
        session: gameplay_session("plugin-b", Some(test_player_id())),
        now_ms: 2,
    };

    assert_eq!(
        unsafe { invoke_gameplay(entrypoints_a.api, Some(&host_api_a), &request_a) },
        GameplayResponse::Empty
    );
    assert_eq!(
        unsafe { invoke_gameplay(entrypoints_b.api, Some(&host_api_b), &request_b) },
        GameplayResponse::Empty
    );
    assert_eq!(
        plugin_a::take_recorded_level_name().as_deref(),
        Some("host-a")
    );
    assert_eq!(
        plugin_b::take_recorded_level_name().as_deref(),
        Some("host-b")
    );
}

#[test]
fn exported_gameplay_plugins_reject_null_host_api() {
    let entrypoints = plugin_a::in_process_plugin_entrypoints();
    let request = GameplayRequest::HandleTick {
        session: gameplay_session("plugin-a", Some(test_player_id())),
        now_ms: 3,
    };
    let payload = encode_gameplay_request(&request).expect("gameplay request should encode");
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (entrypoints.api.invoke)(
            ByteSlice {
                ptr: payload.as_ptr(),
                len: payload.len(),
            },
            std::ptr::null(),
            &raw mut output,
            &raw mut error,
        )
    };
    assert_eq!(status, PluginErrorCode::InvalidInput);
    let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
    unsafe {
        (entrypoints.api.free_buffer)(error);
    }
    assert_eq!(
        String::from_utf8(bytes).expect("plugin error should be utf-8"),
        "gameplay host api was null"
    );
}

#[test]
fn exported_gameplay_plugins_reject_mismatched_host_api_abi() {
    let context = TestHostContext {
        level_name: "host-a",
    };
    let mut host_api = gameplay_host_api_for(&context);
    host_api.abi = mc_plugin_api::abi::PluginAbiVersion { major: 2, minor: 0 };
    let entrypoints = plugin_a::in_process_plugin_entrypoints();
    let request = GameplayRequest::HandleTick {
        session: gameplay_session("plugin-a", Some(test_player_id())),
        now_ms: 4,
    };
    let payload = encode_gameplay_request(&request).expect("gameplay request should encode");
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (entrypoints.api.invoke)(
            ByteSlice {
                ptr: payload.as_ptr(),
                len: payload.len(),
            },
            &raw const host_api,
            &raw mut output,
            &raw mut error,
        )
    };
    assert_eq!(status, PluginErrorCode::AbiMismatch);
    let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
    unsafe {
        (entrypoints.api.free_buffer)(error);
    }
    assert_eq!(
        String::from_utf8(bytes).expect("plugin error should be utf-8"),
        "gameplay host api ABI 2.0 did not match plugin ABI 4.0"
    );
}

#[test]
fn exported_admin_ui_plugins_parse_and_render_round_trip() {
    let context_a = TestHostContext {
        level_name: "host-a",
    };
    let context_b = TestHostContext {
        level_name: "host-b",
    };
    let host_api_a = admin_ui_host_api_for(&context_a);
    let host_api_b = admin_ui_host_api_for(&context_b);
    let entrypoints = console_admin_ui_plugin::in_process_plugin_entrypoints();

    assert_eq!(
        unsafe {
            invoke_admin_ui(
                entrypoints.api,
                Some(&host_api_a),
                &AdminUiInput::ParseLine {
                    line: "status".to_string(),
                },
            )
        },
        AdminUiOutput::ParsedRequest(AdminRequest::Status)
    );
    assert_eq!(
        unsafe {
            invoke_admin_ui(
                entrypoints.api,
                Some(&host_api_b),
                &AdminUiInput::RenderResponse {
                    response: AdminResponse::PermissionDenied {
                        principal: AdminPrincipal::LocalConsole,
                        permission: AdminPermission::ReloadPlugins,
                    },
                },
            )
        },
        AdminUiOutput::RenderedText(
            "permission denied: principal=local-console permission=reload-plugins".to_string()
        )
    );
}

#[test]
fn exported_admin_ui_plugins_reject_null_host_api() {
    let entrypoints = console_admin_ui_plugin::in_process_plugin_entrypoints();
    let request = AdminUiInput::ParseLine {
        line: "status".to_string(),
    };
    let payload = encode_admin_ui_input(&request).expect("admin-ui request should encode");
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (entrypoints.api.invoke)(
            ByteSlice {
                ptr: payload.as_ptr(),
                len: payload.len(),
            },
            std::ptr::null(),
            &raw mut output,
            &raw mut error,
        )
    };
    assert_eq!(status, PluginErrorCode::InvalidInput);
    let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
    unsafe {
        (entrypoints.api.free_buffer)(error);
    }
    assert_eq!(
        String::from_utf8(bytes).expect("plugin error should be utf-8"),
        "admin-ui host api was null"
    );
}

#[test]
fn exported_admin_ui_plugins_reject_mismatched_host_api_abi() {
    let context = TestHostContext {
        level_name: "host-a",
    };
    let mut host_api = admin_ui_host_api_for(&context);
    host_api.abi = mc_plugin_api::abi::PluginAbiVersion { major: 2, minor: 0 };
    let entrypoints = console_admin_ui_plugin::in_process_plugin_entrypoints();
    let request = AdminUiInput::ParseLine {
        line: "status".to_string(),
    };
    let payload = encode_admin_ui_input(&request).expect("admin-ui request should encode");
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (entrypoints.api.invoke)(
            ByteSlice {
                ptr: payload.as_ptr(),
                len: payload.len(),
            },
            &raw const host_api,
            &raw mut output,
            &raw mut error,
        )
    };
    assert_eq!(status, PluginErrorCode::AbiMismatch);
    let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
    unsafe {
        (entrypoints.api.free_buffer)(error);
    }
    assert_eq!(
        String::from_utf8(bytes).expect("plugin error should be utf-8"),
        "admin-ui host api ABI 2.0 did not match plugin ABI 4.0"
    );
}

#[test]
fn declared_protocol_plugins_delegate_wire_codec_and_keep_manifest_capabilities() {
    assert_eq!(
        __macro_support::handle_protocol_request(
            &declared_protocol_plugin::DeclaredProtocolPlugin::default(),
            ProtocolRequest::EncodeWireFrame {
                payload: vec![0x10, 0x20],
            },
        )
        .expect("declared wire frame should encode"),
        ProtocolResponse::Frame(vec![2, 0x10, 0x20])
    );

    assert_eq!(
        __macro_support::handle_protocol_request(
            &declared_protocol_plugin::DeclaredProtocolPlugin::default(),
            ProtocolRequest::TryDecodeWireFrame {
                buffer: vec![2, 0x10, 0x20, 0xff],
            },
        )
        .expect("declared wire frame should decode"),
        ProtocolResponse::WireFrameDecodeResult(Some(WireFrameDecodeResult {
            frame: vec![0x10, 0x20],
            bytes_consumed: 3,
        }))
    );

    let entrypoints = declared_protocol_plugin::in_process_plugin_entrypoints();
    let capabilities = manifest_capability_names(entrypoints.manifest);
    assert_eq!(capabilities, vec!["runtime.reload.protocol".to_string()]);
}
