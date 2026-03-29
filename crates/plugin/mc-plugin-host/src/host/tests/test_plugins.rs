pub(super) mod entity_id_probe_gameplay_plugin {
    use mc_plugin_api::codec::gameplay::{GameplayDescriptor, GameplaySessionSnapshot};
    use mc_plugin_sdk_rust::export_plugin;
    use mc_plugin_sdk_rust::gameplay::{GameplayHost, RustGameplayPlugin};
    use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
    use revy_voxel_core::{
        GameplayCapability, GameplayCapabilitySet, GameplayCommand, GameplayProfileId, PlayerId,
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

        fn capability_set(&self) -> GameplayCapabilitySet {
            let mut capabilities = GameplayCapabilitySet::new();
            let _ = capabilities.insert(GameplayCapability::RuntimeReload);
            capabilities
        }

        fn handle_command(
            &self,
            _host: &dyn GameplayHost,
            session: &GameplaySessionSnapshot,
            _command: &GameplayCommand,
        ) -> Result<(), String> {
            *recorded_session_slot()
                .lock()
                .expect("recorded gameplay session mutex should not be poisoned") =
                Some(session.clone());
            Ok(())
        }

        fn handle_player_join(
            &self,
            _host: &dyn GameplayHost,
            _session: &GameplaySessionSnapshot,
            _player: PlayerId,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    const MANIFEST: StaticPluginManifest = StaticPluginManifest::gameplay(
        "gameplay-entity-aware",
        "Entity Aware Gameplay Plugin",
        "entity-aware",
    );

    export_plugin!(gameplay, EntityIdProbeGameplayPlugin, MANIFEST);
}

pub(super) mod counting_gameplay_plugin {
    use mc_plugin_api::codec::gameplay::{GameplayDescriptor, GameplaySessionSnapshot};
    use mc_plugin_sdk_rust::export_plugin;
    use mc_plugin_sdk_rust::gameplay::{GameplayHost, RustGameplayPlugin};
    use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
    use revy_voxel_core::{
        GameplayCapability, GameplayCapabilitySet, GameplayCommand, GameplayProfileId, PlayerId,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static COMMAND_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Default)]
    pub struct CountingGameplayPlugin;

    pub fn reset_invocations() {
        COMMAND_INVOCATIONS.store(0, Ordering::SeqCst);
    }

    pub fn command_invocations() -> usize {
        COMMAND_INVOCATIONS.load(Ordering::SeqCst)
    }

    fn test_lock_slot() -> &'static Mutex<()> {
        static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_LOCK.get_or_init(|| Mutex::new(()))
    }

    pub fn lock() -> MutexGuard<'static, ()> {
        test_lock_slot()
            .lock()
            .expect("counting gameplay plugin test lock should not be poisoned")
    }

    impl RustGameplayPlugin for CountingGameplayPlugin {
        fn descriptor(&self) -> GameplayDescriptor {
            GameplayDescriptor {
                profile: GameplayProfileId::new("counting"),
            }
        }

        fn capability_set(&self) -> GameplayCapabilitySet {
            let mut capabilities = GameplayCapabilitySet::new();
            let _ = capabilities.insert(GameplayCapability::RuntimeReload);
            capabilities
        }

        fn handle_command(
            &self,
            host: &dyn GameplayHost,
            _session: &GameplaySessionSnapshot,
            command: &GameplayCommand,
        ) -> Result<(), String> {
            COMMAND_INVOCATIONS.fetch_add(1, Ordering::SeqCst);
            match command {
                GameplayCommand::SetHeldSlot { player_id, slot } => {
                    host.read_player_snapshot(*player_id)?.ok_or_else(|| {
                        "counting gameplay plugin expected a live player".to_string()
                    })?;
                    host.set_selected_hotbar_slot(
                        *player_id,
                        u8::try_from(*slot).map_err(|_| {
                            "counting gameplay plugin expected a non-negative held slot".to_string()
                        })?,
                    )?;
                    Ok(())
                }
                other => Err(format!(
                    "counting gameplay plugin only supports SetHeldSlot, got {other:?}"
                )),
            }
        }

        fn handle_player_join(
            &self,
            _host: &dyn GameplayHost,
            _session: &GameplaySessionSnapshot,
            _player: PlayerId,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    const MANIFEST: StaticPluginManifest =
        StaticPluginManifest::gameplay("gameplay-counting", "Counting Gameplay Plugin", "counting");

    export_plugin!(gameplay, CountingGameplayPlugin, MANIFEST);
}

pub(super) mod custom_wire_codec_protocol_plugin {
    use mc_plugin_api::abi::{
        ByteSlice, CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, OwnedBuffer, PluginErrorCode,
        PluginKind, Utf8Slice,
    };
    use mc_plugin_api::codec::protocol::{
        ProtocolRequest, ProtocolResponse, WireFrameDecodeResult, decode_protocol_request,
        encode_protocol_response,
    };
    use mc_plugin_api::host_api::ProtocolPluginApiV3;
    use mc_plugin_api::manifest::PluginManifestV1;
    use mc_plugin_sdk_rust::test_support::InProcessPluginEntrypoints;
    use mc_proto_common::{Edition, ProtocolDescriptor, TransportKind, WireFormatKind};
    use revy_voxel_core::{CapabilityAnnouncement, ProtocolCapability, ProtocolCapabilitySet};
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
                let mut capabilities = ProtocolCapabilitySet::new();
                let _ = capabilities.insert(ProtocolCapability::RuntimeReload);
                Ok(ProtocolResponse::CapabilitySet(
                    CapabilityAnnouncement::new(capabilities),
                ))
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

    pub fn in_process_plugin_entrypoints() -> InProcessPluginEntrypoints<ProtocolPluginApiV3> {
        static MANIFEST: OnceLock<PluginManifestV1> = OnceLock::new();
        static CAPABILITIES: OnceLock<&'static [CapabilityDescriptorV1]> = OnceLock::new();
        static API: OnceLock<ProtocolPluginApiV3> = OnceLock::new();
        InProcessPluginEntrypoints::new(
            MANIFEST.get_or_init(|| PluginManifestV1 {
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
            API.get_or_init(|| ProtocolPluginApiV3 {
                invoke,
                free_buffer,
            }),
        )
    }
}

pub(super) mod failing_protocol_plugin {
    use mc_plugin_api::abi::{
        ByteSlice, CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, OwnedBuffer, PluginErrorCode,
        PluginKind, Utf8Slice,
    };
    use mc_plugin_api::codec::protocol::{
        ProtocolRequest, ProtocolResponse, decode_protocol_request, encode_protocol_response,
    };
    use mc_plugin_api::host_api::ProtocolPluginApiV3;
    use mc_plugin_api::manifest::PluginManifestV1;
    use mc_plugin_sdk_rust::test_support::InProcessPluginEntrypoints;
    use mc_proto_common::{Edition, ProtocolDescriptor, TransportKind, WireFormatKind};
    use revy_voxel_core::{CapabilityAnnouncement, ProtocolCapability, ProtocolCapabilitySet};
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
                let mut capabilities = ProtocolCapabilitySet::new();
                let _ = capabilities.insert(ProtocolCapability::RuntimeReload);
                Ok(ProtocolResponse::CapabilitySet(
                    CapabilityAnnouncement::new(capabilities),
                ))
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

    pub fn in_process_plugin_entrypoints() -> InProcessPluginEntrypoints<ProtocolPluginApiV3> {
        static MANIFEST: OnceLock<PluginManifestV1> = OnceLock::new();
        static CAPABILITIES: OnceLock<&'static [CapabilityDescriptorV1]> = OnceLock::new();
        static API: OnceLock<ProtocolPluginApiV3> = OnceLock::new();
        InProcessPluginEntrypoints::new(
            MANIFEST.get_or_init(|| PluginManifestV1 {
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
            API.get_or_init(|| ProtocolPluginApiV3 {
                invoke,
                free_buffer,
            }),
        )
    }
}

pub(super) mod failing_gameplay_plugin {
    use mc_plugin_api::codec::gameplay::{GameplayDescriptor, GameplaySessionSnapshot};
    use mc_plugin_sdk_rust::export_plugin;
    use mc_plugin_sdk_rust::gameplay::{GameplayHost, RustGameplayPlugin};
    use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
    use revy_voxel_core::{
        GameplayCapability, GameplayCapabilitySet, GameplayCommand, GameplayProfileId, PlayerId,
    };

    #[derive(Default)]
    pub struct FailingGameplayPlugin;

    impl RustGameplayPlugin for FailingGameplayPlugin {
        fn descriptor(&self) -> GameplayDescriptor {
            GameplayDescriptor {
                profile: GameplayProfileId::new("failing"),
            }
        }

        fn capability_set(&self) -> GameplayCapabilitySet {
            let mut capabilities = GameplayCapabilitySet::new();
            let _ = capabilities.insert(GameplayCapability::RuntimeReload);
            capabilities
        }

        fn handle_command(
            &self,
            _host: &dyn GameplayHost,
            _session: &GameplaySessionSnapshot,
            _command: &GameplayCommand,
        ) -> Result<(), String> {
            Err("gameplay runtime failure".to_string())
        }

        fn handle_player_join(
            &self,
            _host: &dyn GameplayHost,
            _session: &GameplaySessionSnapshot,
            _player: PlayerId,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    const MANIFEST: StaticPluginManifest =
        StaticPluginManifest::gameplay("gameplay-failing", "Failing Gameplay Plugin", "failing");

    export_plugin!(gameplay, FailingGameplayPlugin, MANIFEST);
}

pub(super) mod failing_auth_plugin {
    use mc_plugin_api::codec::auth::{AuthDescriptor, AuthMode};
    use mc_plugin_sdk_rust::auth::RustAuthPlugin;
    use mc_plugin_sdk_rust::export_plugin;
    use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
    use revy_voxel_core::{AuthCapability, AuthCapabilitySet, PlayerId};

    pub const PROFILE_ID: &str = "failing-auth";

    #[derive(Default)]
    pub struct FailingAuthPlugin;

    impl RustAuthPlugin for FailingAuthPlugin {
        fn descriptor(&self) -> AuthDescriptor {
            AuthDescriptor {
                auth_profile: PROFILE_ID.into(),
                mode: AuthMode::Offline,
            }
        }

        fn capability_set(&self) -> AuthCapabilitySet {
            let mut capabilities = AuthCapabilitySet::new();
            let _ = capabilities.insert(AuthCapability::RuntimeReload);
            capabilities
        }

        fn authenticate_offline(&self, _username: &str) -> Result<PlayerId, String> {
            Err("auth runtime failure".to_string())
        }
    }

    const MANIFEST: StaticPluginManifest =
        StaticPluginManifest::auth("auth-failing", "Failing Auth Plugin", PROFILE_ID);

    export_plugin!(auth, FailingAuthPlugin, MANIFEST);
}

pub(super) mod route_collision_protocol_plugin {
    use mc_plugin_api::abi::{
        ByteSlice, CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, OwnedBuffer, PluginErrorCode,
        PluginKind, Utf8Slice,
    };
    use mc_plugin_api::codec::protocol::{
        ProtocolRequest, ProtocolResponse, decode_protocol_request, encode_protocol_response,
    };
    use mc_plugin_api::host_api::ProtocolPluginApiV3;
    use mc_plugin_api::manifest::PluginManifestV1;
    use mc_plugin_sdk_rust::test_support::InProcessPluginEntrypoints;
    use mc_proto_common::{Edition, ProtocolDescriptor, TransportKind, WireFormatKind};
    use revy_voxel_core::{CapabilityAnnouncement, ProtocolCapability, ProtocolCapabilitySet};
    use std::sync::OnceLock;

    const PLUGIN_ID: &str = "je-5-collision";

    fn descriptor() -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: PLUGIN_ID.to_string(),
            transport: TransportKind::Tcp,
            wire_format: WireFormatKind::MinecraftFramed,
            edition: Edition::Je,
            version_name: "je-5-collision".to_string(),
            protocol_number: 5,
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
                let mut capabilities = ProtocolCapabilitySet::new();
                let _ = capabilities.insert(ProtocolCapability::RuntimeReload);
                Ok(ProtocolResponse::CapabilitySet(
                    CapabilityAnnouncement::new(capabilities),
                ))
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

    pub fn in_process_plugin_entrypoints() -> InProcessPluginEntrypoints<ProtocolPluginApiV3> {
        static MANIFEST: OnceLock<PluginManifestV1> = OnceLock::new();
        static CAPABILITIES: OnceLock<&'static [CapabilityDescriptorV1]> = OnceLock::new();
        static API: OnceLock<ProtocolPluginApiV3> = OnceLock::new();
        InProcessPluginEntrypoints::new(
            MANIFEST.get_or_init(|| PluginManifestV1 {
                plugin_id: Utf8Slice::from_static_str(PLUGIN_ID),
                display_name: Utf8Slice::from_static_str("Route Collision Protocol Plugin"),
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
            API.get_or_init(|| ProtocolPluginApiV3 {
                invoke,
                free_buffer,
            }),
        )
    }
}

pub(super) mod oversized_protocol_response_plugin {
    use mc_plugin_api::abi::{
        ByteSlice, CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, OwnedBuffer, PluginErrorCode,
        PluginKind, Utf8Slice,
    };
    use mc_plugin_api::codec::protocol::{
        ProtocolRequest, ProtocolResponse, decode_protocol_request, encode_protocol_response,
    };
    use mc_plugin_api::host_api::ProtocolPluginApiV3;
    use mc_plugin_api::manifest::PluginManifestV1;
    use mc_plugin_sdk_rust::test_support::InProcessPluginEntrypoints;
    use mc_proto_common::{Edition, ProtocolDescriptor, TransportKind, WireFormatKind};
    use revy_voxel_core::{CapabilityAnnouncement, ProtocolCapability, ProtocolCapabilitySet};
    use std::sync::OnceLock;

    const PLUGIN_ID: &str = "protocol-oversized-response";

    fn descriptor() -> ProtocolDescriptor {
        ProtocolDescriptor {
            adapter_id: PLUGIN_ID.to_string(),
            transport: TransportKind::Tcp,
            wire_format: WireFormatKind::MinecraftFramed,
            edition: Edition::Je,
            version_name: "x".repeat(512),
            protocol_number: 9123,
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
                let mut capabilities = ProtocolCapabilitySet::new();
                let _ = capabilities.insert(ProtocolCapability::RuntimeReload);
                Ok(ProtocolResponse::CapabilitySet(
                    CapabilityAnnouncement::new(capabilities),
                ))
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

    pub fn in_process_plugin_entrypoints() -> InProcessPluginEntrypoints<ProtocolPluginApiV3> {
        static MANIFEST: OnceLock<PluginManifestV1> = OnceLock::new();
        static CAPABILITIES: OnceLock<&'static [CapabilityDescriptorV1]> = OnceLock::new();
        static API: OnceLock<ProtocolPluginApiV3> = OnceLock::new();
        InProcessPluginEntrypoints::new(
            MANIFEST.get_or_init(|| PluginManifestV1 {
                plugin_id: Utf8Slice::from_static_str(PLUGIN_ID),
                display_name: Utf8Slice::from_static_str("Oversized Protocol Response Plugin"),
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
            API.get_or_init(|| ProtocolPluginApiV3 {
                invoke,
                free_buffer,
            }),
        )
    }
}
