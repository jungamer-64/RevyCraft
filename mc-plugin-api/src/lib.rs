use mc_core::{CapabilitySet, CoreCommand, CoreEvent, EntityId, PlayerId, PlayerSnapshot};
use mc_proto_common::{
    ConnectionPhase, HandshakeIntent, LoginRequest, PlayEncodingContext, ProtocolDescriptor,
    ServerListStatus, StatusRequest,
};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const PLUGIN_MANIFEST_SYMBOL_V1: &[u8] = b"mc_plugin_manifest_v1\0";
pub const PLUGIN_PROTOCOL_API_SYMBOL_V1: &[u8] = b"mc_plugin_protocol_api_v1\0";
pub const PLUGIN_STORAGE_API_SYMBOL_V1: &[u8] = b"mc_plugin_storage_api_v1\0";
pub const PLUGIN_AUTH_API_SYMBOL_V1: &[u8] = b"mc_plugin_auth_api_v1\0";
pub const PLUGIN_GAMEPLAY_API_SYMBOL_V1: &[u8] = b"mc_plugin_gameplay_api_v1\0";

pub const CURRENT_PLUGIN_ABI: PluginAbiVersion = PluginAbiVersion { major: 1, minor: 0 };

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

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HostApiTableV1 {
    pub abi: PluginAbiVersion,
    pub log: Option<HostLogFn>,
}

pub type PluginInvokeFn =
    unsafe extern "C" fn(ByteSlice, *mut OwnedBuffer, *mut OwnedBuffer) -> PluginErrorCode;
pub type PluginFreeBufferFn = unsafe extern "C" fn(OwnedBuffer);

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
    pub invoke: PluginInvokeFn,
    pub free_buffer: PluginFreeBufferFn,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolSessionSnapshot {
    pub phase: ConnectionPhase,
    pub player_id: Option<PlayerId>,
    pub entity_id: Option<EntityId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProtocolRequest {
    Describe,
    CapabilitySet,
    TryRoute {
        frame: Vec<u8>,
    },
    DecodeStatus {
        frame: Vec<u8>,
    },
    DecodeLogin {
        frame: Vec<u8>,
    },
    EncodeStatusResponse {
        status: ServerListStatus,
    },
    EncodeStatusPong {
        payload: i64,
    },
    EncodeDisconnect {
        phase: ConnectionPhase,
        reason: String,
    },
    EncodeLoginSuccess {
        player: PlayerSnapshot,
    },
    DecodePlay {
        player_id: PlayerId,
        frame: Vec<u8>,
    },
    EncodePlayEvent {
        event: CoreEvent,
        context: PlayEncodingContext,
    },
    ExportSessionState {
        session: ProtocolSessionSnapshot,
    },
    ImportSessionState {
        session: ProtocolSessionSnapshot,
        blob: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProtocolResponse {
    Descriptor(ProtocolDescriptor),
    CapabilitySet(CapabilitySet),
    HandshakeIntent(Option<HandshakeIntent>),
    StatusRequest(StatusRequest),
    LoginRequest(LoginRequest),
    Frame(Vec<u8>),
    Frames(Vec<Vec<u8>>),
    CoreCommand(Option<CoreCommand>),
    SessionTransferBlob(Vec<u8>),
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageRequest {
    Describe,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageResponse {
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthRequest {
    Describe,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthResponse {
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameplayRequest {
    Describe,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameplayResponse {
    Empty,
}
