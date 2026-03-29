use crate::{EntityId, GameplayProfileId, PluginBuildTag, PluginGenerationId};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityParseError {
    capability_kind: &'static str,
    value: String,
}

impl CapabilityParseError {
    #[must_use]
    pub fn new(capability_kind: &'static str, value: impl Into<String>) -> Self {
        Self {
            capability_kind,
            value: value.into(),
        }
    }

    #[must_use]
    pub const fn capability_kind(&self) -> &'static str {
        self.capability_kind
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Display for CapabilityParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown {} capability `{}`",
            self.capability_kind, self.value
        )
    }
}

impl std::error::Error for CapabilityParseError {}

pub trait ClosedCapability: Copy + Ord + Eq + std::fmt::Debug + Send + Sync + 'static {
    fn capability_kind() -> &'static str
    where
        Self: Sized;

    fn as_str(self) -> &'static str;

    fn parse(value: &str) -> Result<Self, CapabilityParseError>
    where
        Self: Sized;
}

macro_rules! closed_capability_enum {
    ($name:ident, $kind:literal, { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }
        }

        impl ClosedCapability for $name {
            fn capability_kind() -> &'static str {
                $kind
            }

            fn as_str(self) -> &'static str {
                self.as_str()
            }

            fn parse(value: &str) -> Result<Self, CapabilityParseError> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err(CapabilityParseError::new($kind, value)),
                }
            }
        }

        impl TryFrom<&str> for $name {
            type Error = CapabilityParseError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                <Self as ClosedCapability>::parse(value)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(&value).map_err(serde::de::Error::custom)
            }
        }
    };
}

closed_capability_enum!(ProtocolCapability, "protocol", {
    RuntimeReload => "runtime.reload.protocol",
    Je => "protocol.je",
    Je5 => "protocol.je.5",
    Je47 => "protocol.je.47",
    Je340 => "protocol.je.340",
    Je404 => "protocol.je.404",
    Bedrock => "protocol.bedrock",
    Bedrock924 => "protocol.bedrock.924",
});

closed_capability_enum!(GameplayCapability, "gameplay", {
    RuntimeReload => "runtime.reload.gameplay",
});

closed_capability_enum!(StorageCapability, "storage", {
    RuntimeReload => "runtime.reload.storage",
});

closed_capability_enum!(AuthCapability, "auth", {
    RuntimeReload => "runtime.reload.auth",
});

closed_capability_enum!(AdminSurfaceCapability, "admin-surface", {
    RuntimeReload => "runtime.reload.admin-surface",
});

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(
    serialize = "C: Ord + Serialize",
    deserialize = "C: Ord + Deserialize<'de>"
))]
pub struct ClosedCapabilitySet<C> {
    capabilities: BTreeSet<C>,
}

impl<C> Default for ClosedCapabilitySet<C> {
    fn default() -> Self {
        Self {
            capabilities: BTreeSet::new(),
        }
    }
}

impl<C> ClosedCapabilitySet<C> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.capabilities.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }
}

impl<C> ClosedCapabilitySet<C>
where
    C: Ord,
{
    pub fn insert(&mut self, capability: C) -> bool {
        self.capabilities.insert(capability)
    }

    #[must_use]
    pub fn contains(&self, capability: &C) -> bool {
        self.capabilities.contains(capability)
    }
}

impl<C> ClosedCapabilitySet<C>
where
    C: Copy + Ord,
{
    pub fn iter(&self) -> impl Iterator<Item = C> + '_ {
        self.capabilities.iter().copied()
    }
}

impl<C> std::iter::FromIterator<C> for ClosedCapabilitySet<C>
where
    C: Ord,
{
    fn from_iter<T: IntoIterator<Item = C>>(iter: T) -> Self {
        let mut capabilities = Self::new();
        capabilities.extend(iter);
        capabilities
    }
}

impl<C> Extend<C> for ClosedCapabilitySet<C>
where
    C: Ord,
{
    fn extend<T: IntoIterator<Item = C>>(&mut self, iter: T) {
        self.capabilities.extend(iter);
    }
}

pub type ProtocolCapabilitySet = ClosedCapabilitySet<ProtocolCapability>;
pub type GameplayCapabilitySet = ClosedCapabilitySet<GameplayCapability>;
pub type StorageCapabilitySet = ClosedCapabilitySet<StorageCapability>;
pub type AuthCapabilitySet = ClosedCapabilitySet<AuthCapability>;
pub type AdminSurfaceCapabilitySet = ClosedCapabilitySet<AdminSurfaceCapability>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(
    serialize = "C: Ord + Serialize",
    deserialize = "C: Ord + Deserialize<'de>"
))]
pub struct CapabilityAnnouncement<C> {
    pub capabilities: ClosedCapabilitySet<C>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_tag: Option<PluginBuildTag>,
}

impl<C> CapabilityAnnouncement<C> {
    #[must_use]
    pub const fn new(capabilities: ClosedCapabilitySet<C>) -> Self {
        Self {
            capabilities,
            build_tag: None,
        }
    }

    #[must_use]
    pub fn with_build_tag(mut self, build_tag: Option<PluginBuildTag>) -> Self {
        self.build_tag = build_tag;
        self
    }
}

impl<C> CapabilityAnnouncement<C>
where
    C: Copy + Ord,
{
    #[must_use]
    pub fn contains(&self, capability: C) -> bool {
        self.capabilities.contains(&capability)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCapabilitySet {
    pub protocol: ProtocolCapabilitySet,
    pub gameplay: GameplayCapabilitySet,
    pub gameplay_profile: GameplayProfileId,
    pub entity_id: Option<EntityId>,
    pub protocol_generation: Option<PluginGenerationId>,
    pub gameplay_generation: Option<PluginGenerationId>,
}
