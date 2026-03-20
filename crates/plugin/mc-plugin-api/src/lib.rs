#![allow(clippy::multiple_crate_versions)]
mod auth_codec;
mod gameplay_codec;
pub(crate) mod protocol_codec;
mod storage_codec;

use std::ffi::c_void;
use std::fmt;

pub use auth_codec::{
    AuthDescriptor, AuthMode, AuthRequest, AuthResponse, BedrockAuthResult, decode_auth_request,
    decode_auth_response, encode_auth_request, encode_auth_response,
};
pub use gameplay_codec::{
    GameplayDescriptor, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
    decode_gameplay_request, decode_gameplay_response, decode_host_block_pos_blob,
    decode_host_block_state_blob, decode_host_can_edit_block_key, decode_host_player_id_blob,
    decode_host_player_snapshot_blob, decode_host_world_meta_blob, encode_gameplay_request,
    encode_gameplay_response, encode_host_block_pos_blob, encode_host_block_state_blob,
    encode_host_can_edit_block_key, encode_host_player_id_blob, encode_host_player_snapshot_blob,
    encode_host_world_meta_blob,
};
pub use protocol_codec::{
    PLUGIN_ENVELOPE_HEADER_LEN, PROTOCOL_FLAG_RESPONSE, ProtocolCodecError, ProtocolOpCode,
    ProtocolRequest, ProtocolResponse, ProtocolSessionSnapshot, WireFrameDecodeResult,
    decode_protocol_request, decode_protocol_response, encode_protocol_request,
    encode_protocol_response,
};
pub use storage_codec::{
    StorageDescriptor, StorageRequest, StorageResponse, decode_storage_request,
    decode_storage_response, encode_storage_request, encode_storage_response,
};

pub const PLUGIN_MANIFEST_SYMBOL_V1: &[u8] = b"mc_plugin_manifest_v1\0";
pub const PLUGIN_PROTOCOL_API_SYMBOL_V1: &[u8] = b"mc_plugin_protocol_api_v1\0";
pub const PLUGIN_STORAGE_API_SYMBOL_V1: &[u8] = b"mc_plugin_storage_api_v1\0";
pub const PLUGIN_AUTH_API_SYMBOL_V1: &[u8] = b"mc_plugin_auth_api_v1\0";
pub const PLUGIN_GAMEPLAY_API_SYMBOL_V1: &[u8] = b"mc_plugin_gameplay_api_v1\0";

pub const CURRENT_PLUGIN_ABI: PluginAbiVersion = PluginAbiVersion { major: 1, minor: 3 };

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginAbiVersion {
    pub major: u16,
    pub minor: u16,
}

impl fmt::Display for PluginAbiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PluginKind {
    Protocol = 1,
    Storage = 2,
    Auth = 3,
    Gameplay = 4,
}

impl TryFrom<u8> for PluginKind {
    type Error = ProtocolCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Protocol),
            2 => Ok(Self::Storage),
            3 => Ok(Self::Auth),
            4 => Ok(Self::Gameplay),
            _ => Err(ProtocolCodecError::InvalidPluginKind(value)),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginErrorCode {
    Ok = 0,
    InvalidInput = 1,
    Internal = 2,
    Unsupported = 3,
    AbiMismatch = 4,
    Quarantined = 5,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Utf8Slice {
    pub ptr: *const u8,
    pub len: usize,
}

impl Utf8Slice {
    #[must_use]
    pub const fn from_static_str(value: &'static str) -> Self {
        Self {
            ptr: value.as_ptr(),
            len: value.len(),
        }
    }
}

unsafe impl Send for Utf8Slice {}
unsafe impl Sync for Utf8Slice {}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteSlice {
    pub ptr: *const u8,
    pub len: usize,
}

impl ByteSlice {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            ptr: std::ptr::null(),
            len: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnedBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub cap: usize,
}

impl OwnedBuffer {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
            cap: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityDescriptorV1 {
    pub name: Utf8Slice,
}

unsafe impl Send for CapabilityDescriptorV1 {}
unsafe impl Sync for CapabilityDescriptorV1 {}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionTransferBlobV1 {
    pub bytes: ByteSlice,
}

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
    pub can_edit_block: Option<HostCanEditBlockFn>,
}

unsafe impl Send for HostApiTableV1 {}
unsafe impl Sync for HostApiTableV1 {}

pub type PluginInvokeFn =
    unsafe extern "C" fn(ByteSlice, *mut OwnedBuffer, *mut OwnedBuffer) -> PluginErrorCode;
pub type PluginFreeBufferFn = unsafe extern "C" fn(OwnedBuffer);
pub type GameplaySetHostApiFn = unsafe extern "C" fn(*const HostApiTableV1) -> PluginErrorCode;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PluginManifestV1 {
    pub plugin_id: Utf8Slice,
    pub display_name: Utf8Slice,
    pub plugin_kind: PluginKind,
    pub plugin_abi: PluginAbiVersion,
    pub min_host_abi: PluginAbiVersion,
    pub max_host_abi: PluginAbiVersion,
    pub capabilities: *const CapabilityDescriptorV1,
    pub capabilities_len: usize,
}

unsafe impl Send for PluginManifestV1 {}
unsafe impl Sync for PluginManifestV1 {}

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
pub struct GameplayPluginApiV1 {
    pub set_host_api: GameplaySetHostApiFn,
    pub invoke: PluginInvokeFn,
    pub free_buffer: PluginFreeBufferFn,
}
