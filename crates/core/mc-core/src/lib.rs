#![allow(clippy::multiple_crate_versions)]
pub mod catalog;
pub mod inventory;

pub(crate) mod core;
pub(crate) mod events;
pub(crate) mod player;
#[cfg(test)]
mod tests;
pub(crate) mod world;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;
use uuid::Uuid;

pub use self::core::transaction::{
    GameplayJournal, GameplayJournalApplyResult, GameplayTransaction,
};
pub use self::core::{
    ActiveMiningState, ChestWindowBinding, ChestWindowState, ClientView, ContainerDescriptor,
    CoreConfig, CoreRuntimeStateBlob, DroppedItemState, FurnaceWindowBinding, FurnaceWindowState,
    OnlinePlayerRuntimeState, OpenInventoryWindow, OpenInventoryWindowState, PlayerSessionState,
    ServerCore,
};
pub use self::events::{
    CoreCommand, CoreEvent, EventTarget, GameplayCommand, InventoryClickButton,
    InventoryClickTarget, InventoryClickValidation, InventoryTransactionContext, PlayerSummary,
    RuntimeCommand, SessionCommand, TargetedEvent,
};
pub use self::inventory::{
    InventoryContainer, InventorySlot, InventoryWindowContents, ItemKey, ItemStack, PlayerInventory,
};
pub use self::player::{InteractionHand, PlayerSnapshot};
pub use self::world::{
    BlockEntityState, BlockFace, BlockKey, BlockPos, BlockState, ChunkColumn, ChunkDelta, ChunkPos,
    ChunkSection, DimensionId, DroppedItemSnapshot, SectionBlockIndex, SectionPos, Vec3, WorldMeta,
    WorldSnapshot, expand_block_index,
};

#[cfg(test)]
pub(crate) use self::world::flatten_block_index;

const CHUNK_WIDTH: i32 = 16;
const SECTION_HEIGHT: i32 = 16;
const DEFAULT_KEEPALIVE_INTERVAL_MS: u64 = 10_000;
const DEFAULT_KEEPALIVE_TIMEOUT_MS: u64 = 30_000;
const PLAYER_INVENTORY_SLOT_COUNT: usize = 45;
const AUXILIARY_SLOT_COUNT: u8 = 9;
const MAIN_INVENTORY_SLOT_COUNT: u8 = 27;
const HOTBAR_START_SLOT: u8 = 36;
const HOTBAR_SLOT_COUNT: u8 = 9;
const PLAYER_WIDTH: f64 = 0.6;
const PLAYER_HEIGHT: f64 = 1.8;
const BLOCK_EDIT_REACH: f64 = 6.0;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(
            Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.as_str() == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.as_str() == *other
            }
        }

        impl PartialEq<$name> for str {
            fn eq(&self, other: &$name) -> bool {
                self == other.as_str()
            }
        }

        impl PartialEq<$name> for &str {
            fn eq(&self, other: &$name) -> bool {
                *self == other.as_str()
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                self.as_str() == other
            }
        }

        impl PartialEq<$name> for String {
            fn eq(&self, other: &$name) -> bool {
                self == other.as_str()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConnectionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId(pub i32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlayerId(pub Uuid);

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PluginGenerationId(pub u64);

string_id!(AdapterId);
string_id!(AdminSurfaceProfileId);
string_id!(AuthProfileId);
string_id!(GameplayProfileId);
string_id!(PluginBuildTag);
string_id!(StorageProfileId);

impl Serialize for PlayerId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.hyphenated().to_string())
    }
}

impl<'de> Deserialize<'de> for PlayerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let uuid = Uuid::parse_str(&value).map_err(serde::de::Error::custom)?;
        Ok(Self(uuid))
    }
}

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

    /// Parses a capability from a string value.
    ///
    /// # Errors
    ///
    /// Returns a [`CapabilityParseError`] if the value does not correspond to a valid capability.
    fn parse(value: &str) -> Result<Self, CapabilityParseError>
    where
        Self: Sized;
}

macro_rules! closed_capability_enum {
    ($name:ident, $kind:literal, { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
        )]
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
