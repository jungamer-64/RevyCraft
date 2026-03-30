use super::{__macro_support, admin_surface, capabilities, gameplay, manifest, protocol};
use crate::{
    CapabilityAnnouncement, CoreEvent, GameplayCapability, GameplayEffectBatch, GameplayProfileId,
    GameplayReadSet, PlayerId, PlayerSnapshot, PluginBuildTag, ProtocolCapability,
    ProtocolCapabilitySet, RuntimeCommand,
};
use bytes::BytesMut;
use mc_plugin_api::abi::{ByteSlice, CURRENT_PLUGIN_ABI, OwnedBuffer, PluginErrorCode};
use mc_plugin_api::codec::admin::AdminPermission;
use mc_plugin_api::codec::admin_surface::{
    AdminSurfaceDescriptor, AdminSurfaceEndpointView, AdminSurfaceInstanceDeclaration,
    AdminSurfacePauseView, AdminSurfaceRequest, AdminSurfaceResponse, AdminSurfaceStatusView,
    encode_admin_surface_request,
};
use mc_plugin_api::codec::gameplay::{
    GameplayDescriptor, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
    encode_gameplay_request, host_blob::encode_world_meta,
};
use mc_plugin_api::codec::protocol::{ProtocolRequest, ProtocolResponse, WireFrameDecodeResult};
use mc_plugin_api::host_api::{AdminSurfaceHostApiV1, GameplayHostApiV3};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeIntent, HandshakeProbe, LoginRequest, PlayEncodingContext,
    ProtocolAdapter, ProtocolDescriptor, ProtocolError, ServerListStatus, SessionAdapter,
    StatusRequest, TransportKind, WireCodec, WireFormatKind,
};
use revy_voxel_model::{BlockPos, DimensionId, WorldMeta};
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

fn gameplay_host_api_for(context: &TestHostContext) -> GameplayHostApiV3 {
    GameplayHostApiV3 {
        abi: CURRENT_PLUGIN_ABI,
        context: std::ptr::from_ref(context).cast_mut().cast(),
        log: None,
        read_player_snapshot: None,
        read_world_meta: Some(host_read_world_meta),
        read_block_state: None,
        read_block_entity: None,
        can_edit_block: None,
        push_effect: None,
    }
}

unsafe extern "C" fn host_admin_surface_permissions(
    context: *mut c_void,
    _principal_id: mc_plugin_api::abi::Utf8Slice,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let Some(context) = (unsafe { (context as *const TestHostContext).as_ref() }) else {
        write_test_buffer(error_out, b"missing host context".to_vec());
        return PluginErrorCode::InvalidInput;
    };
    let permissions = match context.level_name {
        "host-a" => vec![AdminPermission::Status],
        "host-b" => vec![AdminPermission::Shutdown],
        _ => vec![AdminPermission::Sessions],
    };
    let bytes = serde_json::to_vec(&permissions).expect("permissions should encode");
    write_test_buffer(output, bytes);
    PluginErrorCode::Ok
}

fn admin_surface_host_api_for(context: &TestHostContext) -> AdminSurfaceHostApiV1 {
    AdminSurfaceHostApiV1 {
        abi: CURRENT_PLUGIN_ABI,
        context: std::ptr::from_ref(context).cast_mut().cast(),
        log: None,
        execute: None,
        permissions: Some(host_admin_surface_permissions),
        take_process_resource: None,
        publish_handoff_resource: None,
        take_handoff_resource: None,
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
    ) -> Result<Option<RuntimeCommand>, ProtocolError> {
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
        announcement.build_tag.as_ref().map(PluginBuildTag::as_str),
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

        fn capability_set(&self) -> crate::GameplayCapabilitySet {
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

        fn capability_set(&self) -> crate::GameplayCapabilitySet {
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
mod test_admin_surface_plugin {
    use super::*;
    use crate::export_plugin;

    #[derive(Default)]
    pub struct TestAdminSurfacePlugin;

    impl admin_surface::RustAdminSurfacePlugin for TestAdminSurfacePlugin {
        fn descriptor(&self) -> AdminSurfaceDescriptor {
            admin_surface::admin_surface_descriptor("console-v1")
        }

        fn declare_instance(
            &self,
            _instance_id: &str,
            _surface_config_path: Option<&str>,
        ) -> Result<AdminSurfaceInstanceDeclaration, String> {
            Ok(AdminSurfaceInstanceDeclaration {
                principals: Vec::new(),
                required_process_resources: Vec::new(),
                supports_upgrade_handoff: false,
            })
        }

        fn start(
            &self,
            instance_id: &str,
            host: admin_surface::SdkAdminSurfaceHost,
            _surface_config_path: Option<&str>,
        ) -> Result<AdminSurfaceStatusView, String> {
            let local_addr =
                crate::admin_surface::AdminSurfaceHost::permissions(&host, instance_id)?
                    .first()
                    .map(|permission| permission.as_str())
                    .unwrap_or("none")
                    .to_string();
            Ok(AdminSurfaceStatusView {
                endpoints: vec![AdminSurfaceEndpointView {
                    surface: instance_id.to_string(),
                    local_addr,
                }],
            })
        }

        fn pause_for_upgrade(
            &self,
            _instance_id: &str,
            _host: admin_surface::SdkAdminSurfaceHost,
        ) -> Result<AdminSurfacePauseView, String> {
            Ok(AdminSurfacePauseView {
                resume_payload: b"paused".to_vec(),
            })
        }

        fn resume_from_upgrade(
            &self,
            instance_id: &str,
            host: admin_surface::SdkAdminSurfaceHost,
            _surface_config_path: Option<&str>,
            _resume_payload: &[u8],
        ) -> Result<AdminSurfaceStatusView, String> {
            self.start(instance_id, host, None)
        }

        fn resume_after_upgrade_rollback(
            &self,
            instance_id: &str,
            host: admin_surface::SdkAdminSurfaceHost,
        ) -> Result<AdminSurfaceStatusView, String> {
            self.start(instance_id, host, None)
        }

        fn shutdown(
            &self,
            _instance_id: &str,
            _host: admin_surface::SdkAdminSurfaceHost,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    const MANIFEST: manifest::StaticPluginManifest = manifest::StaticPluginManifest::admin_surface(
        "admin-surface-test",
        "Admin Surface Test Plugin",
        "console-v1",
    );

    export_plugin!(admin_surface, TestAdminSurfacePlugin, MANIFEST);
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
        ) -> Result<Option<RuntimeCommand>, ProtocolError> {
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

#[allow(unexpected_cfgs)]
mod doc_style_protocol_plugin {
    use super::TestProtocolWireCodec;
    use crate::ProtocolCapability;
    use crate::protocol::declare_protocol_plugin;
    use mc_proto_common::{
        ConnectionPhase, Edition, HandshakeIntent, HandshakeProbe, LoginRequest,
        PlayEncodingContext, PlaySyncAdapter, ProtocolAdapter, ProtocolDescriptor, ProtocolError,
        ProtocolSessionSnapshot, RuntimeCommand, ServerListStatus, SessionAdapter, StatusRequest,
        TransportKind, WireCodec, WireFormatKind,
    };

    #[derive(Default)]
    pub struct DocStyleProtocolAdapter;

    impl HandshakeProbe for DocStyleProtocolAdapter {
        fn transport_kind(&self) -> TransportKind {
            TransportKind::Tcp
        }

        fn try_route(&self, _frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
            Ok(None)
        }
    }

    impl SessionAdapter for DocStyleProtocolAdapter {
        fn wire_codec(&self) -> &dyn WireCodec {
            static CODEC: TestProtocolWireCodec = TestProtocolWireCodec;
            &CODEC
        }

        fn decode_status(&self, _frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused doc-style protocol method",
            ))
        }

        fn decode_login(&self, _frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused doc-style protocol method",
            ))
        }

        fn encode_status_response(
            &self,
            _status: &ServerListStatus,
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused doc-style protocol method",
            ))
        }

        fn encode_status_pong(&self, _payload: i64) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused doc-style protocol method",
            ))
        }

        fn encode_disconnect(
            &self,
            _phase: ConnectionPhase,
            _reason: &str,
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused doc-style protocol method",
            ))
        }

        fn encode_encryption_request(
            &self,
            _server_id: &str,
            _public_key_der: &[u8],
            _verify_token: &[u8],
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused doc-style protocol method",
            ))
        }

        fn encode_network_settings(
            &self,
            _compression_threshold: u16,
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused doc-style protocol method",
            ))
        }

        fn encode_login_success(
            &self,
            _player: &crate::PlayerSnapshot,
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused doc-style protocol method",
            ))
        }
    }

    impl PlaySyncAdapter for DocStyleProtocolAdapter {
        fn decode_play(
            &self,
            _session: &ProtocolSessionSnapshot,
            _frame: &[u8],
        ) -> Result<Option<RuntimeCommand>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused doc-style protocol method",
            ))
        }

        fn encode_play_event(
            &self,
            _event: &crate::CoreEvent,
            _session: &ProtocolSessionSnapshot,
            _context: &PlayEncodingContext,
        ) -> Result<Vec<Vec<u8>>, ProtocolError> {
            Err(ProtocolError::InvalidPacket(
                "unused doc-style protocol method",
            ))
        }
    }

    impl ProtocolAdapter for DocStyleProtocolAdapter {
        fn descriptor(&self) -> ProtocolDescriptor {
            ProtocolDescriptor {
                adapter_id: "doc-style-probe".to_string(),
                transport: TransportKind::Tcp,
                wire_format: WireFormatKind::MinecraftFramed,
                edition: Edition::Je,
                version_name: "docs".to_string(),
                protocol_number: 0,
            }
        }
    }

    declare_protocol_plugin!(
        DocStyleProtocolPlugin,
        DocStyleProtocolAdapter,
        "doc-style-probe",
        "Doc Style Protocol Plugin",
        &[ProtocolCapability::RuntimeReload, ProtocolCapability::Je,],
    );
}

#[allow(unexpected_cfgs)]
mod doc_style_gameplay_plugin {
    use crate::capabilities::gameplay_capabilities;
    use crate::export_plugin;
    use crate::gameplay::{RustGameplayPlugin, gameplay_descriptor};
    use crate::manifest::StaticPluginManifest;
    use crate::{GameplayCapability, GameplayCapabilitySet};
    use mc_plugin_api::codec::gameplay::GameplayDescriptor;

    #[derive(Default)]
    pub struct DocStyleGameplayPlugin;

    impl RustGameplayPlugin for DocStyleGameplayPlugin {
        fn descriptor(&self) -> GameplayDescriptor {
            gameplay_descriptor("doc-style")
        }

        fn capability_set(&self) -> GameplayCapabilitySet {
            gameplay_capabilities(&[GameplayCapability::RuntimeReload])
        }
    }

    const MANIFEST: StaticPluginManifest = StaticPluginManifest::gameplay(
        "doc-style-gameplay",
        "Doc Style Gameplay Plugin",
        "doc-style",
    );

    export_plugin!(gameplay, DocStyleGameplayPlugin, MANIFEST);
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
        (entrypoints_a.factory)()
            .handle(request_a, Some(host_api_a))
            .expect("in-process gameplay handler should succeed"),
        GameplayResponse::EffectBatch(GameplayEffectBatch {
            now_ms: 1,
            reads: GameplayReadSet {
                world_meta: Some(WorldMeta {
                    level_name: "host-a".to_string(),
                    seed: 0,
                    spawn: BlockPos::new(0, 64, 0),
                    dimension: DimensionId::Overworld,
                    age: 0,
                    time: 0,
                    level_type: "FLAT".to_string(),
                    game_mode: 0,
                    difficulty: 1,
                    max_players: 20,
                }),
                ..GameplayReadSet::default()
            },
            effects: Vec::new(),
        })
    );
    assert_eq!(
        (entrypoints_b.factory)()
            .handle(request_b, Some(host_api_b))
            .expect("in-process gameplay handler should succeed"),
        GameplayResponse::EffectBatch(GameplayEffectBatch {
            now_ms: 2,
            reads: GameplayReadSet {
                world_meta: Some(WorldMeta {
                    level_name: "host-b".to_string(),
                    seed: 0,
                    spawn: BlockPos::new(0, 64, 0),
                    dimension: DimensionId::Overworld,
                    age: 0,
                    time: 0,
                    level_type: "FLAT".to_string(),
                    game_mode: 0,
                    difficulty: 1,
                    max_players: 20,
                }),
                ..GameplayReadSet::default()
            },
            effects: Vec::new(),
        })
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
    let api = unsafe { &*plugin_a::mc_plugin_gameplay_api_v4() };
    let request = GameplayRequest::HandleTick {
        session: gameplay_session("plugin-a", Some(test_player_id())),
        now_ms: 3,
    };
    let payload = encode_gameplay_request(&request).expect("gameplay request should encode");
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
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
        (api.free_buffer)(error);
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
    let api = unsafe { &*plugin_a::mc_plugin_gameplay_api_v4() };
    let request = GameplayRequest::HandleTick {
        session: gameplay_session("plugin-a", Some(test_player_id())),
        now_ms: 4,
    };
    let payload = encode_gameplay_request(&request).expect("gameplay request should encode");
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
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
        (api.free_buffer)(error);
    }
    assert_eq!(
        String::from_utf8(bytes).expect("plugin error should be utf-8"),
        format!(
            "gameplay host api ABI 2.0 did not match plugin ABI {}",
            CURRENT_PLUGIN_ABI
        )
    );
}

#[test]
fn exported_admin_surface_plugins_start_round_trip_uses_host_api_callbacks() {
    let context_a = TestHostContext {
        level_name: "host-a",
    };
    let context_b = TestHostContext {
        level_name: "host-b",
    };
    let host_api_a = admin_surface_host_api_for(&context_a);
    let host_api_b = admin_surface_host_api_for(&context_b);
    let entrypoints = test_admin_surface_plugin::in_process_plugin_entrypoints();
    let handler = (entrypoints.factory)();

    assert_eq!(
        handler
            .handle(
                AdminSurfaceRequest::Start {
                    instance_id: "console-a".to_string(),
                    surface_config_path: None,
                },
                Some(host_api_a),
            )
            .expect("in-process admin-surface handler should succeed"),
        AdminSurfaceResponse::Started(AdminSurfaceStatusView {
            endpoints: vec![AdminSurfaceEndpointView {
                surface: "console-a".to_string(),
                local_addr: "status".to_string(),
            }],
        })
    );
    assert_eq!(
        handler
            .handle(
                AdminSurfaceRequest::Start {
                    instance_id: "console-b".to_string(),
                    surface_config_path: None,
                },
                Some(host_api_b),
            )
            .expect("in-process admin-surface handler should succeed"),
        AdminSurfaceResponse::Started(AdminSurfaceStatusView {
            endpoints: vec![AdminSurfaceEndpointView {
                surface: "console-b".to_string(),
                local_addr: "shutdown".to_string(),
            }],
        })
    );
}

#[test]
fn exported_admin_surface_plugins_reject_null_host_api() {
    let api = unsafe { &*test_admin_surface_plugin::mc_plugin_admin_surface_api_v1() };
    let request = AdminSurfaceRequest::Start {
        instance_id: "console".to_string(),
        surface_config_path: None,
    };
    let payload =
        encode_admin_surface_request(&request).expect("admin-surface request should encode");
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
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
        (api.free_buffer)(error);
    }
    assert_eq!(
        String::from_utf8(bytes).expect("plugin error should be utf-8"),
        "admin-surface host api was null"
    );
}

#[test]
fn exported_admin_surface_plugins_reject_mismatched_host_api_abi() {
    let context = TestHostContext {
        level_name: "host-a",
    };
    let mut host_api = admin_surface_host_api_for(&context);
    host_api.abi = mc_plugin_api::abi::PluginAbiVersion { major: 2, minor: 0 };
    let api = unsafe { &*test_admin_surface_plugin::mc_plugin_admin_surface_api_v1() };
    let request = AdminSurfaceRequest::Start {
        instance_id: "console".to_string(),
        surface_config_path: None,
    };
    let payload =
        encode_admin_surface_request(&request).expect("admin-surface request should encode");
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
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
        (api.free_buffer)(error);
    }
    assert_eq!(
        String::from_utf8(bytes).expect("plugin error should be utf-8"),
        format!(
            "admin-surface host api ABI 2.0 did not match plugin ABI {}",
            CURRENT_PLUGIN_ABI
        )
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

#[test]
fn sdk_root_reexports_cover_authoring_semantic_surface() {
    let adapter_id: crate::AdapterId = "je-47".into();
    assert_eq!(adapter_id.as_str(), "je-47");

    let capabilities = crate::SessionCapabilitySet {
        protocol: ProtocolCapabilitySet::new(),
        gameplay: crate::GameplayCapabilitySet::new(),
        gameplay_profile: GameplayProfileId::new("canonical"),
        entity_id: Some(crate::EntityId(7)),
        protocol_generation: None,
        gameplay_generation: None,
    };
    assert_eq!(capabilities.entity_id, Some(crate::EntityId(7)));
}

#[test]
fn doc_style_protocol_plugin_snippet_compiles_with_sdk_root_imports() {
    let entrypoints = doc_style_protocol_plugin::in_process_plugin_entrypoints();
    let capabilities = manifest_capability_names(entrypoints.manifest);
    assert_eq!(capabilities, vec!["runtime.reload.protocol".to_string(),]);
}

#[test]
fn doc_style_gameplay_plugin_snippet_compiles_with_sdk_root_imports() {
    let entrypoints = doc_style_gameplay_plugin::in_process_plugin_entrypoints();
    let capabilities = manifest_capability_names(entrypoints.manifest);
    assert_eq!(
        capabilities,
        vec![
            "gameplay.profile:doc-style".to_string(),
            "runtime.reload.gameplay".to_string(),
        ]
    );
}
