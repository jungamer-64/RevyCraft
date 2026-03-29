use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

string_id!(BlockEntityKindId);
string_id!(ContainerKindId);
string_id!(ContainerPropertyKey);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolClass {
    Pickaxe,
    Shovel,
    Axe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MiningToolSpec {
    pub class: ToolClass,
    pub tier: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockDescriptor {
    pub key: String,
    pub block_entity_kind: Option<BlockEntityKindId>,
    pub open_container: Option<ContainerKindId>,
    pub is_air: bool,
    pub is_unbreakable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDescriptor {
    pub key: String,
    pub placeable_block: Option<String>,
    pub max_stack_size: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerSlotRole {
    Generic,
    OutputOnly,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerSpec {
    pub local_slot_count: u16,
    #[serde(default)]
    pub slot_roles: BTreeMap<u16, ContainerSlotRole>,
}

impl ContainerSpec {
    #[must_use]
    pub fn slot_role(&self, index: u16) -> ContainerSlotRole {
        self.slot_roles
            .get(&index)
            .copied()
            .unwrap_or(ContainerSlotRole::Generic)
    }
}
