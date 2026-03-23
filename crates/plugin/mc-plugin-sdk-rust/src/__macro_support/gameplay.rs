use crate::gameplay::{GameplayHost, RustGameplayPlugin};
use mc_core::{PlayerId, PlayerSnapshot, WorldMeta};
use mc_plugin_api::abi::{ByteSlice, OwnedBuffer, PluginErrorCode, Utf8Slice};
use mc_plugin_api::codec::gameplay::host_blob::{
    decode_block_state, decode_player_snapshot, decode_world_meta, encode_block_pos,
    encode_can_edit_block_key, encode_player_id,
};
use mc_plugin_api::codec::gameplay::{GameplayRequest, GameplayResponse};
use mc_plugin_api::host_api::HostApiTableV1;

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
        let payload = encode_player_id(player_id);
        let bytes = call_host_buffer(self.api.context, &payload, callback)?;
        decode_player_snapshot(&bytes).map_err(|error| error.to_string())
    }

    fn read_world_meta(&self) -> Result<WorldMeta, String> {
        let Some(callback) = self.api.read_world_meta else {
            return Err("gameplay host did not provide read_world_meta".to_string());
        };
        let bytes = call_host_zero_arg(self.api.context, callback)?;
        decode_world_meta(&bytes).map_err(|error| error.to_string())
    }

    fn read_block_state(&self, position: mc_core::BlockPos) -> Result<mc_core::BlockState, String> {
        let Some(callback) = self.api.read_block_state else {
            return Err("gameplay host did not provide read_block_state".to_string());
        };
        let payload = encode_block_pos(position);
        let bytes = call_host_buffer(self.api.context, &payload, callback)?;
        decode_block_state(&bytes).map_err(|error| error.to_string())
    }

    fn can_edit_block(
        &self,
        player_id: PlayerId,
        position: mc_core::BlockPos,
    ) -> Result<bool, String> {
        let Some(callback) = self.api.can_edit_block else {
            return Err("gameplay host did not provide can_edit_block".to_string());
        };
        let payload = encode_can_edit_block_key(player_id, position);
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
        crate::__macro_support::buffers::free_owned_buffer(output);
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
        crate::__macro_support::buffers::free_owned_buffer(output);
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
        crate::__macro_support::buffers::free_owned_buffer(buffer);
    }
    String::from_utf8(bytes).unwrap_or_else(|_| "host callback returned invalid utf-8".to_string())
}

pub fn handle_gameplay_request<P: RustGameplayPlugin>(
    plugin: &P,
    request: GameplayRequest,
) -> Result<GameplayResponse, String> {
    handle_gameplay_request_with_host_api(plugin, request, None)
}

pub fn handle_gameplay_request_with_host_api<P: RustGameplayPlugin>(
    plugin: &P,
    request: GameplayRequest,
    host_api: Option<HostApiTableV1>,
) -> Result<GameplayResponse, String> {
    let require_host_api =
        || host_api.ok_or_else(|| "gameplay host api is not configured".to_string());
    match request {
        GameplayRequest::Describe => Ok(GameplayResponse::Descriptor(plugin.descriptor())),
        GameplayRequest::CapabilitySet => Ok(GameplayResponse::CapabilitySet(
            crate::capabilities::gameplay_announcement(&plugin.capability_set()),
        )),
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
