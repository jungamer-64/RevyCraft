use revy_voxel_model::{
    BlockPos, BlockState, ChunkColumn, ChunkPos, InventoryClickButton, ItemStack, PlayerInventory,
    WorldMeta,
};
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerBinding {
    Virtual,
    Block {
        position: BlockPos,
        block_entity_kind: BlockEntityKindId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenContainerState {
    pub kind: ContainerKindId,
    pub binding: ContainerBinding,
    pub local_slots: Vec<Option<ItemStack>>,
    #[serde(default)]
    pub properties: BTreeMap<ContainerPropertyKey, i16>,
}

impl OpenContainerState {
    #[must_use]
    pub fn world_position(&self) -> Option<BlockPos> {
        match self.binding {
            ContainerBinding::Virtual => None,
            ContainerBinding::Block { position, .. } => Some(position),
        }
    }

    #[must_use]
    pub fn block_entity_kind(&self) -> Option<&BlockEntityKindId> {
        match &self.binding {
            ContainerBinding::Virtual => None,
            ContainerBinding::Block {
                block_entity_kind, ..
            } => Some(block_entity_kind),
        }
    }

    #[must_use]
    pub fn block_entity_state(&self) -> Option<ContainerBlockEntityState> {
        Some(ContainerBlockEntityState {
            kind: self.block_entity_kind()?.clone(),
            slots: self.local_slots.clone(),
            properties: self.properties.clone(),
        })
    }

    pub fn local_slot_mut(&mut self, index: u16) -> Option<&mut Option<ItemStack>> {
        self.local_slots.get_mut(usize::from(index))
    }

    #[must_use]
    pub fn property_entries(&self) -> Vec<(ContainerPropertyKey, i16)> {
        self.properties
            .iter()
            .map(|(property, value)| (property.clone(), *value))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerBlockEntityState {
    pub kind: BlockEntityKindId,
    pub slots: Vec<Option<ItemStack>>,
    #[serde(default)]
    pub properties: BTreeMap<ContainerPropertyKey, i16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockEntityState {
    Container(ContainerBlockEntityState),
}

impl BlockEntityState {
    #[must_use]
    pub fn container(
        kind: impl Into<BlockEntityKindId>,
        slots: Vec<Option<ItemStack>>,
        properties: BTreeMap<ContainerPropertyKey, i16>,
    ) -> Self {
        Self::Container(ContainerBlockEntityState {
            kind: kind.into(),
            slots,
            properties,
        })
    }

    #[must_use]
    pub fn container_state(&self) -> Option<&ContainerBlockEntityState> {
        match self {
            Self::Container(container) => Some(container),
        }
    }

    #[must_use]
    pub fn container_state_mut(&mut self) -> Option<&mut ContainerBlockEntityState> {
        match self {
            Self::Container(container) => Some(container),
        }
    }

    #[must_use]
    pub fn has_inventory_contents(&self) -> bool {
        match self {
            Self::Container(container) => container.slots.iter().any(Option::is_some),
        }
    }
}

pub trait ContentBehavior: std::fmt::Debug + Send + Sync + 'static {
    fn generate_chunk(&self, meta: &WorldMeta, chunk_pos: ChunkPos) -> ChunkColumn;

    fn player_container_kind(&self) -> ContainerKindId;

    fn container_spec(&self, kind: &ContainerKindId) -> Option<ContainerSpec>;

    fn container_title(&self, kind: &ContainerKindId) -> String;

    fn container_kind_for_block(&self, block: &BlockState) -> Option<ContainerKindId>;

    fn default_block_entity_for_block(
        &self,
        block: &BlockState,
    ) -> Option<ContainerBlockEntityState>;

    fn default_block_entity_for_kind(
        &self,
        kind: &BlockEntityKindId,
    ) -> Option<ContainerBlockEntityState>;

    fn block_entity_kind_for_container(&self, kind: &ContainerKindId) -> Option<BlockEntityKindId>;

    fn container_kind_for_block_entity(
        &self,
        entity: &ContainerBlockEntityState,
    ) -> Option<ContainerKindId>;

    fn is_air_block(&self, block: &BlockState) -> bool;

    fn is_unbreakable_block(&self, block: &BlockState) -> bool;

    fn placeable_block_state_from_item_key(&self, key: &str) -> Option<BlockState>;

    fn is_supported_inventory_item(&self, key: &str) -> bool;

    fn starter_inventory(&self) -> PlayerInventory;

    fn tool_spec_for_item(&self, item: Option<&ItemStack>) -> Option<MiningToolSpec>;

    fn survival_mining_duration_ms(
        &self,
        block: &BlockState,
        tool: Option<MiningToolSpec>,
    ) -> Option<u64>;

    fn survival_drop_for_block(&self, block: &BlockState) -> Option<ItemStack>;

    fn normalize_container(&self, state: &mut OpenContainerState);

    fn normalize_player_inventory(&self, inventory: &mut PlayerInventory);

    fn try_take_output(
        &self,
        kind: &ContainerKindId,
        local_slots: &mut Vec<Option<ItemStack>>,
        cursor: &mut Option<ItemStack>,
        button: InventoryClickButton,
    ) -> bool;

    fn tick_container(&self, state: &mut OpenContainerState);
}
