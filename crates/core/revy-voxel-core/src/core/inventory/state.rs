use crate::inventory::{InventoryWindowContents, ItemStack, PlayerInventory};
use revy_voxel_model::BlockPos;
use revy_voxel_rules::{ContainerBlockEntityState, ContainerPropertyKey, OpenContainerState};
use serde::{Deserialize, Serialize};

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
