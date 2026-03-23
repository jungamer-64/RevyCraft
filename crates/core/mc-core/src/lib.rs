#![allow(clippy::multiple_crate_versions)]
pub mod catalog;

pub(crate) mod core;
pub(crate) mod events;
pub(crate) mod gameplay;
pub(crate) mod player;
#[cfg(test)]
mod tests;
pub(crate) mod world;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;
use uuid::Uuid;

pub use self::core::{ClientView, CoreConfig, ServerCore};
pub use self::events::{
    CoreCommand, CoreEvent, EventTarget, InventoryClickButton, InventoryClickTarget,
    InventoryTransactionContext, PlayerSummary, TargetedEvent,
};
pub use self::gameplay::{
    CanonicalGameplayPolicy, GameplayEffect, GameplayJoinEffect, GameplayMutation,
    GameplayPolicyResolver, GameplayQuery, ReadonlyGameplayPolicy,
};
pub use self::player::{
    InteractionHand, InventoryContainer, InventorySlot, ItemKey, ItemStack, PlayerInventory,
    PlayerSnapshot,
};
pub use self::world::{
    BlockFace, BlockKey, BlockPos, BlockState, ChunkColumn, ChunkDelta, ChunkPos, ChunkSection,
    DimensionId, SectionBlockIndex, SectionPos, Vec3, WorldMeta, WorldSnapshot, expand_block_index,
};

#[cfg(test)]
pub(crate) use self::gameplay::canonical_session_capabilities;
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
string_id!(AdminUiProfileId);
string_id!(AuthProfileId);
string_id!(GameplayProfileId);
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySet {
    capabilities: BTreeSet<String>,
}

impl CapabilitySet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, capability: impl Into<String>) -> bool {
        self.capabilities.insert(capability.into())
    }

    #[must_use]
    pub fn contains(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.capabilities.iter().map(String::as_str)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCapabilitySet {
    pub protocol: CapabilitySet,
    pub gameplay: CapabilitySet,
    pub gameplay_profile: GameplayProfileId,
    pub entity_id: Option<EntityId>,
    pub protocol_generation: Option<PluginGenerationId>,
    pub gameplay_generation: Option<PluginGenerationId>,
}
