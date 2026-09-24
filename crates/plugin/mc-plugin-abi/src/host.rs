use crate::PluginAbiVersion;
use crate::raw::{ByteSlice, OwnedBuffer, PluginStatus, Utf8Slice};
use std::ffi::c_void;

pub type HostLogFn = unsafe extern "C" fn(*mut c_void, level: u32, message: Utf8Slice);
pub type HostReadPlayerSnapshotFn = unsafe extern "C" fn(
    *mut c_void,
    ByteSlice,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginStatus;
pub type HostReadWorldMetaFn =
    unsafe extern "C" fn(*mut c_void, *mut OwnedBuffer, *mut OwnedBuffer) -> PluginStatus;
pub type HostReadBlockStateFn = unsafe extern "C" fn(
    *mut c_void,
    ByteSlice,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginStatus;
pub type HostReadBlockEntityFn = unsafe extern "C" fn(
    *mut c_void,
    ByteSlice,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginStatus;
pub type HostCanEditBlockFn =
    unsafe extern "C" fn(*mut c_void, ByteSlice, *mut bool, *mut OwnedBuffer) -> PluginStatus;
pub type GameplayHostPushEffectFn =
    unsafe extern "C" fn(*mut c_void, ByteSlice, *mut OwnedBuffer) -> PluginStatus;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GameplayHostApiV9 {
    pub abi: PluginAbiVersion,
    pub struct_size: usize,
    pub context: *mut c_void,
    pub free_buffer: Option<HostFreeBufferFn>,
    pub log: Option<HostLogFn>,
    pub read_player_snapshot: Option<HostReadPlayerSnapshotFn>,
    pub read_world_meta: Option<HostReadWorldMetaFn>,
    pub read_block_state: Option<HostReadBlockStateFn>,
    pub read_block_entity: Option<HostReadBlockEntityFn>,
    pub can_edit_block: Option<HostCanEditBlockFn>,
    pub push_effect: Option<GameplayHostPushEffectFn>,
}

/// Creates an independent, concurrently invocable plugin object. Success requires a
/// non-null output. Any non-null output transfers ownership, including on failure:
/// the host must retire that object with `destroy_instance` before reporting failure.
/// Returned error buffers use the table's `free_buffer` callback.
pub type PluginCreateInstanceFn =
    unsafe extern "C" fn(*mut *mut c_void, *mut OwnedBuffer) -> PluginStatus;
/// Consumes an object from this table's create callback exactly once. No invocation or
/// plugin-owned work may remain in flight. Destruction must not unwind or fail.
pub type PluginDestroyInstanceFn = unsafe extern "C" fn(*mut c_void);
/// The first argument borrows a live object created by the same function table.
/// Calls may overlap, but destruction cannot overlap any call or retained resource.
pub type PluginInvokeFn = unsafe extern "C" fn(
    *mut c_void,
    ByteSlice,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginStatus;
pub type PluginFreeBufferFn = unsafe extern "C" fn(OwnedBuffer);
pub type HostFreeBufferFn = unsafe extern "C" fn(OwnedBuffer);
pub type GameplayPluginInvokeV9Fn = unsafe extern "C" fn(
    *mut c_void,
    ByteSlice,
    *const GameplayHostApiV9,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginStatus;
pub type AdminSurfaceHostExecuteFn = unsafe extern "C" fn(
    *mut c_void,
    Utf8Slice,
    ByteSlice,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginStatus;
pub type AdminSurfaceHostPermissionsFn = unsafe extern "C" fn(
    *mut c_void,
    Utf8Slice,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginStatus;
pub type AdminSurfaceHostTakeResourceFn = unsafe extern "C" fn(
    *mut c_void,
    Utf8Slice,
    *mut bool,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginStatus;
pub type AdminSurfaceHostPublishResourceFn =
    unsafe extern "C" fn(*mut c_void, Utf8Slice, ByteSlice, *mut OwnedBuffer) -> PluginStatus;
pub type HostRetainContextFn = unsafe extern "C" fn(*mut c_void) -> bool;
pub type HostReleaseContextFn = unsafe extern "C" fn(*mut c_void);
pub type AdminSurfacePluginInvokeV9Fn = unsafe extern "C" fn(
    *mut c_void,
    ByteSlice,
    *const AdminSurfaceHostApiV9,
    *mut OwnedBuffer,
    *mut OwnedBuffer,
) -> PluginStatus;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ProtocolPluginApiV9 {
    pub abi: PluginAbiVersion,
    pub struct_size: usize,
    pub create_instance: Option<PluginCreateInstanceFn>,
    pub destroy_instance: Option<PluginDestroyInstanceFn>,
    pub invoke: Option<PluginInvokeFn>,
    pub free_buffer: Option<PluginFreeBufferFn>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StoragePluginApiV9 {
    pub abi: PluginAbiVersion,
    pub struct_size: usize,
    pub create_instance: Option<PluginCreateInstanceFn>,
    pub destroy_instance: Option<PluginDestroyInstanceFn>,
    pub invoke: Option<PluginInvokeFn>,
    pub free_buffer: Option<PluginFreeBufferFn>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AuthPluginApiV9 {
    pub abi: PluginAbiVersion,
    pub struct_size: usize,
    pub create_instance: Option<PluginCreateInstanceFn>,
    pub destroy_instance: Option<PluginDestroyInstanceFn>,
    pub invoke: Option<PluginInvokeFn>,
    pub free_buffer: Option<PluginFreeBufferFn>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GameplayPluginApiV9 {
    pub abi: PluginAbiVersion,
    pub struct_size: usize,
    pub create_instance: Option<PluginCreateInstanceFn>,
    pub destroy_instance: Option<PluginDestroyInstanceFn>,
    pub invoke: Option<GameplayPluginInvokeV9Fn>,
    pub free_buffer: Option<PluginFreeBufferFn>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AdminSurfaceHostApiV9 {
    pub abi: PluginAbiVersion,
    pub struct_size: usize,
    pub context: *mut c_void,
    pub free_buffer: Option<HostFreeBufferFn>,
    pub retain_context: Option<HostRetainContextFn>,
    pub release_context: Option<HostReleaseContextFn>,
    pub log: Option<HostLogFn>,
    pub execute: Option<AdminSurfaceHostExecuteFn>,
    pub permissions: Option<AdminSurfaceHostPermissionsFn>,
    pub take_process_resource: Option<AdminSurfaceHostTakeResourceFn>,
    pub publish_handoff_resource: Option<AdminSurfaceHostPublishResourceFn>,
    pub take_handoff_resource: Option<AdminSurfaceHostTakeResourceFn>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AdminSurfacePluginApiV9 {
    pub abi: PluginAbiVersion,
    pub struct_size: usize,
    pub create_instance: Option<PluginCreateInstanceFn>,
    pub destroy_instance: Option<PluginDestroyInstanceFn>,
    pub invoke: Option<AdminSurfacePluginInvokeV9Fn>,
    pub free_buffer: Option<PluginFreeBufferFn>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PluginApiHeaderV9 {
    pub abi: PluginAbiVersion,
    pub struct_size: usize,
}
