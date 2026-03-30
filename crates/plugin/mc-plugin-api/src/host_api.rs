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
pub type GameplayHostPushEffectFn =
    unsafe extern "C" fn(*mut c_void, ByteSlice, *mut OwnedBuffer) -> PluginErrorCode;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GameplayHostApiV3 {
    pub abi: PluginAbiVersion,
    pub context: *mut c_void,
    pub log: Option<HostLogFn>,
    pub read_player_snapshot: Option<HostReadPlayerSnapshotFn>,
    pub read_world_meta: Option<HostReadWorldMetaFn>,
    pub read_block_state: Option<HostReadBlockStateFn>,
    pub read_block_entity: Option<HostReadBlockEntityFn>,
    pub can_edit_block: Option<HostCanEditBlockFn>,
    pub push_effect: Option<GameplayHostPushEffectFn>,
}

unsafe impl Send for GameplayHostApiV3 {}
unsafe impl Sync for GameplayHostApiV3 {}

pub type PluginInvokeFn =
    unsafe extern "C" fn(ByteSlice, *mut OwnedBuffer, *mut OwnedBuffer) -> PluginErrorCode;
pub type PluginFreeBufferFn = unsafe extern "C" fn(OwnedBuffer);
pub type GameplayPluginInvokeV4Fn = unsafe extern "C" fn(
    ByteSlice,
    *const GameplayHostApiV3,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginErrorCode;
pub type AdminSurfaceHostExecuteFn = unsafe extern "C" fn(
    *mut c_void,
    Utf8Slice,
    ByteSlice,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginErrorCode;
pub type AdminSurfaceHostPermissionsFn = unsafe extern "C" fn(
    *mut c_void,
    Utf8Slice,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginErrorCode;
pub type AdminSurfaceHostTakeResourceFn = unsafe extern "C" fn(
    *mut c_void,
    Utf8Slice,
    *mut bool,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginErrorCode;
pub type AdminSurfaceHostPublishResourceFn =
    unsafe extern "C" fn(*mut c_void, Utf8Slice, ByteSlice, *mut OwnedBuffer) -> PluginErrorCode;
pub type AdminSurfacePluginInvokeV1Fn = unsafe extern "C" fn(
    ByteSlice,
    *const AdminSurfaceHostApiV1,
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
pub struct GameplayPluginApiV4 {
    pub invoke: GameplayPluginInvokeV4Fn,
    pub free_buffer: PluginFreeBufferFn,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AdminSurfaceHostApiV1 {
    pub abi: PluginAbiVersion,
    pub context: *mut c_void,
    pub log: Option<HostLogFn>,
    pub execute: Option<AdminSurfaceHostExecuteFn>,
    pub permissions: Option<AdminSurfaceHostPermissionsFn>,
    pub take_process_resource: Option<AdminSurfaceHostTakeResourceFn>,
    pub publish_handoff_resource: Option<AdminSurfaceHostPublishResourceFn>,
    pub take_handoff_resource: Option<AdminSurfaceHostTakeResourceFn>,
}

unsafe impl Send for AdminSurfaceHostApiV1 {}
unsafe impl Sync for AdminSurfaceHostApiV1 {}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AdminSurfacePluginApiV1 {
    pub invoke: AdminSurfacePluginInvokeV1Fn,
    pub free_buffer: PluginFreeBufferFn,
}
