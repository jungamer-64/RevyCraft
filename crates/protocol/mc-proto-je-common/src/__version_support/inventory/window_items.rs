use super::layout::{PlayerInventoryLayout, container_descriptor, is_player_container};
use revy_voxel_model::{InventorySlot, InventoryWindowContents, ItemStack};
use revy_voxel_rules::ContainerKindId;

#[must_use]
pub fn window_items(
    container: &ContainerKindId,
    layout: PlayerInventoryLayout,
    contents: &InventoryWindowContents,
) -> Vec<Option<ItemStack>> {
    if is_player_container(container) {
        match layout {
            PlayerInventoryLayout::Legacy => contents.player_inventory.slots.clone(),
            PlayerInventoryLayout::ModernWithOffhand => {
                let mut items = contents.player_inventory.slots.clone();
                items.push(contents.player_inventory.offhand.clone());
                items
            }
        }
    } else {
        let descriptor = container_descriptor(container);
        let mut items = contents.local_slots.clone();
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
