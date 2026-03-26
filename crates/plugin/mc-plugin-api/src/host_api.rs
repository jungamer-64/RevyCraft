use crate::abi::{ByteSlice, OwnedBuffer, PluginAbiVersion, PluginErrorCode, Utf8Slice};
use std::ffi::c_void;

pub type HostLogFn = unsafe extern "C" fn(level: u32, message: Utf8Slice);
pub type HostReadPlayerSnapshotFn = unsafe extern "C" fn(
    *mut c_void,
    ByteSlice,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginErrorCode;
pub type HostReadWorldMetaFn =
    unsafe extern "C" fn(*mut c_void, *mut OwnedBuffer, *mut OwnedBuffer) -> PluginErrorCode;
pub type HostReadBlockStateFn = unsafe extern "C" fn(
    *mut c_void,
    ByteSlice,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginErrorCode;
pub type HostReadBlockEntityFn = unsafe extern "C" fn(
    *mut c_void,
    ByteSlice,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginErrorCode;
pub type HostCanEditBlockFn =
    unsafe extern "C" fn(*mut c_void, ByteSlice, *mut bool, *mut OwnedBuffer) -> PluginErrorCode;
pub type GameplayHostMutationFn =
    unsafe extern "C" fn(*mut c_void, ByteSlice, *mut OwnedBuffer) -> PluginErrorCode;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HostApiTableV1 {
    pub abi: PluginAbiVersion,
    pub context: *mut c_void,
    pub log: Option<HostLogFn>,
    pub read_player_snapshot: Option<HostReadPlayerSnapshotFn>,
    pub read_world_meta: Option<HostReadWorldMetaFn>,
    pub read_block_state: Option<HostReadBlockStateFn>,
    pub read_block_entity: Option<HostReadBlockEntityFn>,
    pub can_edit_block: Option<HostCanEditBlockFn>,
}

unsafe impl Send for HostApiTableV1 {}
unsafe impl Sync for HostApiTableV1 {}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GameplayHostApiV2 {
    pub abi: PluginAbiVersion,
    pub context: *mut c_void,
    pub log: Option<HostLogFn>,
    pub read_player_snapshot: Option<HostReadPlayerSnapshotFn>,
    pub read_world_meta: Option<HostReadWorldMetaFn>,
    pub read_block_state: Option<HostReadBlockStateFn>,
    pub read_block_entity: Option<HostReadBlockEntityFn>,
    pub can_edit_block: Option<HostCanEditBlockFn>,
    pub set_player_pose: Option<GameplayHostMutationFn>,
    pub set_selected_hotbar_slot: Option<GameplayHostMutationFn>,
    pub set_inventory_slot: Option<GameplayHostMutationFn>,
    pub clear_mining: Option<GameplayHostMutationFn>,
    pub begin_mining: Option<GameplayHostMutationFn>,
    pub open_chest: Option<GameplayHostMutationFn>,
    pub open_furnace: Option<GameplayHostMutationFn>,
    pub set_block: Option<GameplayHostMutationFn>,
    pub spawn_dropped_item: Option<GameplayHostMutationFn>,
    pub emit_event: Option<GameplayHostMutationFn>,
}

unsafe impl Send for GameplayHostApiV2 {}
unsafe impl Sync for GameplayHostApiV2 {}

pub type PluginInvokeFn =
    unsafe extern "C" fn(ByteSlice, *mut OwnedBuffer, *mut OwnedBuffer) -> PluginErrorCode;
pub type PluginFreeBufferFn = unsafe extern "C" fn(OwnedBuffer);
pub type GameplayPluginInvokeV3Fn = unsafe extern "C" fn(
    ByteSlice,
    *const GameplayHostApiV2,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginErrorCode;
pub type AdminUiPluginInvokeV1Fn = unsafe extern "C" fn(
    ByteSlice,
    *const HostApiTableV1,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginErrorCode;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ProtocolPluginApiV3 {
    pub invoke: PluginInvokeFn,
    pub free_buffer: PluginFreeBufferFn,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StoragePluginApiV1 {
    pub invoke: PluginInvokeFn,
    pub free_buffer: PluginFreeBufferFn,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AuthPluginApiV1 {
    pub invoke: PluginInvokeFn,
    pub free_buffer: PluginFreeBufferFn,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GameplayPluginApiV3 {
    pub invoke: GameplayPluginInvokeV3Fn,
    pub free_buffer: PluginFreeBufferFn,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AdminUiPluginApiV1 {
    pub invoke: AdminUiPluginInvokeV1Fn,
    pub free_buffer: PluginFreeBufferFn,
}
