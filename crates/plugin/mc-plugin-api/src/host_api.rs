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

pub type PluginInvokeFn =
    unsafe extern "C" fn(ByteSlice, *mut OwnedBuffer, *mut OwnedBuffer) -> PluginErrorCode;
pub type PluginFreeBufferFn = unsafe extern "C" fn(OwnedBuffer);
pub type GameplayPluginInvokeV2Fn = unsafe extern "C" fn(
    ByteSlice,
    *const HostApiTableV1,
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
pub struct ProtocolPluginApiV1 {
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
pub struct GameplayPluginApiV2 {
    pub invoke: GameplayPluginInvokeV2Fn,
    pub free_buffer: PluginFreeBufferFn,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AdminUiPluginApiV1 {
    pub invoke: AdminUiPluginInvokeV1Fn,
    pub free_buffer: PluginFreeBufferFn,
}
