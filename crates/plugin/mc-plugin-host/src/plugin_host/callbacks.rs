use crate::config::PluginBufferLimits;
use crate::plugin_host::write_owned_buffer;
use mc_core::GameplayTransaction;
use mc_plugin_api::abi::{ByteSlice, CURRENT_PLUGIN_ABI, OwnedBuffer, PluginErrorCode, Utf8Slice};
use mc_plugin_api::codec::gameplay::host_blob::{
    decode_begin_mining, decode_block_pos, decode_can_edit_block_key, decode_clear_mining,
    decode_inventory_slot_update, decode_open_chest, decode_open_crafting_table,
    decode_open_furnace, decode_player_id, decode_player_pose_update,
    decode_selected_hotbar_slot_update, decode_set_block, decode_spawn_dropped_item,
    decode_targeted_event_blob, encode_block_entity, encode_block_state, encode_player_snapshot,
    encode_world_meta,
};
use mc_plugin_api::host_api::{GameplayHostApiV2, HostApiTableV1};
use std::cell::Cell;

struct GameplayTxScope<'scope, 'core> {
    tx: &'scope mut GameplayTransaction<'core>,
    buffer_limits: PluginBufferLimits,
}

thread_local! {
    static CURRENT_GAMEPLAY_TX: Cell<Option<*mut ()>> = const { Cell::new(None) };
}

pub(crate) fn with_gameplay_transaction_and_limits<T>(
    tx: &mut GameplayTransaction<'_>,
    buffer_limits: PluginBufferLimits,
    f: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    CURRENT_GAMEPLAY_TX.with(|slot| {
        let mut scope = GameplayTxScope { tx, buffer_limits };
        let previous = slot.replace(Some(std::ptr::from_mut(&mut scope).cast()));
        let result = f();
        let _ = slot.replace(previous);
        result
    })
}

#[cfg(test)]
pub(crate) fn with_current_gameplay_transaction<T>(
    f: impl FnOnce(&mut GameplayTransaction<'_>) -> Result<T, String>,
) -> Result<T, String> {
    with_current_gameplay_context(|scope| f(scope.tx))
}

fn with_current_gameplay_context<T>(
    f: impl FnOnce(&mut GameplayTxScope<'_, '_>) -> Result<T, String>,
) -> Result<T, String> {
    CURRENT_GAMEPLAY_TX.with(|slot| {
        let tx_scope_ptr = slot.get().ok_or_else(|| {
            "gameplay host callback invoked without an active transaction".to_string()
        })?;
        let scope = unsafe { &mut *tx_scope_ptr.cast::<GameplayTxScope<'_, '_>>() };
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
        let bytes = encode_player_snapshot(scope.tx.player_snapshot(player_id).as_ref())
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
        let world_meta = scope.tx.world_meta();
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
        let bytes = encode_block_state(&scope.tx.block_state(position))
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

unsafe extern "C" fn gameplay_host_read_block_entity(
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
        let bytes = encode_block_entity(scope.tx.block_entity(position).as_ref())
            .map_err(|e| e.to_string())?;
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
                *out = scope.tx.can_edit_block(player_id, position);
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

unsafe extern "C" fn gameplay_host_set_player_pose(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let (player_id, position, yaw, pitch, on_ground) =
            decode_player_pose_update(payload).map_err(|error| error.to_string())?;
        scope
            .tx
            .set_player_pose(player_id, position, yaw, pitch, on_ground);
        Ok(())
    });
    mutation_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_set_selected_hotbar_slot(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let (player_id, slot) =
            decode_selected_hotbar_slot_update(payload).map_err(|error| error.to_string())?;
        scope.tx.set_selected_hotbar_slot(player_id, slot);
        Ok(())
    });
    mutation_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_set_inventory_slot(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let (player_id, slot, stack) =
            decode_inventory_slot_update(payload).map_err(|error| error.to_string())?;
        scope.tx.set_inventory_slot(player_id, slot, stack);
        Ok(())
    });
    mutation_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_clear_mining(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let player_id = decode_clear_mining(payload).map_err(|error| error.to_string())?;
        scope.tx.clear_mining(player_id);
        Ok(())
    });
    mutation_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_begin_mining(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let (player_id, position, duration_ms) =
            decode_begin_mining(payload).map_err(|error| error.to_string())?;
        scope.tx.begin_mining(player_id, position, duration_ms);
        Ok(())
    });
    mutation_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_open_chest(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let (player_id, position) =
            decode_open_chest(payload).map_err(|error| error.to_string())?;
        scope.tx.open_chest(player_id, position);
        Ok(())
    });
    mutation_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_open_furnace(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let (player_id, position) =
            decode_open_furnace(payload).map_err(|error| error.to_string())?;
        scope.tx.open_furnace(player_id, position);
        Ok(())
    });
    mutation_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_open_crafting_table(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let player_id = decode_open_crafting_table(payload).map_err(|error| error.to_string())?;
        scope.tx.open_crafting_table(player_id);
        Ok(())
    });
    mutation_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_set_block(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let (position, block) = decode_set_block(payload).map_err(|error| error.to_string())?;
        scope.tx.set_block(position, block);
        Ok(())
    });
    mutation_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_spawn_dropped_item(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let (position, item) =
            decode_spawn_dropped_item(payload).map_err(|error| error.to_string())?;
        scope.tx.spawn_dropped_item(position, item);
        Ok(())
    });
    mutation_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_emit_event(
    _context: *mut std::ffi::c_void,
    payload: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let payload = super::read_byte_slice(
            payload,
            scope.buffer_limits.callback_payload_bytes,
            "gameplay host callback payload",
        )?;
        let event = decode_targeted_event_blob(payload).map_err(|error| error.to_string())?;
        scope.tx.emit_event(event.target, event.event);
        Ok(())
    });
    mutation_status(result, error_out)
}

fn mutation_status(result: Result<(), String>, error_out: *mut OwnedBuffer) -> PluginErrorCode {
    match result {
        Ok(()) => PluginErrorCode::Ok,
        Err(error) => {
            write_error_buffer(error_out, error);
            PluginErrorCode::Internal
        }
    }
}

pub(crate) fn gameplay_host_api() -> GameplayHostApiV2 {
    GameplayHostApiV2 {
        abi: CURRENT_PLUGIN_ABI,
        context: std::ptr::null_mut(),
        log: Some(gameplay_host_log),
        read_player_snapshot: Some(gameplay_host_read_player_snapshot),
        read_world_meta: Some(gameplay_host_read_world_meta),
        read_block_state: Some(gameplay_host_read_block_state),
        read_block_entity: Some(gameplay_host_read_block_entity),
        can_edit_block: Some(gameplay_host_can_edit_block),
        set_player_pose: Some(gameplay_host_set_player_pose),
        set_selected_hotbar_slot: Some(gameplay_host_set_selected_hotbar_slot),
        set_inventory_slot: Some(gameplay_host_set_inventory_slot),
        clear_mining: Some(gameplay_host_clear_mining),
        begin_mining: Some(gameplay_host_begin_mining),
        open_chest: Some(gameplay_host_open_chest),
        open_furnace: Some(gameplay_host_open_furnace),
        set_block: Some(gameplay_host_set_block),
        spawn_dropped_item: Some(gameplay_host_spawn_dropped_item),
        emit_event: Some(gameplay_host_emit_event),
        open_crafting_table: Some(gameplay_host_open_crafting_table),
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
        read_block_entity: None,
        can_edit_block: None,
    }
}

pub(crate) fn write_error_buffer(error_out: *mut OwnedBuffer, message: String) {
    write_owned_buffer(error_out, message.into_bytes());
}
