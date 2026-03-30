use crate::gameplay::{GameplayHost, RustGameplayPlugin};
use crate::{
    GameplayCanEditBlockKey, GameplayEffect, GameplayEffectBatch, GameplayReadSet, PlayerId,
    PlayerSnapshot, TargetedEvent,
};
use mc_plugin_api::abi::{ByteSlice, OwnedBuffer, PluginErrorCode, Utf8Slice};
use mc_plugin_api::codec::gameplay::host_blob::{
    decode_block_entity, decode_block_state, decode_gameplay_effect_blob, decode_player_snapshot,
    decode_targeted_event_blob, decode_world_meta, encode_block_pos, encode_can_edit_block_key,
    encode_gameplay_effect_blob, encode_player_id,
};
use mc_plugin_api::codec::gameplay::{GameplayRequest, GameplayResponse};
use mc_plugin_api::host_api::GameplayHostApiV3;
use revy_voxel_model::{BlockPos, BlockState, InventorySlot, ItemStack, Vec3, WorldMeta};
use revy_voxel_rules::{BlockEntityState, ContainerKindId};
use std::cell::RefCell;

#[derive(Default)]
struct GameplayInvocationRecorder {
    now_ms: u64,
    reads: GameplayReadSet,
    effects: Vec<GameplayEffect>,
}

impl GameplayInvocationRecorder {
    fn new(now_ms: u64) -> Self {
        Self {
            now_ms,
            reads: GameplayReadSet::default(),
            effects: Vec::new(),
        }
    }

    fn record_player_snapshot(&mut self, player_id: PlayerId, snapshot: Option<PlayerSnapshot>) {
        self.reads.player_snapshots.insert(player_id, snapshot);
    }

    fn record_world_meta(&mut self, world_meta: WorldMeta) {
        self.reads.world_meta = Some(world_meta);
    }

    fn record_block_state(&mut self, position: BlockPos, block: Option<BlockState>) {
        self.reads.block_states.insert(position, block);
    }

    fn record_block_entity(&mut self, position: BlockPos, entity: Option<BlockEntityState>) {
        self.reads.block_entities.insert(position, entity);
    }

    fn record_can_edit_block(&mut self, player_id: PlayerId, position: BlockPos, allowed: bool) {
        self.reads.can_edit_block.insert(
            GameplayCanEditBlockKey {
                player_id,
                position,
            },
            allowed,
        );
    }

    fn push_effect(&mut self, effect: GameplayEffect) {
        self.effects.push(effect);
    }

    fn finish(self) -> GameplayEffectBatch {
        GameplayEffectBatch {
            now_ms: self.now_ms,
            reads: self.reads,
            effects: self.effects,
        }
    }
}

struct SdkGameplayHost {
    api: GameplayHostApiV3,
    recorder: RefCell<GameplayInvocationRecorder>,
}

impl SdkGameplayHost {
    fn new(api: GameplayHostApiV3, now_ms: u64) -> Self {
        Self {
            api,
            recorder: RefCell::new(GameplayInvocationRecorder::new(now_ms)),
        }
    }

    fn push_effect(&self, effect: GameplayEffect) -> Result<(), String> {
        let Some(callback) = self.api.push_effect else {
            return Err("gameplay host did not provide push_effect".to_string());
        };
        let payload = encode_gameplay_effect_blob(&effect).map_err(|error| error.to_string())?;
        call_host_mutation(self.api.context, &payload, callback)?;
        self.recorder.borrow_mut().push_effect(effect);
        Ok(())
    }

    fn finish(self) -> GameplayEffectBatch {
        self.recorder.into_inner().finish()
    }
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
        let snapshot = decode_player_snapshot(&bytes).map_err(|error| error.to_string())?;
        self.recorder
            .borrow_mut()
            .record_player_snapshot(player_id, snapshot.clone());
        Ok(snapshot)
    }

    fn read_world_meta(&self) -> Result<WorldMeta, String> {
        let Some(callback) = self.api.read_world_meta else {
            return Err("gameplay host did not provide read_world_meta".to_string());
        };
        let bytes = call_host_zero_arg(self.api.context, callback)?;
        let world_meta = decode_world_meta(&bytes).map_err(|error| error.to_string())?;
        self.recorder
            .borrow_mut()
            .record_world_meta(world_meta.clone());
        Ok(world_meta)
    }

    fn read_block_state(&self, position: BlockPos) -> Result<Option<BlockState>, String> {
        let Some(callback) = self.api.read_block_state else {
            return Err("gameplay host did not provide read_block_state".to_string());
        };
        let payload = encode_block_pos(position);
        let bytes = call_host_buffer(self.api.context, &payload, callback)?;
        let block_state = decode_block_state(&bytes).map_err(|error| error.to_string())?;
        self.recorder
            .borrow_mut()
            .record_block_state(position, block_state.clone());
        Ok(block_state)
    }

    fn read_block_entity(&self, position: BlockPos) -> Result<Option<BlockEntityState>, String> {
        let Some(callback) = self.api.read_block_entity else {
            return Err("gameplay host did not provide read_block_entity".to_string());
        };
        let payload = encode_block_pos(position);
        let bytes = call_host_buffer(self.api.context, &payload, callback)?;
        let block_entity = decode_block_entity(&bytes).map_err(|error| error.to_string())?;
        self.recorder
            .borrow_mut()
            .record_block_entity(position, block_entity.clone());
        Ok(block_entity)
    }

    fn can_edit_block(&self, player_id: PlayerId, position: BlockPos) -> Result<bool, String> {
        let Some(callback) = self.api.can_edit_block else {
            return Err("gameplay host did not provide can_edit_block".to_string());
        };
        let payload = encode_can_edit_block_key(player_id, position);
        let allowed = call_host_bool(self.api.context, &payload, callback)?;
        self.recorder
            .borrow_mut()
            .record_can_edit_block(player_id, position, allowed);
        Ok(allowed)
    }

    fn set_player_pose(
        &self,
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    ) -> Result<(), String> {
        self.push_effect(GameplayEffect::SetPlayerPose {
            player_id,
            position,
            yaw,
            pitch,
            on_ground,
        })
    }

    fn set_selected_hotbar_slot(&self, player_id: PlayerId, slot: u8) -> Result<(), String> {
        self.push_effect(GameplayEffect::SetSelectedHotbarSlot { player_id, slot })
    }

    fn set_inventory_slot(
        &self,
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    ) -> Result<(), String> {
        self.push_effect(GameplayEffect::SetInventorySlot {
            player_id,
            slot,
            stack,
        })
    }

    fn clear_mining(&self, player_id: PlayerId) -> Result<(), String> {
        self.push_effect(GameplayEffect::ClearMining { player_id })
    }

    fn begin_mining(
        &self,
        player_id: PlayerId,
        position: BlockPos,
        duration_ms: u64,
    ) -> Result<(), String> {
        self.push_effect(GameplayEffect::BeginMining {
            player_id,
            position,
            duration_ms,
        })
    }

    fn open_container_at(&self, player_id: PlayerId, position: BlockPos) -> Result<(), String> {
        self.push_effect(GameplayEffect::OpenContainerAt {
            player_id,
            position,
        })
    }

    fn open_virtual_container(
        &self,
        player_id: PlayerId,
        kind: &ContainerKindId,
    ) -> Result<(), String> {
        self.push_effect(GameplayEffect::OpenVirtualContainer {
            player_id,
            kind: kind.clone(),
        })
    }

    fn set_block(&self, position: BlockPos, block: Option<BlockState>) -> Result<(), String> {
        self.push_effect(GameplayEffect::SetBlock { position, block })
    }

    fn spawn_dropped_item(&self, position: Vec3, item: ItemStack) -> Result<(), String> {
        self.push_effect(GameplayEffect::SpawnDroppedItem { position, item })
    }

    fn emit_event(&self, event: TargetedEvent) -> Result<(), String> {
        self.push_effect(GameplayEffect::EmitEvent { event })
    }
}

fn with_gameplay_host_api<T>(
    api: GameplayHostApiV3,
    now_ms: u64,
    f: impl FnOnce(&dyn GameplayHost) -> Result<T, String>,
) -> Result<(T, GameplayEffectBatch), String> {
    let host = SdkGameplayHost::new(api, now_ms);
    let output = f(&host)?;
    Ok((output, host.finish()))
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

fn call_host_mutation(
    context: *mut std::ffi::c_void,
    payload: &[u8],
    callback: unsafe extern "C" fn(
        *mut std::ffi::c_void,
        ByteSlice,
        *mut OwnedBuffer,
    ) -> PluginErrorCode,
) -> Result<(), String> {
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        callback(
            context,
            ByteSlice {
                ptr: payload.as_ptr(),
                len: payload.len(),
            },
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        return Err(read_error_buffer(error));
    }
    Ok(())
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

#[allow(dead_code)]
fn _decode_targeted_event_for_tests(bytes: &[u8]) -> Result<TargetedEvent, String> {
    decode_targeted_event_blob(bytes).map_err(|error| error.to_string())
}

#[allow(dead_code)]
fn _decode_gameplay_effect_for_tests(bytes: &[u8]) -> Result<GameplayEffect, String> {
    decode_gameplay_effect_blob(bytes).map_err(|error| error.to_string())
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
    host_api: Option<GameplayHostApiV3>,
) -> Result<GameplayResponse, String> {
    let require_host_api =
        || host_api.ok_or_else(|| "gameplay host api is not configured".to_string());
    match request {
        GameplayRequest::Describe => Ok(GameplayResponse::Descriptor(plugin.descriptor())),
        GameplayRequest::CapabilitySet => Ok(GameplayResponse::CapabilitySet(
            crate::capabilities::gameplay_announcement(&plugin.capability_set()),
        )),
        GameplayRequest::HandlePlayerJoin {
            session,
            player_id,
            now_ms,
        } => with_gameplay_host_api(require_host_api()?, now_ms, |host| {
            plugin.handle_player_join(host, &session, player_id)?;
            Ok(())
        })
        .map(|(_, batch)| GameplayResponse::EffectBatch(batch)),
        GameplayRequest::HandleCommand {
            session,
            command,
            now_ms,
        } => with_gameplay_host_api(require_host_api()?, now_ms, |host| {
            plugin.handle_command(host, &session, &command)?;
            Ok(())
        })
        .map(|(_, batch)| GameplayResponse::EffectBatch(batch)),
        GameplayRequest::HandleTick { session, now_ms } => {
            with_gameplay_host_api(require_host_api()?, now_ms, |host| {
                plugin.handle_tick(host, &session, now_ms)?;
                Ok(())
            })
            .map(|(_, batch)| GameplayResponse::EffectBatch(batch))
        }
        GameplayRequest::SessionClosed { session } => {
            with_gameplay_host_api(require_host_api()?, 0, |host| {
                plugin.session_closed(host, &session)?;
                Ok(())
            })?;
            Ok(GameplayResponse::Empty)
        }
        GameplayRequest::ExportSessionState { session } => {
            with_gameplay_host_api(require_host_api()?, 0, |host| {
                plugin.export_session_state(host, &session)
            })
            .map(|(blob, _)| GameplayResponse::SessionTransferBlob(blob))
        }
        GameplayRequest::ImportSessionState { session, blob } => {
            with_gameplay_host_api(require_host_api()?, 0, |host| {
                plugin.import_session_state(host, &session, &blob)?;
                Ok(())
            })?;
            Ok(GameplayResponse::Empty)
        }
    }
}
