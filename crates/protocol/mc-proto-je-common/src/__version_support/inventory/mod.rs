mod layout;
mod slot_codec;
mod window_items;

pub use self::layout::{
    CHEST_WINDOW_TYPE, CRAFTING_TABLE_WINDOW_TYPE, CURSOR_SLOT_ID, CURSOR_WINDOW_ID,
    FURNACE_WINDOW_TYPE, InventoryProtocolSpec, JE_1_7_10_INVENTORY_SPEC, JE_1_8_X_INVENTORY_SPEC,
    JE_1_12_2_INVENTORY_SPEC, JE_1_13_2_INVENTORY_SPEC, PLAYER_WINDOW_CRAFTING_INPUT_SLOTS,
    PLAYER_WINDOW_CRAFTING_RESULT_SLOT, PlayerInventoryLayout, inventory_slot, player_window_id,
    protocol_slot, signed_window_id, unique_slot_count, window_type,
};
pub use self::slot_codec::{SlotEncoding, SlotNbtEncoding, read_slot, write_slot};
pub use self::window_items::window_items;

#[cfg(test)]
mod tests {
    use super::*;
    use mc_core::{
        ContainerKindId, InventorySlot, InventoryWindowContents, ItemStack, PlayerInventory,
    };
    use mc_proto_common::{PacketReader, PacketWriter};

    fn player_container() -> ContainerKindId {
        ContainerKindId::new("canonical:player")
    }

    fn chest_container() -> ContainerKindId {
        ContainerKindId::new("canonical:chest_27")
    }

    fn furnace_container() -> ContainerKindId {
        ContainerKindId::new("canonical:furnace")
    }

    fn stone_stack() -> ItemStack {
        ItemStack::new("minecraft:stone", 32, 0)
    }

    #[test]
    fn length_prefixed_slot_round_trips_with_empty_sentinel() {
        let mut writer = PacketWriter::default();
        write_slot(
            &mut writer,
            Some(&stone_stack()),
            JE_1_7_10_INVENTORY_SPEC.slot,
        )
        .expect("legacy slot should encode");

        let encoded = writer.into_inner();
        let mut reader = PacketReader::new(&encoded);
        assert_eq!(reader.read_i16().expect("item id should decode"), 1);
        assert_eq!(reader.read_u8().expect("count should decode"), 32);
        assert_eq!(reader.read_i16().expect("damage should decode"), 0);
        assert_eq!(reader.read_i16().expect("nbt sentinel should decode"), -1);

        let mut reader = PacketReader::new(&encoded);
        assert_eq!(
            read_slot(&mut reader, JE_1_7_10_INVENTORY_SPEC.slot)
                .expect("legacy slot should decode"),
            Some(stone_stack())
        );
    }

    #[test]
    fn root_tag_slot_round_trips_with_end_marker() {
        let mut writer = PacketWriter::default();
        write_slot(
            &mut writer,
            Some(&stone_stack()),
            JE_1_8_X_INVENTORY_SPEC.slot,
        )
        .expect("root-tag slot should encode");

        let encoded = writer.into_inner();
        let mut reader = PacketReader::new(&encoded);
        assert_eq!(reader.read_i16().expect("item id should decode"), 1);
        assert_eq!(reader.read_u8().expect("count should decode"), 32);
        assert_eq!(reader.read_i16().expect("damage should decode"), 0);
        assert_eq!(reader.read_u8().expect("nbt tag should decode"), 0);

        let mut reader = PacketReader::new(&encoded);
        assert_eq!(
            read_slot(&mut reader, JE_1_8_X_INVENTORY_SPEC.slot)
                .expect("root-tag slot should decode"),
            Some(stone_stack())
        );
    }

    #[test]
    fn present_varint_slot_round_trips_with_root_tag_marker() {
        let mut writer = PacketWriter::default();
        write_slot(
            &mut writer,
            Some(&stone_stack()),
            JE_1_13_2_INVENTORY_SPEC.slot,
        )
        .expect("1.13.2 slot should encode");

        let encoded = writer.into_inner();
        let mut reader = PacketReader::new(&encoded);
        assert!(reader.read_bool().expect("present flag should decode"));
        assert_eq!(reader.read_varint().expect("item id should decode"), 1);
        assert_eq!(reader.read_u8().expect("count should decode"), 32);
        assert_eq!(reader.read_u8().expect("nbt tag should decode"), 0);

        let mut reader = PacketReader::new(&encoded);
        assert_eq!(
            read_slot(&mut reader, JE_1_13_2_INVENTORY_SPEC.slot)
                .expect("1.13.2 slot should decode"),
            Some(stone_stack())
        );
    }

    #[test]
    fn modern_layout_exposes_offhand_slot_without_affecting_legacy_slots() {
        let mut inventory = mc_content_canonical::creative_starter_inventory();
        inventory.offhand = Some(ItemStack::new("minecraft:brick_block", 1, 0));
        let player_contents = InventoryWindowContents::player(inventory.clone());

        let legacy_items = window_items(
            &player_container(),
            PlayerInventoryLayout::Legacy,
            &player_contents,
        );
        let modern_items = window_items(
            &player_container(),
            PlayerInventoryLayout::ModernWithOffhand,
            &player_contents,
        );

        assert_eq!(legacy_items.len(), inventory.slots.len());
        assert_eq!(modern_items.len(), inventory.slots.len() + 1);
        assert_eq!(
            protocol_slot(
                &player_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::Offhand,
            ),
            None
        );
        assert_eq!(
            protocol_slot(
                &player_container(),
                PlayerInventoryLayout::ModernWithOffhand,
                InventorySlot::Offhand
            ),
            Some(45)
        );
        assert_eq!(
            inventory_slot(
                &player_container(),
                PlayerInventoryLayout::ModernWithOffhand,
                45,
            ),
            Some(InventorySlot::Offhand)
        );
        assert_eq!(
            inventory_slot(&player_container(), PlayerInventoryLayout::Legacy, 45),
            None
        );
        assert_eq!(
            modern_items.last().expect("offhand slot should exist"),
            &inventory.offhand
        );
    }

    #[test]
    fn furnace_slot_mapping_uses_container_descriptor_layout() {
        assert_eq!(unique_slot_count(&furnace_container()), 3);
        assert_eq!(window_type(&furnace_container()), FURNACE_WINDOW_TYPE);

        assert_eq!(
            protocol_slot(
                &furnace_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::WindowLocal(0),
            ),
            Some(0)
        );
        assert_eq!(
            protocol_slot(
                &furnace_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::WindowLocal(2),
            ),
            Some(2)
        );
        assert_eq!(
            protocol_slot(
                &furnace_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::MainInventory(0),
            ),
            Some(3)
        );
        assert_eq!(
            protocol_slot(
                &furnace_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::MainInventory(26),
            ),
            Some(29)
        );
        assert_eq!(
            protocol_slot(
                &furnace_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::Hotbar(0),
            ),
            Some(30)
        );
        assert_eq!(
            protocol_slot(
                &furnace_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::Hotbar(8),
            ),
            Some(38)
        );

        assert_eq!(
            inventory_slot(&furnace_container(), PlayerInventoryLayout::Legacy, 0),
            Some(InventorySlot::WindowLocal(0))
        );
        assert_eq!(
            inventory_slot(&furnace_container(), PlayerInventoryLayout::Legacy, 2),
            Some(InventorySlot::WindowLocal(2))
        );
        assert_eq!(
            inventory_slot(&furnace_container(), PlayerInventoryLayout::Legacy, 3),
            Some(InventorySlot::MainInventory(0))
        );
        assert_eq!(
            inventory_slot(&furnace_container(), PlayerInventoryLayout::Legacy, 29),
            Some(InventorySlot::MainInventory(26))
        );
        assert_eq!(
            inventory_slot(&furnace_container(), PlayerInventoryLayout::Legacy, 30),
            Some(InventorySlot::Hotbar(0))
        );
        assert_eq!(
            inventory_slot(&furnace_container(), PlayerInventoryLayout::Legacy, 38),
            Some(InventorySlot::Hotbar(8))
        );
        assert_eq!(
            inventory_slot(&furnace_container(), PlayerInventoryLayout::Legacy, 39),
            None
        );
    }

    #[test]
    fn chest_slot_mapping_uses_container_descriptor_layout() {
        assert_eq!(unique_slot_count(&chest_container()), 27);
        assert_eq!(window_type(&chest_container()), CHEST_WINDOW_TYPE);

        assert_eq!(
            protocol_slot(
                &chest_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::WindowLocal(0),
            ),
            Some(0)
        );
        assert_eq!(
            protocol_slot(
                &chest_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::WindowLocal(26),
            ),
            Some(26)
        );
        assert_eq!(
            protocol_slot(
                &chest_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::MainInventory(0),
            ),
            Some(27)
        );
        assert_eq!(
            protocol_slot(
                &chest_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::MainInventory(26),
            ),
            Some(53)
        );
        assert_eq!(
            protocol_slot(
                &chest_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::Hotbar(0),
            ),
            Some(54)
        );
        assert_eq!(
            protocol_slot(
                &chest_container(),
                PlayerInventoryLayout::Legacy,
                InventorySlot::Hotbar(8),
            ),
            Some(62)
        );

        assert_eq!(
            inventory_slot(&chest_container(), PlayerInventoryLayout::Legacy, 0),
            Some(InventorySlot::WindowLocal(0))
        );
        assert_eq!(
            inventory_slot(&chest_container(), PlayerInventoryLayout::Legacy, 26),
            Some(InventorySlot::WindowLocal(26))
        );
        assert_eq!(
            inventory_slot(&chest_container(), PlayerInventoryLayout::Legacy, 27),
            Some(InventorySlot::MainInventory(0))
        );
        assert_eq!(
            inventory_slot(&chest_container(), PlayerInventoryLayout::Legacy, 53),
            Some(InventorySlot::MainInventory(26))
        );
        assert_eq!(
            inventory_slot(&chest_container(), PlayerInventoryLayout::Legacy, 54),
            Some(InventorySlot::Hotbar(0))
        );
        assert_eq!(
            inventory_slot(&chest_container(), PlayerInventoryLayout::Legacy, 62),
            Some(InventorySlot::Hotbar(8))
        );
        assert_eq!(
            inventory_slot(&chest_container(), PlayerInventoryLayout::Legacy, 63),
            None
        );
    }

    #[test]
    fn furnace_window_items_prefix_container_slots_before_player_inventory() {
        let mut inventory = PlayerInventory::new_empty();
        let _ = inventory.set_slot(
            InventorySlot::MainInventory(0),
            Some(ItemStack::new("minecraft:cobblestone", 12, 0)),
        );
        let _ = inventory.set_slot(
            InventorySlot::Hotbar(0),
            Some(ItemStack::new("minecraft:stick", 16, 0)),
        );
        let contents = InventoryWindowContents::with_container(
            inventory,
            vec![
                Some(ItemStack::new("minecraft:sand", 1, 0)),
                Some(ItemStack::new("minecraft:oak_planks", 1, 0)),
                Some(ItemStack::new("minecraft:glass", 1, 0)),
            ],
        );

        let items = window_items(
            &furnace_container(),
            PlayerInventoryLayout::Legacy,
            &contents,
        );

        assert_eq!(items.len(), 39);
        assert_eq!(items[0], Some(ItemStack::new("minecraft:sand", 1, 0)));
        assert_eq!(items[1], Some(ItemStack::new("minecraft:oak_planks", 1, 0)));
        assert_eq!(items[2], Some(ItemStack::new("minecraft:glass", 1, 0)));
        assert_eq!(
            items[3],
            Some(ItemStack::new("minecraft:cobblestone", 12, 0))
        );
        assert_eq!(items[30], Some(ItemStack::new("minecraft:stick", 16, 0)));
    }

    #[test]
    fn chest_window_items_prefix_container_slots_before_player_inventory() {
        let mut inventory = PlayerInventory::new_empty();
        let _ = inventory.set_slot(
            InventorySlot::MainInventory(0),
            Some(ItemStack::new("minecraft:cobblestone", 12, 0)),
        );
        let _ = inventory.set_slot(
            InventorySlot::Hotbar(0),
            Some(ItemStack::new("minecraft:stick", 16, 0)),
        );
        let mut container_slots = vec![None; 27];
        container_slots[0] = Some(ItemStack::new("minecraft:sand", 1, 0));
        container_slots[26] = Some(ItemStack::new("minecraft:glass", 1, 0));
        let contents = InventoryWindowContents::with_container(inventory, container_slots);

        let items = window_items(&chest_container(), PlayerInventoryLayout::Legacy, &contents);

        assert_eq!(items.len(), 63);
        assert_eq!(items[0], Some(ItemStack::new("minecraft:sand", 1, 0)));
        assert_eq!(items[26], Some(ItemStack::new("minecraft:glass", 1, 0)));
        assert_eq!(
            items[27],
            Some(ItemStack::new("minecraft:cobblestone", 12, 0))
        );
        assert_eq!(items[54], Some(ItemStack::new("minecraft:stick", 16, 0)));
    }
}
