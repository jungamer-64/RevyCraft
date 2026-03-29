use crate::gameplay::{GameplayHost, RustGameplayPlugin};
use mc_content_api::{BlockEntityState, ContainerKindId};
use mc_core::{PlayerId, PlayerSnapshot, TargetedEvent};
use mc_model::{BlockPos, BlockState, InventorySlot, ItemStack, Vec3, WorldMeta};
use mc_plugin_api::abi::{ByteSlice, OwnedBuffer, PluginErrorCode, Utf8Slice};
use mc_plugin_api::codec::gameplay::host_blob::{
    decode_block_entity, decode_block_state, decode_player_snapshot, decode_targeted_event_blob,
    decode_world_meta, encode_begin_mining, encode_block_pos, encode_can_edit_block_key,
    encode_clear_mining, encode_inventory_slot_update, encode_open_container_at,
    encode_open_virtual_container, encode_player_id, encode_player_pose_update,
    encode_selected_hotbar_slot_update, encode_set_block, encode_spawn_dropped_item,
    encode_targeted_event_blob,
};
use mc_plugin_api::codec::gameplay::{GameplayRequest, GameplayResponse};
use mc_plugin_api::host_api::GameplayHostApiV2;

struct SdkGameplayHost {
    api: GameplayHostApiV2,
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

    fn read_block_state(&self, position: BlockPos) -> Result<Option<BlockState>, String> {
        let Some(callback) = self.api.read_block_state else {
            return Err("gameplay host did not provide read_block_state".to_string());
        };
        let payload = encode_block_pos(position);
        let bytes = call_host_buffer(self.api.context, &payload, callback)?;
        decode_block_state(&bytes).map_err(|error| error.to_string())
    }

    fn read_block_entity(&self, position: BlockPos) -> Result<Option<BlockEntityState>, String> {
        let Some(callback) = self.api.read_block_entity else {
            return Err("gameplay host did not provide read_block_entity".to_string());
        };
        let payload = encode_block_pos(position);
        let bytes = call_host_buffer(self.api.context, &payload, callback)?;
        decode_block_entity(&bytes).map_err(|error| error.to_string())
    }

    fn can_edit_block(&self, player_id: PlayerId, position: BlockPos) -> Result<bool, String> {
        let Some(callback) = self.api.can_edit_block else {
            return Err("gameplay host did not provide can_edit_block".to_string());
        };
        let payload = encode_can_edit_block_key(player_id, position);
        call_host_bool(self.api.context, &payload, callback)
    }

    fn set_player_pose(
        &self,
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    ) -> Result<(), String> {
        let Some(callback) = self.api.set_player_pose else {
            return Err("gameplay host did not provide set_player_pose".to_string());
        };
        let payload = encode_player_pose_update(player_id, position, yaw, pitch, on_ground);
        call_host_mutation(self.api.context, &payload, callback)
    }

    fn set_selected_hotbar_slot(&self, player_id: PlayerId, slot: u8) -> Result<(), String> {
        let Some(callback) = self.api.set_selected_hotbar_slot else {
            return Err("gameplay host did not provide set_selected_hotbar_slot".to_string());
        };
        let payload = encode_selected_hotbar_slot_update(player_id, slot);
        call_host_mutation(self.api.context, &payload, callback)
    }

    fn set_inventory_slot(
        &self,
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    ) -> Result<(), String> {
        let Some(callback) = self.api.set_inventory_slot else {
            return Err("gameplay host did not provide set_inventory_slot".to_string());
        };
        let payload = encode_inventory_slot_update(player_id, slot, stack.as_ref())
            .map_err(|e| e.to_string())?;
        call_host_mutation(self.api.context, &payload, callback)
    }

    fn clear_mining(&self, player_id: PlayerId) -> Result<(), String> {
        let Some(callback) = self.api.clear_mining else {
            return Err("gameplay host did not provide clear_mining".to_string());
        };
        let payload = encode_clear_mining(player_id);
        call_host_mutation(self.api.context, &payload, callback)
    }

    fn begin_mining(
        &self,
        player_id: PlayerId,
        position: BlockPos,
        duration_ms: u64,
    ) -> Result<(), String> {
        let Some(callback) = self.api.begin_mining else {
            return Err("gameplay host did not provide begin_mining".to_string());
        };
        let payload = encode_begin_mining(player_id, position, duration_ms);
        call_host_mutation(self.api.context, &payload, callback)
    }

    fn open_container_at(&self, player_id: PlayerId, position: BlockPos) -> Result<(), String> {
        let Some(callback) = self.api.open_container_at else {
            return Err("gameplay host did not provide open_container_at".to_string());
        };
        let payload = encode_open_container_at(player_id, position);
        call_host_mutation(self.api.context, &payload, callback)
    }

    fn open_virtual_container(
        &self,
        player_id: PlayerId,
        kind: &ContainerKindId,
    ) -> Result<(), String> {
        let Some(callback) = self.api.open_virtual_container else {
            return Err("gameplay host did not provide open_virtual_container".to_string());
        };
        let payload = encode_open_virtual_container(player_id, kind);
        call_host_mutation(self.api.context, &payload, callback)
    }

    fn set_block(&self, position: BlockPos, block: Option<BlockState>) -> Result<(), String> {
        let Some(callback) = self.api.set_block else {
            return Err("gameplay host did not provide set_block".to_string());
        };
        let payload = encode_set_block(position, block.as_ref()).map_err(|e| e.to_string())?;
        call_host_mutation(self.api.context, &payload, callback)
    }

    fn spawn_dropped_item(&self, position: Vec3, item: ItemStack) -> Result<(), String> {
        let Some(callback) = self.api.spawn_dropped_item else {
            return Err("gameplay host did not provide spawn_dropped_item".to_string());
        };
        let payload = encode_spawn_dropped_item(position, &item).map_err(|e| e.to_string())?;
        call_host_mutation(self.api.context, &payload, callback)
    }

    fn emit_event(&self, event: TargetedEvent) -> Result<(), String> {
        let Some(callback) = self.api.emit_event else {
            return Err("gameplay host did not provide emit_event".to_string());
        };
        let payload = encode_targeted_event_blob(&event).map_err(|e| e.to_string())?;
        call_host_mutation(self.api.context, &payload, callback)
    }
}

fn with_gameplay_host_api<T>(
    api: GameplayHostApiV2,
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

pub fn handle_gameplay_request<P: RustGameplayPlugin>(
    plugin: &P,
    request: GameplayRequest,
) -> Result<GameplayResponse, String> {
    handle_gameplay_request_with_host_api(plugin, request, None)
}

pub fn handle_gameplay_request_with_host_api<P: RustGameplayPlugin>(
    plugin: &P,
    request: GameplayRequest,
    host_api: Option<GameplayHostApiV2>,
) -> Result<GameplayResponse, String> {
    let require_host_api =
        || host_api.ok_or_else(|| "gameplay host api is not configured".to_string());
    match request {
        GameplayRequest::Describe => Ok(GameplayResponse::Descriptor(plugin.descriptor())),
        GameplayRequest::CapabilitySet => Ok(GameplayResponse::CapabilitySet(
            crate::capabilities::gameplay_announcement(&plugin.capability_set()),
        )),
        GameplayRequest::HandlePlayerJoin { session, player_id } => {
            with_gameplay_host_api(require_host_api()?, |host| {
                plugin.handle_player_join(host, &session, player_id)?;
                Ok(GameplayResponse::Empty)
            })
        }
        GameplayRequest::HandleCommand { session, command } => {
            with_gameplay_host_api(require_host_api()?, |host| {
                plugin.handle_command(host, &session, &command)?;
                Ok(GameplayResponse::Empty)
            })
        }
        GameplayRequest::HandleTick { session, now_ms } => {
            with_gameplay_host_api(require_host_api()?, |host| {
                plugin.handle_tick(host, &session, now_ms)?;
                Ok(GameplayResponse::Empty)
            })
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
