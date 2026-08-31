use serde::{Deserialize, Serialize};
use std::fmt;

pub const CURRENT_PLUGIN_ABI: PluginAbiVersion = PluginAbiVersion { major: 9, minor: 0 };

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PluginKind {
    Protocol = 1,
    Storage = 2,
    Auth = 3,
    Gameplay = 4,
    AdminSurface = 7,
}

impl TryFrom<u8> for PluginKind {
    type Error = InvalidPluginKind;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Protocol),
            2 => Ok(Self::Storage),
            3 => Ok(Self::Auth),
            4 => Ok(Self::Gameplay),
            7 => Ok(Self::AdminSurface),
            _ => Err(InvalidPluginKind(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown plugin kind tag {0}")]
pub struct InvalidPluginKind(pub u8);
