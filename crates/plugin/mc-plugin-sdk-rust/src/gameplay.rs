use super::*;

pub use crate::export_gameplay_plugin;

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
        let payload = encode_host_player_id_blob(player_id);
        let bytes = call_host_buffer(self.api.context, &payload, callback)?;
        decode_host_player_snapshot_blob(&bytes).map_err(|error| error.to_string())
    }

    fn read_world_meta(&self) -> Result<WorldMeta, String> {
        let Some(callback) = self.api.read_world_meta else {
            return Err("gameplay host did not provide read_world_meta".to_string());
        };
        let bytes = call_host_zero_arg(self.api.context, callback)?;
        decode_host_world_meta_blob(&bytes).map_err(|error| error.to_string())
    }

    fn read_block_state(&self, position: mc_core::BlockPos) -> Result<mc_core::BlockState, String> {
        let Some(callback) = self.api.read_block_state else {
            return Err("gameplay host did not provide read_block_state".to_string());
        };
        let payload = encode_host_block_pos_blob(position);
        let bytes = call_host_buffer(self.api.context, &payload, callback)?;
        decode_host_block_state_blob(&bytes).map_err(|error| error.to_string())
    }

    fn can_edit_block(
        &self,
        player_id: PlayerId,
        position: mc_core::BlockPos,
    ) -> Result<bool, String> {
        let Some(callback) = self.api.can_edit_block else {
            return Err("gameplay host did not provide can_edit_block".to_string());
        };
        let payload = encode_host_can_edit_block_key(player_id, position);
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
    ) -> PluginErrorCode,
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
    if status != PluginErrorCode::Ok {
        return Err(read_error_buffer(error));
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        crate::buffers::free_owned_buffer(output);
    }
    Ok(bytes)
}

fn call_host_zero_arg(
    context: *mut std::ffi::c_void,
    callback: unsafe extern "C" fn(
        *mut std::ffi::c_void,
        *mut OwnedBuffer,
        *mut OwnedBuffer,
    ) -> PluginErrorCode,
) -> Result<Vec<u8>, String> {
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe { callback(context, &raw mut output, &raw mut error) };
    if status != PluginErrorCode::Ok {
        return Err(read_error_buffer(error));
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        crate::buffers::free_owned_buffer(output);
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
    ) -> PluginErrorCode,
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
    if status != PluginErrorCode::Ok {
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
        crate::buffers::free_owned_buffer(buffer);
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
