use crate::codec::protocol::ProtocolCodecError;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const CURRENT_PLUGIN_ABI: PluginAbiVersion = PluginAbiVersion { major: 4, minor: 0 };

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    AdminUi = 5,
}

impl TryFrom<u8> for PluginKind {
    type Error = ProtocolCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Protocol),
            2 => Ok(Self::Storage),
            3 => Ok(Self::Auth),
            4 => Ok(Self::Gameplay),
            5 => Ok(Self::AdminUi),
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
