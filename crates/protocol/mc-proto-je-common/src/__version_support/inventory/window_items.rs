use super::layout::{PlayerInventoryLayout, container_descriptor};
use mc_core::{InventoryContainer, InventorySlot, InventoryWindowContents, ItemStack};

#[must_use]
pub fn window_items(
    container: InventoryContainer,
    layout: PlayerInventoryLayout,
    contents: &InventoryWindowContents,
) -> Vec<Option<ItemStack>> {
    match container {
        InventoryContainer::Player => match layout {
            PlayerInventoryLayout::Legacy => contents.player_inventory.slots.clone(),
            PlayerInventoryLayout::ModernWithOffhand => {
                let mut items = contents.player_inventory.slots.clone();
                items.push(contents.player_inventory.offhand.clone());
                items
            }
        },
        InventoryContainer::CraftingTable
        | InventoryContainer::Chest
        | InventoryContainer::Furnace => {
            let descriptor = container_descriptor(container);
            let mut items = contents.container_slots.clone();
            debug_assert_eq!(items.len(), usize::from(descriptor.local_slot_count));
            items.extend(
                (0_u8..27)
                    .map(InventorySlot::MainInventory)
                    .map(|slot| contents.player_inventory.get_slot(slot).cloned()),
            );
            items.extend(
                (0_u8..9)
                    .map(InventorySlot::Hotbar)
                    .map(|slot| contents.player_inventory.get_slot(slot).cloned()),
            );
            items
        }
    }
}
