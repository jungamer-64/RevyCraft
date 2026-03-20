#![allow(clippy::multiple_crate_versions)]
use bytes::BytesMut;
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
    StorageRequest, StorageResponse, Utf8Slice, WireFrameDecodeResult,
};
use mc_proto_common::{HandshakeProbe, ProtocolAdapter, ProtocolError, StorageError};
use std::path::Path;

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
        Self::protocol_with_capabilities(plugin_id, display_name, &[])
    }

    #[must_use]
    pub const fn protocol_with_capabilities(
        plugin_id: &'static str,
        display_name: &'static str,
        capabilities: &'static [&'static str],
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Protocol,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            capabilities,
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
    /// Exports protocol plugin session state into an opaque transfer blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot serialize its protocol session state.
    fn export_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        Ok(Vec::new())
    }

    /// Imports protocol plugin session state from a previously exported blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the transfer blob is invalid for the current plugin.
    fn import_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), ProtocolError> {
        Ok(())
    }
}

pub trait RustStoragePlugin: Send + Sync + 'static {
    fn descriptor(&self) -> StorageDescriptor;

    fn capability_set(&self) -> CapabilitySet {
        CapabilitySet::new()
    }

    /// Loads the current world snapshot for the provided world directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the snapshot cannot be read or decoded.
    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError>;

    /// Persists the provided world snapshot for the provided world directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the snapshot cannot be written.
    fn save_snapshot(&self, world_dir: &Path, snapshot: &WorldSnapshot)
    -> Result<(), StorageError>;

    /// Exports runtime-specific state for later re-import.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime state cannot be read or serialized.
    fn export_runtime_state(
        &self,
        world_dir: &Path,
    ) -> Result<Option<WorldSnapshot>, StorageError> {
        self.load_snapshot(world_dir)
    }

    /// Imports runtime-specific state that was previously exported.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime state cannot be applied.
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

    /// Authenticates a Java Edition player without external services.
    ///
    /// # Errors
    ///
    /// Returns an error when the username cannot be authenticated.
    fn authenticate_offline(&self, username: &str) -> Result<PlayerId, String>;

    /// Authenticates a Java Edition player against an online service.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin does not support or rejects online auth.
    fn authenticate_online(&self, _username: &str, _server_hash: &str) -> Result<PlayerId, String> {
        Err("online auth is not implemented for this plugin".to_string())
    }

    /// Authenticates a Bedrock player without XBL validation.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin does not support or rejects Bedrock offline auth.
    fn authenticate_bedrock_offline(
        &self,
        _display_name: &str,
    ) -> Result<BedrockAuthResult, String> {
        Err("bedrock offline auth is not implemented for this plugin".to_string())
    }

    /// Authenticates a Bedrock player using the provided XBL token chain.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin does not support or rejects Bedrock XBL auth.
    fn authenticate_bedrock_xbl(
        &self,
        _chain_jwts: &[String],
        _client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, String> {
        Err("bedrock xbl auth is not implemented for this plugin".to_string())
    }
}

pub trait GameplayHost {
    /// Writes a diagnostic message through the host runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the host rejects or cannot persist the log entry.
    fn log(&self, level: u32, message: &str) -> Result<(), String>;

    /// Reads the latest snapshot for the given player.
    ///
    /// # Errors
    ///
    /// Returns an error when the host query fails.
    fn read_player_snapshot(&self, player_id: PlayerId) -> Result<Option<PlayerSnapshot>, String>;

    /// Reads world metadata from the host runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the host query fails.
    fn read_world_meta(&self) -> Result<WorldMeta, String>;

    /// Reads the current block state at the given position.
    ///
    /// # Errors
    ///
    /// Returns an error when the host query fails.
    fn read_block_state(&self, position: mc_core::BlockPos) -> Result<mc_core::BlockState, String>;

    /// Checks whether the given player is allowed to edit the given block.
    ///
    /// # Errors
    ///
    /// Returns an error when the host query fails.
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

    /// Handles a player joining the gameplay session.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot produce join-side effects.
    fn handle_player_join(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _player: &PlayerSnapshot,
    ) -> Result<GameplayJoinEffect, String> {
        Ok(GameplayJoinEffect::default())
    }

    /// Handles a gameplay command emitted by the runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot produce command-side effects.
    fn handle_command(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _command: &CoreCommand,
    ) -> Result<GameplayEffect, String> {
        Ok(GameplayEffect::default())
    }

    /// Handles a gameplay tick for the current session.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot produce tick-side effects.
    fn handle_tick(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _now_ms: u64,
    ) -> Result<GameplayEffect, String> {
        Ok(GameplayEffect::default())
    }

    /// Notifies the plugin that the gameplay session has been closed.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot clean up its session state.
    fn session_closed(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Exports plugin-specific gameplay session state into an opaque blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot serialize its session state.
    fn export_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
    ) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    /// Imports plugin-specific gameplay session state from an opaque blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the provided blob is invalid for the current plugin.
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
pub const fn into_owned_buffer(mut buffer: Vec<u8>) -> OwnedBuffer {
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
#[must_use]
pub const unsafe fn byte_slice_as_bytes(slice: ByteSlice) -> &'static [u8] {
    if slice.ptr.is_null() || slice.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) }
    }
}

fn write_owned_buffer_ptr(output: *mut OwnedBuffer, bytes: Vec<u8>) {
    unsafe {
        *output = into_owned_buffer(bytes);
    }
}

#[doc(hidden)]
pub fn write_error_buffer(error_out: *mut OwnedBuffer, message: String) {
    if error_out.is_null() {
        return;
    }
    write_owned_buffer_ptr(error_out, message.into_bytes());
}

#[doc(hidden)]
pub fn write_output_buffer(output: *mut OwnedBuffer, bytes: Vec<u8>) {
    if output.is_null() {
        return;
    }
    write_owned_buffer_ptr(output, bytes);
}

#[doc(hidden)]
pub fn handle_protocol_request<P: RustProtocolPlugin>(
    plugin: &P,
    request: ProtocolRequest,
) -> Result<ProtocolResponse, String> {
    match request {
        ProtocolRequest::Describe => Ok(ProtocolResponse::Descriptor(plugin.descriptor())),
        ProtocolRequest::DescribeBedrockListener => Ok(
            ProtocolResponse::BedrockListenerDescriptor(plugin.bedrock_listener_descriptor()),
        ),
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
        ProtocolRequest::EncodeWireFrame { payload } => plugin
            .wire_codec()
            .encode_frame(&payload)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::TryDecodeWireFrame { buffer } => {
            let mut buffer = BytesMut::from(buffer.as_slice());
            let original_len = buffer.len();
            plugin
                .wire_codec()
                .try_decode_frame(&mut buffer)
                .map(|frame| {
                    ProtocolResponse::WireFrameDecodeResult(frame.map(|frame| {
                        WireFrameDecodeResult {
                            frame,
                            bytes_consumed: original_len - buffer.len(),
                        }
                    }))
                })
                .map_err(|error| error.to_string())
        }
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

fn with_gameplay_host_api<T>(
    api: HostApiTableV1,
    f: impl FnOnce(&dyn GameplayHost) -> Result<T, String>,
) -> Result<T, String> {
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
            &raw mut output,
            &raw mut error,
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
    let status = unsafe { callback(context, &raw mut output, &raw mut error) };
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
            &raw mut value,
            &raw mut error,
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
    handle_gameplay_request_with_host_api(plugin, request, None)
}

#[doc(hidden)]
pub fn handle_gameplay_request_with_host_api<P: RustGameplayPlugin>(
    plugin: &P,
    request: GameplayRequest,
    host_api: Option<HostApiTableV1>,
) -> Result<GameplayResponse, String> {
    let require_host_api =
        || host_api.ok_or_else(|| "gameplay host api is not configured".to_string());
    match request {
        GameplayRequest::Describe => Ok(GameplayResponse::Descriptor(plugin.descriptor())),
        GameplayRequest::CapabilitySet => {
            Ok(GameplayResponse::CapabilitySet(plugin.capability_set()))
        }
        GameplayRequest::HandlePlayerJoin { session, player } => {
            with_gameplay_host_api(require_host_api()?, |host| {
                plugin.handle_player_join(host, &session, &player)
            })
            .map(GameplayResponse::JoinEffect)
        }
        GameplayRequest::HandleCommand { session, command } => {
            with_gameplay_host_api(require_host_api()?, |host| {
                plugin.handle_command(host, &session, &command)
            })
            .map(GameplayResponse::Effect)
        }
        GameplayRequest::HandleTick { session, now_ms } => {
            with_gameplay_host_api(require_host_api()?, |host| {
                plugin.handle_tick(host, &session, now_ms)
            })
            .map(GameplayResponse::Effect)
        }
        GameplayRequest::SessionClosed { session } => {
            with_gameplay_host_api(require_host_api()?, |host| {
                plugin.session_closed(host, &session)?;
                Ok(GameplayResponse::Empty)
            })
        }
        GameplayRequest::ExportSessionState { session } => {
            with_gameplay_host_api(require_host_api()?, |host| {
                plugin.export_session_state(host, &session)
            })
            .map(GameplayResponse::SessionTransferBlob)
        }
        GameplayRequest::ImportSessionState { session, blob } => {
            with_gameplay_host_api(require_host_api()?, |host| {
                plugin.import_session_state(host, &session, &blob)?;
                Ok(GameplayResponse::Empty)
            })
        }
    }
}

#[macro_export]
macro_rules! delegate_protocol_adapter {
    ($plugin_ty:ty, $field:ident, $capability_body:block $(,)?) => {
        impl $crate::RustProtocolPlugin for $plugin_ty {}

        impl mc_proto_common::HandshakeProbe for $plugin_ty {
            fn transport_kind(&self) -> mc_proto_common::TransportKind {
                self.$field.transport_kind()
            }

            fn adapter_id(&self) -> Option<&'static str> {
                self.$field.adapter_id()
            }

            fn try_route(
                &self,
                frame: &[u8],
            ) -> Result<Option<mc_proto_common::HandshakeIntent>, mc_proto_common::ProtocolError>
            {
                self.$field.try_route(frame)
            }
        }

        impl mc_proto_common::SessionAdapter for $plugin_ty {
            fn wire_codec(&self) -> &dyn mc_proto_common::WireCodec {
                self.$field.wire_codec()
            }

            fn decode_status(
                &self,
                frame: &[u8],
            ) -> Result<mc_proto_common::StatusRequest, mc_proto_common::ProtocolError> {
                self.$field.decode_status(frame)
            }

            fn decode_login(
                &self,
                frame: &[u8],
            ) -> Result<mc_proto_common::LoginRequest, mc_proto_common::ProtocolError> {
                self.$field.decode_login(frame)
            }

            fn encode_status_response(
                &self,
                status: &mc_proto_common::ServerListStatus,
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field.encode_status_response(status)
            }

            fn encode_status_pong(
                &self,
                payload: i64,
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field.encode_status_pong(payload)
            }

            fn encode_disconnect(
                &self,
                phase: mc_proto_common::ConnectionPhase,
                reason: &str,
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field.encode_disconnect(phase, reason)
            }

            fn encode_encryption_request(
                &self,
                server_id: &str,
                public_key_der: &[u8],
                verify_token: &[u8],
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field
                    .encode_encryption_request(server_id, public_key_der, verify_token)
            }

            fn encode_network_settings(
                &self,
                compression_threshold: u16,
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field.encode_network_settings(compression_threshold)
            }

            fn encode_login_success(
                &self,
                player: &mc_core::PlayerSnapshot,
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field.encode_login_success(player)
            }
        }

        impl mc_proto_common::PlaySyncAdapter for $plugin_ty {
            fn decode_play(
                &self,
                player_id: mc_core::PlayerId,
                frame: &[u8],
            ) -> Result<Option<mc_core::CoreCommand>, mc_proto_common::ProtocolError> {
                self.$field.decode_play(player_id, frame)
            }

            fn encode_play_event(
                &self,
                event: &mc_core::CoreEvent,
                context: &mc_proto_common::PlayEncodingContext,
            ) -> Result<Vec<Vec<u8>>, mc_proto_common::ProtocolError> {
                self.$field.encode_play_event(event, context)
            }
        }

        impl mc_proto_common::ProtocolAdapter for $plugin_ty {
            fn descriptor(&self) -> mc_proto_common::ProtocolDescriptor {
                self.$field.descriptor()
            }

            fn bedrock_listener_descriptor(
                &self,
            ) -> Option<mc_proto_common::BedrockListenerDescriptor> {
                self.$field.bedrock_listener_descriptor()
            }

            fn capability_set(&self) -> mc_core::CapabilitySet {
                $capability_body
            }
        }
    };
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

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::PluginManifestV1 {
            std::ptr::from_ref(
                MC_PLUGIN_MANIFEST.get_or_init(|| $crate::manifest_from_static(&$manifest)),
            )
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_protocol_api_v1() -> *const mc_plugin_api::ProtocolPluginApiV1 {
            std::ptr::from_ref(
                MC_PLUGIN_API.get_or_init(|| mc_plugin_api::ProtocolPluginApiV1 {
                    invoke: mc_plugin_invoke,
                    free_buffer: mc_plugin_free_buffer,
                }),
            )
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
        static MC_GAMEPLAY_HOST_API_SLOT: std::sync::OnceLock<
            std::sync::Mutex<Option<mc_plugin_api::HostApiTableV1>>,
        > = std::sync::OnceLock::new();

        fn mc_gameplay_plugin_instance() -> &'static $plugin_ty {
            MC_GAMEPLAY_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        fn mc_gameplay_host_api_slot()
        -> &'static std::sync::Mutex<Option<mc_plugin_api::HostApiTableV1>> {
            MC_GAMEPLAY_HOST_API_SLOT.get_or_init(|| std::sync::Mutex::new(None))
        }

        unsafe extern "C" fn mc_gameplay_plugin_set_host_api(
            host_api: *const mc_plugin_api::HostApiTableV1,
        ) -> mc_plugin_api::PluginErrorCode {
            let Some(host_api) = (unsafe { host_api.as_ref() }) else {
                return mc_plugin_api::PluginErrorCode::InvalidInput;
            };
            let mut guard = mc_gameplay_host_api_slot()
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
                let host_api = {
                    let guard = mc_gameplay_host_api_slot()
                        .lock()
                        .expect("gameplay host api mutex should not be poisoned");
                    *guard
                };
                $crate::handle_gameplay_request_with_host_api(
                    mc_gameplay_plugin_instance(),
                    request.clone(),
                    host_api,
                )
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

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::PluginManifestV1 {
            std::ptr::from_ref(
                MC_GAMEPLAY_PLUGIN_MANIFEST
                    .get_or_init(|| $crate::manifest_from_static(&$manifest)),
            )
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_gameplay_api_v1() -> *const mc_plugin_api::GameplayPluginApiV1 {
            std::ptr::from_ref(MC_GAMEPLAY_PLUGIN_API.get_or_init(|| {
                mc_plugin_api::GameplayPluginApiV1 {
                    set_host_api: mc_gameplay_plugin_set_host_api,
                    invoke: mc_gameplay_plugin_invoke,
                    free_buffer: mc_gameplay_plugin_free_buffer,
                }
            }))
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

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::PluginManifestV1 {
            std::ptr::from_ref(
                MC_STORAGE_PLUGIN_MANIFEST.get_or_init(|| $crate::manifest_from_static(&$manifest)),
            )
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_storage_api_v1() -> *const mc_plugin_api::StoragePluginApiV1 {
            std::ptr::from_ref(MC_STORAGE_PLUGIN_API.get_or_init(|| {
                mc_plugin_api::StoragePluginApiV1 {
                    invoke: mc_storage_plugin_invoke,
                    free_buffer: mc_storage_plugin_free_buffer,
                }
            }))
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

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::PluginManifestV1 {
            std::ptr::from_ref(
                MC_AUTH_PLUGIN_MANIFEST.get_or_init(|| $crate::manifest_from_static(&$manifest)),
            )
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_auth_api_v1() -> *const mc_plugin_api::AuthPluginApiV1 {
            std::ptr::from_ref(
                MC_AUTH_PLUGIN_API.get_or_init(|| mc_plugin_api::AuthPluginApiV1 {
                    invoke: mc_auth_plugin_invoke,
                    free_buffer: mc_auth_plugin_free_buffer,
                }),
            )
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

#[cfg(test)]
mod tests {
    use super::{
        CapabilitySet, GameplayHost, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
        HostApiTableV1, OwnedBuffer, RustGameplayPlugin, StaticPluginManifest,
        handle_gameplay_request_with_host_api, handle_protocol_request,
    };
    use bytes::BytesMut;
    use mc_core::{CoreCommand, CoreEvent, PlayerId, PlayerSnapshot};
    use mc_core::{GameplayEffect, GameplayProfileId, WorldMeta};
    use mc_plugin_api::{
        ByteSlice, CURRENT_PLUGIN_ABI, GameplayDescriptor, PluginErrorCode, ProtocolRequest,
        ProtocolResponse, WireFrameDecodeResult, decode_gameplay_response, encode_gameplay_request,
        encode_host_world_meta_blob,
    };
    use mc_proto_common::{
        ConnectionPhase, Edition, HandshakeIntent, HandshakeProbe, LoginRequest,
        PlayEncodingContext, ProtocolAdapter, ProtocolDescriptor, ProtocolError, ServerListStatus,
        SessionAdapter, StatusRequest, TransportKind, WireCodec, WireFormatKind,
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
        let bytes = encode_host_world_meta_blob(&WorldMeta {
            level_name: context.level_name.to_string(),
            seed: 0,
            spawn: mc_core::BlockPos::new(0, 64, 0),
            dimension: mc_core::DimensionId::Overworld,
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

    impl RustGameplayPlugin for DirectProbePlugin {
        fn descriptor(&self) -> GameplayDescriptor {
            GameplayDescriptor {
                profile: GameplayProfileId::new("probe"),
            }
        }

        fn handle_tick(
            &self,
            host: &dyn GameplayHost,
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

        fn try_decode_frame(
            &self,
            buffer: &mut BytesMut,
        ) -> Result<Option<Vec<u8>>, ProtocolError> {
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

        fn encode_status_response(
            &self,
            _status: &ServerListStatus,
        ) -> Result<Vec<u8>, ProtocolError> {
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

    #[test]
    fn direct_protocol_requests_route_wire_codec_ops_through_plugin_codec() {
        assert_eq!(
            handle_protocol_request(
                &DirectProtocolPlugin,
                ProtocolRequest::EncodeWireFrame {
                    payload: vec![0xaa, 0xbb, 0xcc],
                },
            )
            .expect("wire frame should encode"),
            ProtocolResponse::Frame(vec![3, 0xaa, 0xbb, 0xcc])
        );

        assert_eq!(
            handle_protocol_request(
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
            handle_protocol_request(
                &DirectProtocolPlugin,
                ProtocolRequest::TryDecodeWireFrame {
                    buffer: vec![3, 0xaa],
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
        let error = handle_gameplay_request_with_host_api(&DirectProbePlugin, request, None)
            .expect_err("host callbacks should require configured host api");
        assert!(error.contains("gameplay host api is not configured"));
    }

    #[allow(unexpected_cfgs)]
    mod plugin_a {
        use super::*;

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

        impl RustGameplayPlugin for PluginA {
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
                host: &dyn GameplayHost,
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

        const MANIFEST: StaticPluginManifest =
            StaticPluginManifest::gameplay("plugin-a", "Plugin A", &["runtime.reload.gameplay"]);

        export_gameplay_plugin!(PluginA, MANIFEST);
    }

    #[allow(unexpected_cfgs)]
    mod plugin_b {
        use super::*;

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

        impl RustGameplayPlugin for PluginB {
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
                host: &dyn GameplayHost,
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

        const MANIFEST: StaticPluginManifest =
            StaticPluginManifest::gameplay("plugin-b", "Plugin B", &["runtime.reload.gameplay"]);

        export_gameplay_plugin!(PluginB, MANIFEST);
    }

    unsafe fn invoke_gameplay(
        api: &mc_plugin_api::GameplayPluginApiV1,
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
}
