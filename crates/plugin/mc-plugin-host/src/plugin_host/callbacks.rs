use crate::config::PluginBufferLimits;
use crate::plugin_host::write_owned_buffer;
use mc_core::GameplayQuery;
use mc_plugin_api::abi::{ByteSlice, CURRENT_PLUGIN_ABI, OwnedBuffer, PluginErrorCode, Utf8Slice};
use mc_plugin_api::codec::gameplay::host_blob::{
    decode_block_pos, decode_can_edit_block_key, decode_player_id, encode_block_state,
    encode_player_snapshot, encode_world_meta,
};
use mc_plugin_api::host_api::HostApiTableV1;
use std::cell::Cell;

#[derive(Clone, Copy)]
struct GameplayQueryScope<'a> {
    query: &'a dyn GameplayQuery,
    buffer_limits: PluginBufferLimits,
}

thread_local! {
    static CURRENT_GAMEPLAY_QUERY: Cell<Option<*const ()>> = const { Cell::new(None) };
}

/// Runs plugin gameplay code with the current query temporarily published in thread-local state.
///
/// # Safety invariants
///
/// The stored pointer borrows `query`, so it is only valid for the dynamic extent of `f`.
/// Gameplay host callbacks must therefore remain synchronous, stay on the same thread, and never
/// retain the pointer beyond the callback.
#[cfg(test)]
pub(crate) fn with_gameplay_query<T>(
    query: &dyn GameplayQuery,
    f: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    with_gameplay_query_and_limits(query, PluginBufferLimits::default(), f)
}

/// Runs plugin gameplay code with the current query and buffer limits published in thread-local
/// state.
pub(crate) fn with_gameplay_query_and_limits<T>(
    query: &dyn GameplayQuery,
    buffer_limits: PluginBufferLimits,
    f: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    CURRENT_GAMEPLAY_QUERY.with(|slot| {
        let scope = GameplayQueryScope {
            query,
            buffer_limits,
        };
        let previous = slot.replace(Some(std::ptr::from_ref(&scope).cast()));
        let result = f();
        let _ = slot.replace(previous);
        result
    })
}

/// Resolves the gameplay query currently published by [`with_gameplay_query`].
///
/// # Safety invariants
///
/// This function may only be reached while `with_gameplay_query` is active on the current thread;
/// otherwise the stored pointer would be dangling or absent. Nested calls are safe because the
/// thread-local slot restores the previous pointer before returning.
#[cfg(test)]
pub(crate) fn with_current_gameplay_query<T>(
    f: impl FnOnce(&dyn GameplayQuery) -> Result<T, String>,
) -> Result<T, String> {
    with_current_gameplay_context(|scope| f(scope.query))
}

fn with_current_gameplay_context<T>(
    f: impl FnOnce(&GameplayQueryScope<'_>) -> Result<T, String>,
) -> Result<T, String> {
    CURRENT_GAMEPLAY_QUERY.with(|slot| {
        let query_scope_ptr = slot
            .get()
            .ok_or_else(|| "gameplay host callback invoked without an active query".to_string())?;
        let scope = unsafe { &*query_scope_ptr.cast::<GameplayQueryScope<'_>>() };
        f(scope)
    })
}

unsafe extern "C" fn gameplay_host_log(level: u32, message: Utf8Slice) {
    let metadata_limit =
        with_current_gameplay_context(|scope| Ok(scope.buffer_limits.metadata_bytes))
            .unwrap_or_else(|_| PluginBufferLimits::default().metadata_bytes);
    if let Ok(message) = super::decode_utf8_slice(message, metadata_limit) {
        eprintln!("gameplay[{level}]: {message}");
    }
}

unsafe extern "C" fn gameplay_host_read_player_snapshot(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let player_id = decode_player_id(payload).map_err(|error| error.to_string())?;
        let bytes = encode_player_snapshot(scope.query.player_snapshot(player_id).as_ref())
            .map_err(|error| error.to_string())?;
        write_owned_buffer(output, bytes);
        Ok(())
    });
    match result {
        Ok(()) => PluginErrorCode::Ok,
        Err(error) => {
            write_error_buffer(error_out, error);
            PluginErrorCode::Internal
        }
    }
}

unsafe extern "C" fn gameplay_host_read_world_meta(
    _context: *mut std::ffi::c_void,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let world_meta = scope.query.world_meta();
        let bytes = encode_world_meta(&world_meta).map_err(|error| error.to_string())?;
        write_owned_buffer(output, bytes);
        Ok(())
    });
    match result {
        Ok(()) => PluginErrorCode::Ok,
        Err(error) => {
            write_error_buffer(error_out, error);
            PluginErrorCode::Internal
        }
    }
}

unsafe extern "C" fn gameplay_host_read_block_state(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let position = decode_block_pos(payload).map_err(|error| error.to_string())?;
        let bytes = encode_block_state(&scope.query.block_state(position))
            .map_err(|error| error.to_string())?;
        write_owned_buffer(output, bytes);
        Ok(())
    });
    match result {
        Ok(()) => PluginErrorCode::Ok,
        Err(error) => {
            write_error_buffer(error_out, error);
            PluginErrorCode::Internal
        }
    }
}

unsafe extern "C" fn gameplay_host_can_edit_block(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    out: *mut bool,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let (player_id, position) =
            decode_can_edit_block_key(payload).map_err(|error| error.to_string())?;
        if !out.is_null() {
            unsafe {
                *out = scope.query.can_edit_block(player_id, position);
            }
        }
        Ok(())
    });
    match result {
        Ok(()) => PluginErrorCode::Ok,
        Err(error) => {
            write_error_buffer(error_out, error);
            PluginErrorCode::Internal
        }
    }
}

pub(crate) fn gameplay_host_api() -> HostApiTableV1 {
    HostApiTableV1 {
        abi: CURRENT_PLUGIN_ABI,
        context: std::ptr::null_mut(),
        log: Some(gameplay_host_log),
        read_player_snapshot: Some(gameplay_host_read_player_snapshot),
        read_world_meta: Some(gameplay_host_read_world_meta),
        read_block_state: Some(gameplay_host_read_block_state),
        can_edit_block: Some(gameplay_host_can_edit_block),
    }
}

pub(crate) fn admin_ui_host_api() -> HostApiTableV1 {
    HostApiTableV1 {
        abi: CURRENT_PLUGIN_ABI,
        context: std::ptr::null_mut(),
        log: None,
        read_player_snapshot: None,
        read_world_meta: None,
        read_block_state: None,
        can_edit_block: None,
    }
}

pub(crate) fn write_error_buffer(error_out: *mut OwnedBuffer, message: String) {
    write_owned_buffer(error_out, message.into_bytes());
}
