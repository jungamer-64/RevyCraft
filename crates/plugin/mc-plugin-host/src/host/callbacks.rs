use crate::config::PluginBufferLimits;
use mc_plugin_api::abi::{ByteSlice, CURRENT_PLUGIN_ABI, OwnedBuffer, PluginErrorCode, Utf8Slice};
use mc_plugin_api::codec::gameplay::host_blob::{
    decode_block_pos, decode_can_edit_block_key, decode_gameplay_effect_blob, decode_player_id,
    encode_block_entity, encode_block_state, encode_player_snapshot, encode_world_meta,
};
use mc_plugin_api::host_api::{AdminSurfaceHostApiV1, GameplayHostApiV3};
use revy_server_gameplay_bridge::{
    BlockEntityState, BlockPos, BlockState, GameplayCanEditBlockKey, GameplayEffect,
    GameplayEffectBatch, GameplayReadSet, GameplayReadView, PlayerId, PlayerSnapshot, WorldMeta,
};
use std::cell::Cell;

use super::write_owned_buffer;

pub(crate) struct GameplayInvocationScope {
    read_view: Box<dyn GameplayReadView>,
    reads: GameplayReadSet,
    effects: Vec<GameplayEffect>,
    buffer_limits: PluginBufferLimits,
}

impl GameplayInvocationScope {
    #[must_use]
    pub(crate) fn new(
        read_view: Box<dyn GameplayReadView>,
        buffer_limits: PluginBufferLimits,
    ) -> Self {
        Self {
            read_view,
            reads: GameplayReadSet::default(),
            effects: Vec::new(),
            buffer_limits,
        }
    }

    fn world_meta(&mut self) -> WorldMeta {
        let world_meta = self.read_view.world_meta();
        self.reads.world_meta = Some(world_meta.clone());
        world_meta
    }

    fn player_snapshot(&mut self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        let snapshot = self.read_view.player_snapshot(player_id);
        self.reads
            .player_snapshots
            .insert(player_id, snapshot.clone());
        snapshot
    }

    fn block_state(&mut self, position: BlockPos) -> Option<BlockState> {
        let block_state = self.read_view.block_state(position);
        self.reads
            .block_states
            .insert(position, block_state.clone());
        block_state
    }

    fn block_entity(&mut self, position: BlockPos) -> Option<BlockEntityState> {
        let block_entity = self.read_view.block_entity(position);
        self.reads
            .block_entities
            .insert(position, block_entity.clone());
        block_entity
    }

    fn can_edit_block(&mut self, player_id: PlayerId, position: BlockPos) -> bool {
        let allowed = self.read_view.can_edit_block(player_id, position);
        self.reads.can_edit_block.insert(
            GameplayCanEditBlockKey {
                player_id,
                position,
            },
            allowed,
        );
        allowed
    }

    fn push_effect(&mut self, effect: GameplayEffect) {
        self.effects.push(effect);
    }

    #[must_use]
    pub(crate) fn finish(self, now_ms: u64) -> GameplayEffectBatch {
        GameplayEffectBatch {
            now_ms,
            reads: self.reads,
            effects: self.effects,
        }
    }
}

#[cfg(test)]
pub(crate) struct GameplayQueryProxy<'scope>(&'scope mut GameplayInvocationScope);

#[cfg(test)]
impl GameplayQueryProxy<'_> {
    pub(crate) fn world_meta(&mut self) -> WorldMeta {
        self.0.world_meta()
    }
}

thread_local! {
    static CURRENT_GAMEPLAY_SCOPE: Cell<Option<*mut ()>> = const { Cell::new(None) };
}

pub(crate) fn with_gameplay_invocation_and_limits<T>(
    scope: &mut GameplayInvocationScope,
    f: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    CURRENT_GAMEPLAY_SCOPE.with(|slot| {
        let previous = slot.replace(Some(std::ptr::from_mut(scope).cast()));
        let result = f();
        let _ = slot.replace(previous);
        result
    })
}

#[cfg(test)]
pub(crate) fn with_current_gameplay_query<T>(
    f: impl FnOnce(&mut GameplayQueryProxy<'_>) -> Result<T, String>,
) -> Result<T, String> {
    with_current_gameplay_context(|scope| f(&mut GameplayQueryProxy(scope)))
}

fn with_current_gameplay_context<T>(
    f: impl FnOnce(&mut GameplayInvocationScope) -> Result<T, String>,
) -> Result<T, String> {
    CURRENT_GAMEPLAY_SCOPE.with(|slot| {
        let scope_ptr = slot.get().ok_or_else(|| {
            "gameplay host callback invoked without an active invocation scope".to_string()
        })?;
        let scope = unsafe { &mut *scope_ptr.cast::<GameplayInvocationScope>() };
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
        let bytes = encode_player_snapshot(scope.player_snapshot(player_id).as_ref())
            .map_err(|error| error.to_string())?;
        write_owned_buffer(output, bytes);
        Ok(())
    });
    callback_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_read_world_meta(
    _context: *mut std::ffi::c_void,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_context(|scope| {
        let bytes = encode_world_meta(&scope.world_meta()).map_err(|error| error.to_string())?;
        write_owned_buffer(output, bytes);
        Ok(())
    });
    callback_status(result, error_out)
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
        let bytes = encode_block_state(scope.block_state(position).as_ref())
            .map_err(|error| error.to_string())?;
        write_owned_buffer(output, bytes);
        Ok(())
    });
    callback_status(result, error_out)
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
        let bytes = encode_block_entity(scope.block_entity(position).as_ref())
            .map_err(|error| error.to_string())?;
        write_owned_buffer(output, bytes);
        Ok(())
    });
    callback_status(result, error_out)
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
                *out = scope.can_edit_block(player_id, position);
            }
        }
        Ok(())
    });
    callback_status(result, error_out)
}

unsafe extern "C" fn gameplay_host_push_effect(
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
        let effect = decode_gameplay_effect_blob(payload).map_err(|error| error.to_string())?;
        scope.push_effect(effect);
        Ok(())
    });
    callback_status(result, error_out)
}

fn callback_status(result: Result<(), String>, error_out: *mut OwnedBuffer) -> PluginErrorCode {
    match result {
        Ok(()) => PluginErrorCode::Ok,
        Err(error) => {
            write_error_buffer(error_out, error);
            PluginErrorCode::Internal
        }
    }
}

pub(crate) fn gameplay_host_api() -> GameplayHostApiV3 {
    GameplayHostApiV3 {
        abi: CURRENT_PLUGIN_ABI,
        context: std::ptr::null_mut(),
        log: Some(gameplay_host_log),
        read_player_snapshot: Some(gameplay_host_read_player_snapshot),
        read_world_meta: Some(gameplay_host_read_world_meta),
        read_block_state: Some(gameplay_host_read_block_state),
        read_block_entity: Some(gameplay_host_read_block_entity),
        can_edit_block: Some(gameplay_host_can_edit_block),
        push_effect: Some(gameplay_host_push_effect),
    }
}

pub(crate) fn admin_surface_host_api() -> AdminSurfaceHostApiV1 {
    AdminSurfaceHostApiV1 {
        abi: CURRENT_PLUGIN_ABI,
        context: std::ptr::null_mut(),
        log: None,
        execute: None,
        permissions: None,
        take_process_resource: None,
        publish_handoff_resource: None,
        take_handoff_resource: None,
    }
}

pub(crate) fn write_error_buffer(error_out: *mut OwnedBuffer, message: String) {
    write_owned_buffer(error_out, message.into_bytes());
}
