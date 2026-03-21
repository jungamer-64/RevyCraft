use super::{__macro_support, gameplay, manifest, protocol};
use bytes::BytesMut;
use mc_core::{
    BlockPos, CapabilitySet, CoreCommand, CoreEvent, DimensionId, GameplayEffect,
    GameplayProfileId, PlayerId, PlayerSnapshot, WorldMeta,
};
use mc_plugin_api::abi::{ByteSlice, CURRENT_PLUGIN_ABI, OwnedBuffer, PluginErrorCode};
use mc_plugin_api::codec::gameplay::{
    GameplayDescriptor, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
    decode_gameplay_response, encode_gameplay_request, host_blob::encode_world_meta,
};
use mc_plugin_api::codec::protocol::{ProtocolRequest, ProtocolResponse, WireFrameDecodeResult};
use mc_plugin_api::host_api::HostApiTableV1;
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

fn host_api_for(context: &TestHostContext) -> HostApiTableV1 {
    HostApiTableV1 {
        abi: CURRENT_PLUGIN_ABI,
        context: std::ptr::from_ref(context).cast_mut().cast(),
        log: None,
        read_player_snapshot: None,
        read_world_meta: Some(host_read_world_meta),
        read_block_state: None,
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
    ) -> Result<GameplayEffect, String> {
        let world_meta = host.read_world_meta()?;
        if world_meta.level_name.is_empty() {
            return Err("world meta should not be empty".to_string());
        }
        Ok(GameplayEffect::default())
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
        _player_id: PlayerId,
        _frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError> {
        Err(ProtocolError::InvalidPacket("unused test protocol method"))
    }

    fn encode_play_event(
        &self,
        _event: &CoreEvent,
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
        session: GameplaySessionSnapshot {
            phase: ConnectionPhase::Play,
            player_id: None,
            entity_id: None,
            gameplay_profile: GameplayProfileId::new("probe"),
        },
        now_ms: 0,
    };
    let error =
        __macro_support::handle_gameplay_request_with_host_api(&DirectProbePlugin, request, None)
            .expect_err("host callbacks should require configured host api");
    assert!(error.contains("gameplay host api is not configured"));
}

#[allow(unexpected_cfgs)]
mod plugin_a {
    use super::*;
    use crate::export_gameplay_plugin;

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
            GameplayDescriptor {
                profile: GameplayProfileId::new("plugin-a"),
            }
        }

        fn capability_set(&self) -> CapabilitySet {
            let mut capabilities = CapabilitySet::new();
            let _ = capabilities.insert("runtime.reload.gameplay");
            capabilities
        }

        fn handle_tick(
            &self,
            host: &dyn gameplay::GameplayHost,
            _session: &GameplaySessionSnapshot,
            _now_ms: u64,
        ) -> Result<GameplayEffect, String> {
            *recorded_slot()
                .lock()
                .expect("recorded level name mutex should not be poisoned") =
                Some(host.read_world_meta()?.level_name);
            Ok(GameplayEffect::default())
        }
    }

    const MANIFEST: manifest::StaticPluginManifest = manifest::StaticPluginManifest::gameplay(
        "plugin-a",
        "Plugin A",
        &["runtime.reload.gameplay"],
    );

    export_gameplay_plugin!(PluginA, MANIFEST);
}

#[allow(unexpected_cfgs)]
mod plugin_b {
    use super::*;
    use crate::export_gameplay_plugin;

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
            GameplayDescriptor {
                profile: GameplayProfileId::new("plugin-b"),
            }
        }

        fn capability_set(&self) -> CapabilitySet {
            let mut capabilities = CapabilitySet::new();
            let _ = capabilities.insert("runtime.reload.gameplay");
            capabilities
        }

        fn handle_tick(
            &self,
            host: &dyn gameplay::GameplayHost,
            _session: &GameplaySessionSnapshot,
            _now_ms: u64,
        ) -> Result<GameplayEffect, String> {
            *recorded_slot()
                .lock()
                .expect("recorded level name mutex should not be poisoned") =
                Some(host.read_world_meta()?.level_name);
            Ok(GameplayEffect::default())
        }
    }

    const MANIFEST: manifest::StaticPluginManifest = manifest::StaticPluginManifest::gameplay(
        "plugin-b",
        "Plugin B",
        &["runtime.reload.gameplay"],
    );

    export_gameplay_plugin!(PluginB, MANIFEST);
}

unsafe fn invoke_gameplay(
    api: &mc_plugin_api::host_api::GameplayPluginApiV1,
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

#[test]
fn exported_gameplay_plugins_keep_host_api_slots_isolated() {
    let context_a = TestHostContext {
        level_name: "host-a",
    };
    let context_b = TestHostContext {
        level_name: "host-b",
    };
    let host_api_a = host_api_for(&context_a);
    let host_api_b = host_api_for(&context_b);

    let entrypoints_a = plugin_a::in_process_gameplay_entrypoints();
    let entrypoints_b = plugin_b::in_process_gameplay_entrypoints();

    assert_eq!(
        unsafe { (entrypoints_a.api.set_host_api)(&raw const host_api_a) },
        PluginErrorCode::Ok
    );
    assert_eq!(
        unsafe { (entrypoints_b.api.set_host_api)(&raw const host_api_b) },
        PluginErrorCode::Ok
    );

    let request_a = GameplayRequest::HandleTick {
        session: GameplaySessionSnapshot {
            phase: ConnectionPhase::Play,
            player_id: None,
            entity_id: None,
            gameplay_profile: GameplayProfileId::new("plugin-a"),
        },
        now_ms: 1,
    };
    let request_b = GameplayRequest::HandleTick {
        session: GameplaySessionSnapshot {
            phase: ConnectionPhase::Play,
            player_id: None,
            entity_id: None,
            gameplay_profile: GameplayProfileId::new("plugin-b"),
        },
        now_ms: 2,
    };

    assert_eq!(
        unsafe { invoke_gameplay(entrypoints_a.api, &request_a) },
        GameplayResponse::Effect(GameplayEffect::default())
    );
    assert_eq!(
        unsafe { invoke_gameplay(entrypoints_b.api, &request_b) },
        GameplayResponse::Effect(GameplayEffect::default())
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
