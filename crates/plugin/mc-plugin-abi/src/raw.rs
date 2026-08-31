use mc_plugin_contract::plugin::{InvalidPluginKind, PluginKind};

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PluginKindTag(pub u32);

impl PluginKindTag {
    pub const PROTOCOL: Self = Self(1);
    pub const STORAGE: Self = Self(2);
    pub const AUTH: Self = Self(3);
    pub const GAMEPLAY: Self = Self(4);
    pub const ADMIN_SURFACE: Self = Self(7);
}

impl From<PluginKind> for PluginKindTag {
    fn from(value: PluginKind) -> Self {
        match value {
            PluginKind::Protocol => Self::PROTOCOL,
            PluginKind::Storage => Self::STORAGE,
            PluginKind::Auth => Self::AUTH,
            PluginKind::Gameplay => Self::GAMEPLAY,
            PluginKind::AdminSurface => Self::ADMIN_SURFACE,
        }
    }
}

impl TryFrom<PluginKindTag> for PluginKind {
    type Error = InvalidPluginKindTag;

    fn try_from(value: PluginKindTag) -> Result<Self, Self::Error> {
        let tag = u8::try_from(value.0).map_err(|_| InvalidPluginKindTag(value.0))?;
        PluginKind::try_from(tag).map_err(|InvalidPluginKind(_)| InvalidPluginKindTag(value.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown plugin kind tag {0}")]
pub struct InvalidPluginKindTag(pub u32);

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PluginStatus(pub u32);

impl PluginStatus {
    pub const OK: Self = Self(0);
    pub const INVALID_INPUT: Self = Self(1);
    pub const INTERNAL: Self = Self(2);
    pub const UNSUPPORTED: Self = Self(3);
    pub const ABI_MISMATCH: Self = Self(4);
    pub const QUARANTINED: Self = Self(5);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidatedPluginStatus {
    Ok,
    InvalidInput,
    Internal,
    Unsupported,
    AbiMismatch,
    Quarantined,
}

impl TryFrom<PluginStatus> for ValidatedPluginStatus {
    type Error = InvalidPluginStatus;

    fn try_from(value: PluginStatus) -> Result<Self, Self::Error> {
        match value {
            PluginStatus::OK => Ok(Self::Ok),
            PluginStatus::INVALID_INPUT => Ok(Self::InvalidInput),
            PluginStatus::INTERNAL => Ok(Self::Internal),
            PluginStatus::UNSUPPORTED => Ok(Self::Unsupported),
            PluginStatus::ABI_MISMATCH => Ok(Self::AbiMismatch),
            PluginStatus::QUARANTINED => Ok(Self::Quarantined),
            PluginStatus(tag) => Err(InvalidPluginStatus(tag)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown plugin status tag {0}")]
pub struct InvalidPluginStatus(pub u32);

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
pub struct CapabilityDescriptorV9 {
    pub name: Utf8Slice,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionTransferBlobV9 {
    pub bytes: ByteSlice,
}
