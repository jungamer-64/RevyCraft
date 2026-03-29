use crate::inventory::{InventoryWindowContents, ItemStack, PlayerInventory};
use crate::world::{BlockPos, ContainerBlockEntityState};
use mc_content_api::{BlockEntityKindId, ContainerKindId, ContainerPropertyKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

    pub(super) fn local_slot_mut(&mut self, index: u16) -> Option<&mut Option<ItemStack>> {
        self.local_slots.get_mut(usize::from(index))
    }

    #[must_use]
    pub(crate) fn property_entries(&self) -> Vec<(ContainerPropertyKey, i16)> {
        self.properties
            .iter()
            .map(|(property, value)| (property.clone(), *value))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenInventoryWindow {
    pub window_id: u8,
    pub container: OpenContainerState,
}

impl OpenInventoryWindow {
    pub(crate) fn contents(&self, player_inventory: &PlayerInventory) -> InventoryWindowContents {
        InventoryWindowContents::with_local_slots(
            player_inventory.clone(),
            self.container.local_slots.clone(),
        )
    }

    pub(super) fn local_slot_mut(&mut self, index: u16) -> Option<&mut Option<ItemStack>> {
        self.container.local_slot_mut(index)
    }

    pub(crate) fn property_entries(&self) -> Vec<(ContainerPropertyKey, i16)> {
        self.container.property_entries()
    }

    #[must_use]
    pub(super) fn world_position(&self) -> Option<BlockPos> {
        self.container.world_position()
    }

    #[must_use]
    pub(super) fn world_block_entity(&self) -> Option<(BlockPos, ContainerBlockEntityState)> {
        Some((self.world_position()?, self.container.block_entity_state()?))
    }
}
