use mc_core::{
    CapabilitySet, CoreCommand, GameplayEffect, GameplayJoinEffect, PlayerId, PlayerSnapshot,
    WorldMeta, WorldSnapshot,
};
use mc_plugin_api::{
    AuthDescriptor, AuthPluginApiV1, AuthRequest, AuthResponse, BedrockAuthResult, ByteSlice,
    CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, GameplayDescriptor, GameplayPluginApiV1,
    GameplayRequest, GameplayResponse, GameplaySessionSnapshot, HostApiTableV1, OwnedBuffer,
    PluginAbiVersion, PluginKind, PluginManifestV1, ProtocolPluginApiV1, ProtocolRequest,
    ProtocolResponse, ProtocolSessionSnapshot, StorageDescriptor, StoragePluginApiV1,
    StorageRequest, StorageResponse, Utf8Slice,
};
use mc_proto_common::{HandshakeProbe, ProtocolAdapter, ProtocolError, StorageError};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub struct StaticPluginManifest {
    pub plugin_id: &'static str,
    pub display_name: &'static str,
    pub plugin_kind: PluginKind,
    pub plugin_abi: PluginAbiVersion,
    pub min_host_abi: PluginAbiVersion,
    pub max_host_abi: PluginAbiVersion,
    pub capabilities: &'static [&'static str],
}

impl StaticPluginManifest {
    #[must_use]
    pub const fn protocol(plugin_id: &'static str, display_name: &'static str) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Protocol,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            capabilities: &[],
        }
    }

    #[must_use]
    pub const fn gameplay(
        plugin_id: &'static str,
        display_name: &'static str,
        capabilities: &'static [&'static str],
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Gameplay,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            capabilities,
        }
    }

    #[must_use]
    pub const fn storage(
        plugin_id: &'static str,
        display_name: &'static str,
        capabilities: &'static [&'static str],
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Storage,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            capabilities,
        }
    }

    #[must_use]
    pub const fn auth(
        plugin_id: &'static str,
        display_name: &'static str,
        capabilities: &'static [&'static str],
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Auth,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            capabilities,
        }
    }
}

#[derive(Clone, Copy)]
pub struct InProcessProtocolEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub api: &'static ProtocolPluginApiV1,
}

#[derive(Clone, Copy)]
pub struct InProcessGameplayEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub api: &'static GameplayPluginApiV1,
}

#[derive(Clone, Copy)]
pub struct InProcessStorageEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub api: &'static StoragePluginApiV1,
}

#[derive(Clone, Copy)]
pub struct InProcessAuthEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub api: &'static AuthPluginApiV1,
}

pub trait RustProtocolPlugin: HandshakeProbe + ProtocolAdapter + Send + Sync + 'static {
    fn export_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        Ok(Vec::new())
    }

    fn import_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), ProtocolError> {
        Ok(())
    }
}

impl<T> RustProtocolPlugin for T where T: HandshakeProbe + ProtocolAdapter + Send + Sync + 'static {}

pub trait RustStoragePlugin: Send + Sync + 'static {
    fn descriptor(&self) -> StorageDescriptor;

    fn capability_set(&self) -> CapabilitySet {
        CapabilitySet::new()
    }

    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError>;

    fn save_snapshot(&self, world_dir: &Path, snapshot: &WorldSnapshot)
    -> Result<(), StorageError>;

    fn export_runtime_state(
        &self,
        world_dir: &Path,
    ) -> Result<Option<WorldSnapshot>, StorageError> {
        self.load_snapshot(world_dir)
    }

    fn import_runtime_state(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        self.save_snapshot(world_dir, snapshot)
    }
}

pub trait RustAuthPlugin: Send + Sync + 'static {
    fn descriptor(&self) -> AuthDescriptor;

    fn capability_set(&self) -> CapabilitySet {
        CapabilitySet::new()
    }

    fn authenticate_offline(&self, username: &str) -> Result<PlayerId, String>;

    fn authenticate_online(&self, _username: &str, _server_hash: &str) -> Result<PlayerId, String> {
        Err("online auth is not implemented for this plugin".to_string())
    }

    fn authenticate_bedrock_offline(
        &self,
        _display_name: &str,
    ) -> Result<BedrockAuthResult, String> {
        Err("bedrock offline auth is not implemented for this plugin".to_string())
    }

    fn authenticate_bedrock_xbl(
        &self,
        _chain_jwts: &[String],
        _client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, String> {
        Err("bedrock xbl auth is not implemented for this plugin".to_string())
    }
}

pub trait GameplayHost {
    fn log(&self, level: u32, message: &str) -> Result<(), String>;
    fn read_player_snapshot(&self, player_id: PlayerId) -> Result<Option<PlayerSnapshot>, String>;
    fn read_world_meta(&self) -> Result<WorldMeta, String>;
    fn read_block_state(&self, position: mc_core::BlockPos) -> Result<mc_core::BlockState, String>;
    fn can_edit_block(
        &self,
        player_id: PlayerId,
        position: mc_core::BlockPos,
    ) -> Result<bool, String>;
}

pub trait RustGameplayPlugin: Send + Sync + 'static {
    fn descriptor(&self) -> GameplayDescriptor;

    fn capability_set(&self) -> CapabilitySet {
        CapabilitySet::new()
    }

    fn handle_player_join(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _player: &PlayerSnapshot,
    ) -> Result<GameplayJoinEffect, String> {
        Ok(GameplayJoinEffect::default())
    }

    fn handle_command(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _command: &CoreCommand,
    ) -> Result<GameplayEffect, String> {
        Ok(GameplayEffect::default())
    }

    fn handle_tick(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _now_ms: u64,
    ) -> Result<GameplayEffect, String> {
        Ok(GameplayEffect::default())
    }

    fn session_closed(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
    ) -> Result<(), String> {
        Ok(())
    }

    fn export_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
    ) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    fn import_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), String> {
        Ok(())
    }
}

#[must_use]
pub fn manifest_from_static(manifest: &StaticPluginManifest) -> PluginManifestV1 {
    let (capabilities, capabilities_len) = if manifest.capabilities.is_empty() {
        (std::ptr::null(), 0)
    } else {
        let descriptors = manifest
            .capabilities
            .iter()
            .map(|capability| CapabilityDescriptorV1 {
                name: Utf8Slice::from_static_str(capability),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let leaked = Box::leak(descriptors);
        (leaked.as_ptr(), leaked.len())
    };
    PluginManifestV1 {
        plugin_id: Utf8Slice::from_static_str(manifest.plugin_id),
        display_name: Utf8Slice::from_static_str(manifest.display_name),
        plugin_kind: manifest.plugin_kind,
        plugin_abi: manifest.plugin_abi,
        min_host_abi: manifest.min_host_abi,
        max_host_abi: manifest.max_host_abi,
        capabilities,
        capabilities_len,
    }
}

#[must_use]
pub fn into_owned_buffer(mut buffer: Vec<u8>) -> OwnedBuffer {
    let owned = OwnedBuffer {
        ptr: buffer.as_mut_ptr(),
        len: buffer.len(),
        cap: buffer.capacity(),
    };
    std::mem::forget(buffer);
    owned
}

/// # Safety
///
/// `buffer` must have been allocated by [`into_owned_buffer`].
pub unsafe fn free_owned_buffer(buffer: OwnedBuffer) {
    if !buffer.ptr.is_null() {
        unsafe {
            let _ = Vec::from_raw_parts(buffer.ptr, buffer.len, buffer.cap);
        }
    }
}

#[doc(hidden)]
pub unsafe fn byte_slice_as_bytes(slice: ByteSlice) -> &'static [u8] {
    if slice.ptr.is_null() || slice.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) }
    }
}

#[doc(hidden)]
pub fn write_error_buffer(error_out: *mut OwnedBuffer, message: String) {
    if error_out.is_null() {
        return;
    }
    unsafe {
        *error_out = into_owned_buffer(message.into_bytes());
    }
}

#[doc(hidden)]
pub fn write_output_buffer(output: *mut OwnedBuffer, bytes: Vec<u8>) {
    if output.is_null() {
        return;
    }
    unsafe {
        *output = into_owned_buffer(bytes);
    }
}

#[doc(hidden)]
pub fn handle_protocol_request<P: RustProtocolPlugin>(
    plugin: &P,
    request: ProtocolRequest,
) -> Result<ProtocolResponse, String> {
    match request {
        ProtocolRequest::Describe => Ok(ProtocolResponse::Descriptor(plugin.descriptor())),
        ProtocolRequest::CapabilitySet => {
            Ok(ProtocolResponse::CapabilitySet(plugin.capability_set()))
        }
        ProtocolRequest::TryRoute { frame } => plugin
            .try_route(&frame)
            .map(ProtocolResponse::HandshakeIntent)
            .map_err(|error| error.to_string()),
        ProtocolRequest::DecodeStatus { frame } => plugin
            .decode_status(&frame)
            .map(ProtocolResponse::StatusRequest)
            .map_err(|error| error.to_string()),
        ProtocolRequest::DecodeLogin { frame } => plugin
            .decode_login(&frame)
            .map(ProtocolResponse::LoginRequest)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeStatusResponse { status } => plugin
            .encode_status_response(&status)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeStatusPong { payload } => plugin
            .encode_status_pong(payload)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeDisconnect { phase, reason } => plugin
            .encode_disconnect(phase, &reason)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeEncryptionRequest {
            server_id,
            public_key_der,
            verify_token,
        } => plugin
            .encode_encryption_request(&server_id, &public_key_der, &verify_token)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeNetworkSettings {
            compression_threshold,
        } => plugin
            .encode_network_settings(compression_threshold)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeLoginSuccess { player } => plugin
            .encode_login_success(&player)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::DecodePlay { player_id, frame } => plugin
            .decode_play(player_id, &frame)
            .map(ProtocolResponse::CoreCommand)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodePlayEvent { event, context } => plugin
            .encode_play_event(&event, &context)
            .map(ProtocolResponse::Frames)
            .map_err(|error| error.to_string()),
        ProtocolRequest::ExportSessionState { session } => plugin
            .export_session_state(&session)
            .map(ProtocolResponse::SessionTransferBlob)
            .map_err(|error| error.to_string()),
        ProtocolRequest::ImportSessionState { session, blob } => plugin
            .import_session_state(&session, &blob)
            .map(|()| ProtocolResponse::Empty)
            .map_err(|error| error.to_string()),
    }
}

#[doc(hidden)]
pub fn handle_storage_request<P: RustStoragePlugin>(
    plugin: &P,
    request: StorageRequest,
) -> Result<StorageResponse, String> {
    match request {
        StorageRequest::Describe => Ok(StorageResponse::Descriptor(plugin.descriptor())),
        StorageRequest::CapabilitySet => {
            Ok(StorageResponse::CapabilitySet(plugin.capability_set()))
        }
        StorageRequest::LoadSnapshot { world_dir } => plugin
            .load_snapshot(Path::new(&world_dir))
            .map(StorageResponse::Snapshot)
            .map_err(|error| error.to_string()),
        StorageRequest::SaveSnapshot {
            world_dir,
            snapshot,
        } => plugin
            .save_snapshot(Path::new(&world_dir), &snapshot)
            .map(|()| StorageResponse::Empty)
            .map_err(|error| error.to_string()),
        StorageRequest::ExportRuntimeState { world_dir } => plugin
            .export_runtime_state(Path::new(&world_dir))
            .map(StorageResponse::Snapshot)
            .map_err(|error| error.to_string()),
        StorageRequest::ImportRuntimeState {
            world_dir,
            snapshot,
        } => plugin
            .import_runtime_state(Path::new(&world_dir), &snapshot)
            .map(|()| StorageResponse::Empty)
            .map_err(|error| error.to_string()),
    }
}

#[doc(hidden)]
pub fn handle_auth_request<P: RustAuthPlugin>(
    plugin: &P,
    request: AuthRequest,
) -> Result<AuthResponse, String> {
    match request {
        AuthRequest::Describe => Ok(AuthResponse::Descriptor(plugin.descriptor())),
        AuthRequest::CapabilitySet => Ok(AuthResponse::CapabilitySet(plugin.capability_set())),
        AuthRequest::AuthenticateOffline { username } => plugin
            .authenticate_offline(&username)
            .map(AuthResponse::AuthenticatedPlayer),
        AuthRequest::AuthenticateOnline {
            username,
            server_hash,
        } => plugin
            .authenticate_online(&username, &server_hash)
            .map(AuthResponse::AuthenticatedPlayer),
        AuthRequest::AuthenticateBedrockOffline { display_name } => plugin
            .authenticate_bedrock_offline(&display_name)
            .map(AuthResponse::AuthenticatedBedrockPlayer),
        AuthRequest::AuthenticateBedrockXbl {
            chain_jwts,
            client_data_jwt,
        } => plugin
            .authenticate_bedrock_xbl(&chain_jwts, &client_data_jwt)
            .map(AuthResponse::AuthenticatedBedrockPlayer),
    }
}

struct SdkGameplayHost {
    api: HostApiTableV1,
}

impl GameplayHost for SdkGameplayHost {
    fn log(&self, level: u32, message: &str) -> Result<(), String> {
        let Some(log) = self.api.log else {
            return Ok(());
        };
        unsafe {
            log(
                level,
                Utf8Slice {
                    ptr: message.as_ptr(),
                    len: message.len(),
                },
            );
        }
        Ok(())
    }

    fn read_player_snapshot(&self, player_id: PlayerId) -> Result<Option<PlayerSnapshot>, String> {
        let Some(callback) = self.api.read_player_snapshot else {
            return Err("gameplay host did not provide read_player_snapshot".to_string());
        };
        let payload = mc_plugin_api::encode_host_player_id_blob(player_id);
        let bytes = call_host_buffer(self.api.context, &payload, callback)?;
        mc_plugin_api::decode_host_player_snapshot_blob(&bytes).map_err(|error| error.to_string())
    }

    fn read_world_meta(&self) -> Result<WorldMeta, String> {
        let Some(callback) = self.api.read_world_meta else {
            return Err("gameplay host did not provide read_world_meta".to_string());
        };
        let bytes = call_host_zero_arg(self.api.context, callback)?;
        mc_plugin_api::decode_host_world_meta_blob(&bytes).map_err(|error| error.to_string())
    }

    fn read_block_state(&self, position: mc_core::BlockPos) -> Result<mc_core::BlockState, String> {
        let Some(callback) = self.api.read_block_state else {
            return Err("gameplay host did not provide read_block_state".to_string());
        };
        let payload = mc_plugin_api::encode_host_block_pos_blob(position);
        let bytes = call_host_buffer(self.api.context, &payload, callback)?;
        mc_plugin_api::decode_host_block_state_blob(&bytes).map_err(|error| error.to_string())
    }

    fn can_edit_block(
        &self,
        player_id: PlayerId,
        position: mc_core::BlockPos,
    ) -> Result<bool, String> {
        let Some(callback) = self.api.can_edit_block else {
            return Err("gameplay host did not provide can_edit_block".to_string());
        };
        let payload = mc_plugin_api::encode_host_can_edit_block_key(player_id, position);
        call_host_bool(self.api.context, &payload, callback)
    }
}

#[doc(hidden)]
pub fn gameplay_host_api_slot() -> &'static Mutex<Option<HostApiTableV1>> {
    static HOST_API: OnceLock<Mutex<Option<HostApiTableV1>>> = OnceLock::new();
    HOST_API.get_or_init(|| Mutex::new(None))
}

fn with_gameplay_host<T>(
    f: impl FnOnce(&dyn GameplayHost) -> Result<T, String>,
) -> Result<T, String> {
    let api = {
        let guard = gameplay_host_api_slot()
            .lock()
            .expect("gameplay host api mutex should not be poisoned");
        guard.ok_or_else(|| "gameplay host api is not configured".to_string())?
    };
    let host = SdkGameplayHost { api };
    f(&host)
}

fn call_host_buffer(
    context: *mut std::ffi::c_void,
    payload: &[u8],
    callback: unsafe extern "C" fn(
        *mut std::ffi::c_void,
        ByteSlice,
        *mut OwnedBuffer,
        *mut OwnedBuffer,
    ) -> mc_plugin_api::PluginErrorCode,
) -> Result<Vec<u8>, String> {
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        callback(
            context,
            ByteSlice {
                ptr: payload.as_ptr(),
                len: payload.len(),
            },
            &mut output,
            &mut error,
        )
    };
    if status != mc_plugin_api::PluginErrorCode::Ok {
        return Err(read_error_buffer(error));
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        free_owned_buffer(output);
    }
    Ok(bytes)
}

fn call_host_zero_arg(
    context: *mut std::ffi::c_void,
    callback: unsafe extern "C" fn(
        *mut std::ffi::c_void,
        *mut OwnedBuffer,
        *mut OwnedBuffer,
    ) -> mc_plugin_api::PluginErrorCode,
) -> Result<Vec<u8>, String> {
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe { callback(context, &mut output, &mut error) };
    if status != mc_plugin_api::PluginErrorCode::Ok {
        return Err(read_error_buffer(error));
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        free_owned_buffer(output);
    }
    Ok(bytes)
}

fn call_host_bool(
    context: *mut std::ffi::c_void,
    payload: &[u8],
    callback: unsafe extern "C" fn(
        *mut std::ffi::c_void,
        ByteSlice,
        *mut bool,
        *mut OwnedBuffer,
    ) -> mc_plugin_api::PluginErrorCode,
) -> Result<bool, String> {
    let mut value = false;
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        callback(
            context,
            ByteSlice {
                ptr: payload.as_ptr(),
                len: payload.len(),
            },
            &mut value,
            &mut error,
        )
    };
    if status != mc_plugin_api::PluginErrorCode::Ok {
        return Err(read_error_buffer(error));
    }
    Ok(value)
}

fn read_error_buffer(buffer: OwnedBuffer) -> String {
    if buffer.ptr.is_null() {
        return "host callback failed".to_string();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len) }.to_vec();
    unsafe {
        free_owned_buffer(buffer);
    }
    String::from_utf8(bytes).unwrap_or_else(|_| "host callback returned invalid utf-8".to_string())
}

#[doc(hidden)]
pub fn handle_gameplay_request<P: RustGameplayPlugin>(
    plugin: &P,
    request: GameplayRequest,
) -> Result<GameplayResponse, String> {
    match request {
        GameplayRequest::Describe => Ok(GameplayResponse::Descriptor(plugin.descriptor())),
        GameplayRequest::CapabilitySet => {
            Ok(GameplayResponse::CapabilitySet(plugin.capability_set()))
        }
        GameplayRequest::HandlePlayerJoin { session, player } => {
            with_gameplay_host(|host| plugin.handle_player_join(host, &session, &player))
                .map(GameplayResponse::JoinEffect)
        }
        GameplayRequest::HandleCommand { session, command } => {
            with_gameplay_host(|host| plugin.handle_command(host, &session, &command))
                .map(GameplayResponse::Effect)
        }
        GameplayRequest::HandleTick { session, now_ms } => {
            with_gameplay_host(|host| plugin.handle_tick(host, &session, now_ms))
                .map(GameplayResponse::Effect)
        }
        GameplayRequest::SessionClosed { session } => with_gameplay_host(|host| {
            plugin.session_closed(host, &session)?;
            Ok(GameplayResponse::Empty)
        }),
        GameplayRequest::ExportSessionState { session } => {
            with_gameplay_host(|host| plugin.export_session_state(host, &session))
                .map(GameplayResponse::SessionTransferBlob)
        }
        GameplayRequest::ImportSessionState { session, blob } => with_gameplay_host(|host| {
            plugin.import_session_state(host, &session, &blob)?;
            Ok(GameplayResponse::Empty)
        }),
    }
}

#[macro_export]
macro_rules! export_protocol_plugin {
    ($plugin_ty:ty, $manifest:expr) => {
        static MC_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> = std::sync::OnceLock::new();
        static MC_PLUGIN_MANIFEST: std::sync::OnceLock<mc_plugin_api::PluginManifestV1> =
            std::sync::OnceLock::new();
        static MC_PLUGIN_API: std::sync::OnceLock<mc_plugin_api::ProtocolPluginApiV1> =
            std::sync::OnceLock::new();

        fn mc_plugin_instance() -> &'static $plugin_ty {
            MC_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        unsafe extern "C" fn mc_plugin_invoke(
            request: mc_plugin_api::ByteSlice,
            output: *mut mc_plugin_api::OwnedBuffer,
            error_out: *mut mc_plugin_api::OwnedBuffer,
        ) -> mc_plugin_api::PluginErrorCode {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes = unsafe { $crate::byte_slice_as_bytes(request) };
                mc_plugin_api::decode_protocol_request(request_bytes)
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::write_error_buffer(error_out, error.to_string());
                    return mc_plugin_api::PluginErrorCode::InvalidInput;
                }
                Err(_) => {
                    $crate::write_error_buffer(
                        error_out,
                        "protocol plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::handle_protocol_request(mc_plugin_instance(), request.clone())
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::write_error_buffer(error_out, message);
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::write_error_buffer(
                        error_out,
                        "protocol plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::encode_protocol_response(&request, &response) {
                Ok(bytes) => {
                    $crate::write_output_buffer(output, bytes);
                    mc_plugin_api::PluginErrorCode::Ok
                }
                Err(message) => {
                    $crate::write_error_buffer(error_out, message.to_string());
                    mc_plugin_api::PluginErrorCode::Internal
                }
            }
        }

        unsafe extern "C" fn mc_plugin_free_buffer(buffer: mc_plugin_api::OwnedBuffer) {
            unsafe {
                $crate::free_owned_buffer(buffer);
            }
        }

        #[cfg_attr(not(feature = "disable-exported-symbols"), unsafe(no_mangle))]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::PluginManifestV1 {
            MC_PLUGIN_MANIFEST.get_or_init(|| $crate::manifest_from_static(&$manifest))
                as *const mc_plugin_api::PluginManifestV1
        }

        #[cfg_attr(not(feature = "disable-exported-symbols"), unsafe(no_mangle))]
        pub extern "C" fn mc_plugin_protocol_api_v1() -> *const mc_plugin_api::ProtocolPluginApiV1 {
            MC_PLUGIN_API.get_or_init(|| mc_plugin_api::ProtocolPluginApiV1 {
                invoke: mc_plugin_invoke,
                free_buffer: mc_plugin_free_buffer,
            }) as *const mc_plugin_api::ProtocolPluginApiV1
        }

        #[must_use]
        pub fn in_process_protocol_entrypoints() -> $crate::InProcessProtocolEntrypoints {
            $crate::InProcessProtocolEntrypoints {
                manifest: unsafe { &*mc_plugin_manifest_v1() },
                api: unsafe { &*mc_plugin_protocol_api_v1() },
            }
        }
    };
}

#[macro_export]
macro_rules! export_gameplay_plugin {
    ($plugin_ty:ty, $manifest:expr) => {
        static MC_GAMEPLAY_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> =
            std::sync::OnceLock::new();
        static MC_GAMEPLAY_PLUGIN_MANIFEST: std::sync::OnceLock<mc_plugin_api::PluginManifestV1> =
            std::sync::OnceLock::new();
        static MC_GAMEPLAY_PLUGIN_API: std::sync::OnceLock<mc_plugin_api::GameplayPluginApiV1> =
            std::sync::OnceLock::new();

        fn mc_gameplay_plugin_instance() -> &'static $plugin_ty {
            MC_GAMEPLAY_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        unsafe extern "C" fn mc_gameplay_plugin_set_host_api(
            host_api: *const mc_plugin_api::HostApiTableV1,
        ) -> mc_plugin_api::PluginErrorCode {
            let Some(host_api) = (unsafe { host_api.as_ref() }) else {
                return mc_plugin_api::PluginErrorCode::InvalidInput;
            };
            let mut guard = $crate::gameplay_host_api_slot()
                .lock()
                .expect("gameplay host api mutex should not be poisoned");
            *guard = Some(*host_api);
            mc_plugin_api::PluginErrorCode::Ok
        }

        unsafe extern "C" fn mc_gameplay_plugin_invoke(
            request: mc_plugin_api::ByteSlice,
            output: *mut mc_plugin_api::OwnedBuffer,
            error_out: *mut mc_plugin_api::OwnedBuffer,
        ) -> mc_plugin_api::PluginErrorCode {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes = unsafe { $crate::byte_slice_as_bytes(request) };
                mc_plugin_api::decode_gameplay_request(request_bytes)
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::write_error_buffer(error_out, error.to_string());
                    return mc_plugin_api::PluginErrorCode::InvalidInput;
                }
                Err(_) => {
                    $crate::write_error_buffer(
                        error_out,
                        "gameplay plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::handle_gameplay_request(mc_gameplay_plugin_instance(), request.clone())
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::write_error_buffer(error_out, message);
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::write_error_buffer(
                        error_out,
                        "gameplay plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::encode_gameplay_response(&request, &response) {
                Ok(bytes) => {
                    $crate::write_output_buffer(output, bytes);
                    mc_plugin_api::PluginErrorCode::Ok
                }
                Err(message) => {
                    $crate::write_error_buffer(error_out, message.to_string());
                    mc_plugin_api::PluginErrorCode::Internal
                }
            }
        }

        unsafe extern "C" fn mc_gameplay_plugin_free_buffer(buffer: mc_plugin_api::OwnedBuffer) {
            unsafe {
                $crate::free_owned_buffer(buffer);
            }
        }

        #[cfg_attr(not(feature = "disable-exported-symbols"), unsafe(no_mangle))]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::PluginManifestV1 {
            MC_GAMEPLAY_PLUGIN_MANIFEST.get_or_init(|| $crate::manifest_from_static(&$manifest))
                as *const mc_plugin_api::PluginManifestV1
        }

        #[cfg_attr(not(feature = "disable-exported-symbols"), unsafe(no_mangle))]
        pub extern "C" fn mc_plugin_gameplay_api_v1() -> *const mc_plugin_api::GameplayPluginApiV1 {
            MC_GAMEPLAY_PLUGIN_API.get_or_init(|| mc_plugin_api::GameplayPluginApiV1 {
                set_host_api: mc_gameplay_plugin_set_host_api,
                invoke: mc_gameplay_plugin_invoke,
                free_buffer: mc_gameplay_plugin_free_buffer,
            }) as *const mc_plugin_api::GameplayPluginApiV1
        }

        #[must_use]
        pub fn in_process_gameplay_entrypoints() -> $crate::InProcessGameplayEntrypoints {
            $crate::InProcessGameplayEntrypoints {
                manifest: unsafe { &*mc_plugin_manifest_v1() },
                api: unsafe { &*mc_plugin_gameplay_api_v1() },
            }
        }
    };
}

#[macro_export]
macro_rules! export_storage_plugin {
    ($plugin_ty:ty, $manifest:expr) => {
        static MC_STORAGE_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> =
            std::sync::OnceLock::new();
        static MC_STORAGE_PLUGIN_MANIFEST: std::sync::OnceLock<mc_plugin_api::PluginManifestV1> =
            std::sync::OnceLock::new();
        static MC_STORAGE_PLUGIN_API: std::sync::OnceLock<mc_plugin_api::StoragePluginApiV1> =
            std::sync::OnceLock::new();

        fn mc_storage_plugin_instance() -> &'static $plugin_ty {
            MC_STORAGE_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        unsafe extern "C" fn mc_storage_plugin_invoke(
            request: mc_plugin_api::ByteSlice,
            output: *mut mc_plugin_api::OwnedBuffer,
            error_out: *mut mc_plugin_api::OwnedBuffer,
        ) -> mc_plugin_api::PluginErrorCode {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes = unsafe { $crate::byte_slice_as_bytes(request) };
                mc_plugin_api::decode_storage_request(request_bytes)
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::write_error_buffer(error_out, error.to_string());
                    return mc_plugin_api::PluginErrorCode::InvalidInput;
                }
                Err(_) => {
                    $crate::write_error_buffer(
                        error_out,
                        "storage plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::handle_storage_request(mc_storage_plugin_instance(), request.clone())
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::write_error_buffer(error_out, message);
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::write_error_buffer(
                        error_out,
                        "storage plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::encode_storage_response(&request, &response) {
                Ok(bytes) => {
                    $crate::write_output_buffer(output, bytes);
                    mc_plugin_api::PluginErrorCode::Ok
                }
                Err(message) => {
                    $crate::write_error_buffer(error_out, message.to_string());
                    mc_plugin_api::PluginErrorCode::Internal
                }
            }
        }

        unsafe extern "C" fn mc_storage_plugin_free_buffer(buffer: mc_plugin_api::OwnedBuffer) {
            unsafe {
                $crate::free_owned_buffer(buffer);
            }
        }

        #[cfg_attr(not(feature = "disable-exported-symbols"), unsafe(no_mangle))]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::PluginManifestV1 {
            MC_STORAGE_PLUGIN_MANIFEST.get_or_init(|| $crate::manifest_from_static(&$manifest))
                as *const mc_plugin_api::PluginManifestV1
        }

        #[cfg_attr(not(feature = "disable-exported-symbols"), unsafe(no_mangle))]
        pub extern "C" fn mc_plugin_storage_api_v1() -> *const mc_plugin_api::StoragePluginApiV1 {
            MC_STORAGE_PLUGIN_API.get_or_init(|| mc_plugin_api::StoragePluginApiV1 {
                invoke: mc_storage_plugin_invoke,
                free_buffer: mc_storage_plugin_free_buffer,
            }) as *const mc_plugin_api::StoragePluginApiV1
        }

        #[must_use]
        pub fn in_process_storage_entrypoints() -> $crate::InProcessStorageEntrypoints {
            $crate::InProcessStorageEntrypoints {
                manifest: unsafe { &*mc_plugin_manifest_v1() },
                api: unsafe { &*mc_plugin_storage_api_v1() },
            }
        }
    };
}

#[macro_export]
macro_rules! export_auth_plugin {
    ($plugin_ty:ty, $manifest:expr) => {
        static MC_AUTH_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> =
            std::sync::OnceLock::new();
        static MC_AUTH_PLUGIN_MANIFEST: std::sync::OnceLock<mc_plugin_api::PluginManifestV1> =
            std::sync::OnceLock::new();
        static MC_AUTH_PLUGIN_API: std::sync::OnceLock<mc_plugin_api::AuthPluginApiV1> =
            std::sync::OnceLock::new();

        fn mc_auth_plugin_instance() -> &'static $plugin_ty {
            MC_AUTH_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        unsafe extern "C" fn mc_auth_plugin_invoke(
            request: mc_plugin_api::ByteSlice,
            output: *mut mc_plugin_api::OwnedBuffer,
            error_out: *mut mc_plugin_api::OwnedBuffer,
        ) -> mc_plugin_api::PluginErrorCode {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes = unsafe { $crate::byte_slice_as_bytes(request) };
                mc_plugin_api::decode_auth_request(request_bytes)
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::write_error_buffer(error_out, error.to_string());
                    return mc_plugin_api::PluginErrorCode::InvalidInput;
                }
                Err(_) => {
                    $crate::write_error_buffer(
                        error_out,
                        "auth plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::handle_auth_request(mc_auth_plugin_instance(), request.clone())
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::write_error_buffer(error_out, message);
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::write_error_buffer(
                        error_out,
                        "auth plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::encode_auth_response(&request, &response) {
                Ok(bytes) => {
                    $crate::write_output_buffer(output, bytes);
                    mc_plugin_api::PluginErrorCode::Ok
                }
                Err(message) => {
                    $crate::write_error_buffer(error_out, message.to_string());
                    mc_plugin_api::PluginErrorCode::Internal
                }
            }
        }

        unsafe extern "C" fn mc_auth_plugin_free_buffer(buffer: mc_plugin_api::OwnedBuffer) {
            unsafe {
                $crate::free_owned_buffer(buffer);
            }
        }

        #[cfg_attr(not(feature = "disable-exported-symbols"), unsafe(no_mangle))]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::PluginManifestV1 {
            MC_AUTH_PLUGIN_MANIFEST.get_or_init(|| $crate::manifest_from_static(&$manifest))
                as *const mc_plugin_api::PluginManifestV1
        }

        #[cfg_attr(not(feature = "disable-exported-symbols"), unsafe(no_mangle))]
        pub extern "C" fn mc_plugin_auth_api_v1() -> *const mc_plugin_api::AuthPluginApiV1 {
            MC_AUTH_PLUGIN_API.get_or_init(|| mc_plugin_api::AuthPluginApiV1 {
                invoke: mc_auth_plugin_invoke,
                free_buffer: mc_auth_plugin_free_buffer,
            }) as *const mc_plugin_api::AuthPluginApiV1
        }

        #[must_use]
        pub fn in_process_auth_entrypoints() -> $crate::InProcessAuthEntrypoints {
            $crate::InProcessAuthEntrypoints {
                manifest: unsafe { &*mc_plugin_manifest_v1() },
                api: unsafe { &*mc_plugin_auth_api_v1() },
            }
        }
    };
}
