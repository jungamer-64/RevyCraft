use mc_content_canonical::catalog;
use revy_voxel_model::{BlockState, ItemStack};

fn block(key: &str) -> BlockState {
    BlockState::new(key)
}

#[must_use]
pub fn legacy_block(state: &BlockState) -> (u16, u8) {
    match state.key.as_str() {
        catalog::STONE => (1, 0),
        catalog::GRASS_BLOCK => (2, 0),
        catalog::DIRT => (3, 0),
        catalog::COBBLESTONE => (4, 0),
        catalog::OAK_PLANKS => (5, 0),
        catalog::BEDROCK => (7, 0),
        catalog::SAND => (12, 0),
        catalog::GLASS => (20, 0),
        catalog::SANDSTONE => (24, 0),
        catalog::BRICKS => (45, 0),
        catalog::CRAFTING_TABLE => (58, 0),
        catalog::CHEST => (54, 0),
        catalog::FURNACE => (61, 0),
        _ => (0, 0),
    }
}

#[must_use]
pub fn legacy_block_state_id(state: &BlockState) -> i32 {
    let (block_id, metadata) = legacy_block(state);
    (i32::from(block_id) << 4) | i32::from(metadata)
}

#[must_use]
pub fn flattened_block_state_id_1_13_2(state: &BlockState) -> i32 {
    match state.key.as_str() {
        catalog::STONE => 1,
        catalog::GRASS_BLOCK => 8,
        catalog::DIRT => 10,
        catalog::COBBLESTONE => 14,
        catalog::OAK_PLANKS => 15,
        catalog::BEDROCK => 33,
        catalog::SAND => 66,
        catalog::GLASS => 230,
        catalog::SANDSTONE => 245,
        catalog::BRICKS => 1125,
        catalog::CHEST => 1729,
        catalog::CRAFTING_TABLE => 3051,
        catalog::FURNACE => 3068,
        _ => 0,
    }
}

#[must_use]
pub fn semantic_block(block_id: u16, metadata: u8) -> BlockState {
    match block_id {
        1 => block(catalog::STONE),
        2 => block(catalog::GRASS_BLOCK),
        3 => block(catalog::DIRT),
        4 => block(catalog::COBBLESTONE),
        5 if metadata == 0 => block(catalog::OAK_PLANKS),
        7 => block(catalog::BEDROCK),
        12 if metadata == 0 => block(catalog::SAND),
        20 => block(catalog::GLASS),
        24 if metadata == 0 => block(catalog::SANDSTONE),
        45 => block(catalog::BRICKS),
        58 => block(catalog::CRAFTING_TABLE),
        54 => block(catalog::CHEST),
        61 => block(catalog::FURNACE),
        _ => block(catalog::AIR),
    }
}

#[must_use]
pub fn semantic_flattened_block_1_13_2(state_id: i32) -> BlockState {
    match state_id {
        1 => block(catalog::STONE),
        8 | 9 => block(catalog::GRASS_BLOCK),
        10 => block(catalog::DIRT),
        14 => block(catalog::COBBLESTONE),
        15 => block(catalog::OAK_PLANKS),
        33 => block(catalog::BEDROCK),
        66 => block(catalog::SAND),
        230 => block(catalog::GLASS),
        245 => block(catalog::SANDSTONE),
        1125 => block(catalog::BRICKS),
        1729..=1752 => block(catalog::CHEST),
        3051 => block(catalog::CRAFTING_TABLE),
        3068..=3075 => block(catalog::FURNACE),
        _ => block(catalog::AIR),
    }
}

#[must_use]
pub fn legacy_item(stack: &ItemStack) -> Option<(i16, u16)> {
    let damage = stack.damage;
    match stack.key.as_str() {
        catalog::STONE => Some((1, damage)),
        catalog::GRASS_BLOCK => Some((2, damage)),
        catalog::DIRT => Some((3, damage)),
        catalog::COBBLESTONE => Some((4, damage)),
        catalog::OAK_PLANKS => Some((5, damage)),
        catalog::OAK_LOG => Some((17, damage)),
        catalog::SAND => Some((12, damage)),
        catalog::GLASS => Some((20, damage)),
        catalog::SANDSTONE => Some((24, damage)),
        catalog::BRICKS => Some((45, damage)),
        catalog::CRAFTING_TABLE => Some((58, damage)),
        catalog::CHEST => Some((54, damage)),
        catalog::FURNACE => Some((61, damage)),
        catalog::STICK => Some((280, damage)),
        _ => None,
    }
}

#[must_use]
pub fn flattened_item_id_1_13_2(stack: &ItemStack) -> Option<i32> {
    match stack.key.as_str() {
        catalog::STONE => Some(1),
        catalog::GRASS_BLOCK => Some(8),
        catalog::DIRT => Some(9),
        catalog::COBBLESTONE => Some(12),
        catalog::OAK_PLANKS => Some(13),
        catalog::OAK_LOG => Some(32),
        catalog::BEDROCK => Some(25),
        catalog::SAND => Some(26),
        catalog::GLASS => Some(64),
        catalog::SANDSTONE => Some(68),
        catalog::BRICKS => Some(135),
        catalog::CHEST => Some(149),
        catalog::CRAFTING_TABLE => Some(152),
        catalog::FURNACE => Some(154),
        catalog::STICK => Some(497),
        _ => None,
    }
}

#[must_use]
pub fn semantic_item(item_id: i16, damage: u16, count: u8) -> ItemStack {
    let key = match item_id {
        1 => catalog::STONE,
        2 => catalog::GRASS_BLOCK,
        3 => catalog::DIRT,
        4 => catalog::COBBLESTONE,
        5 if damage == 0 => catalog::OAK_PLANKS,
        17 if damage == 0 => catalog::OAK_LOG,
        12 if damage == 0 => catalog::SAND,
        20 => catalog::GLASS,
        24 if damage == 0 => catalog::SANDSTONE,
        45 => catalog::BRICKS,
        58 if damage == 0 => catalog::CRAFTING_TABLE,
        54 if damage == 0 => catalog::CHEST,
        61 if damage == 0 => catalog::FURNACE,
        280 if damage == 0 => catalog::STICK,
        _ => return ItemStack::unsupported(count, damage),
    };
    ItemStack::new(key, count, damage)
}

#[must_use]
pub fn semantic_flattened_item_1_13_2(item_id: i32, count: u8) -> ItemStack {
    let key = match item_id {
        1 => catalog::STONE,
        8 => catalog::GRASS_BLOCK,
        9 => catalog::DIRT,
        12 => catalog::COBBLESTONE,
        13 => catalog::OAK_PLANKS,
        32 => catalog::OAK_LOG,
        25 => catalog::BEDROCK,
        26 => catalog::SAND,
        64 => catalog::GLASS,
        68 => catalog::SANDSTONE,
        135 => catalog::BRICKS,
        149 => catalog::CHEST,
        152 => catalog::CRAFTING_TABLE,
        154 => catalog::FURNACE,
        497 => catalog::STICK,
        _ => {
            return ItemStack::unsupported(count, u16::try_from(item_id).unwrap_or(u16::MAX));
        }
    };
    ItemStack::new(key, count, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chest_block_and_item_round_trip_through_legacy_ids() {
        assert_eq!(legacy_block(&block(catalog::CHEST)), (54, 0));
        assert_eq!(semantic_block(54, 0), block(catalog::CHEST));

        let chest_stack = ItemStack::new(catalog::CHEST, 3, 0);
        assert_eq!(legacy_item(&chest_stack), Some((54, 0)));
        assert_eq!(semantic_item(54, 0, 3), chest_stack);
    }

    #[test]
    fn furnace_block_and_item_round_trip_through_legacy_ids() {
        assert_eq!(legacy_block(&block(catalog::FURNACE)), (61, 0));
        assert_eq!(semantic_block(61, 0), block(catalog::FURNACE));

        let furnace_stack = ItemStack::new(catalog::FURNACE, 2, 0);
        assert_eq!(legacy_item(&furnace_stack), Some((61, 0)));
        assert_eq!(semantic_item(61, 0, 2), furnace_stack);
    }

    #[test]
    fn crafting_table_block_and_item_round_trip_through_legacy_ids() {
        assert_eq!(legacy_block(&block(catalog::CRAFTING_TABLE)), (58, 0));
        assert_eq!(semantic_block(58, 0), block(catalog::CRAFTING_TABLE));

        let crafting_table_stack = ItemStack::new(catalog::CRAFTING_TABLE, 1, 0);
        assert_eq!(legacy_item(&crafting_table_stack), Some((58, 0)));
        assert_eq!(semantic_item(58, 0, 1), crafting_table_stack);
    }

    #[test]
    fn chest_round_trips_through_flattened_ids() {
        assert_eq!(
            flattened_block_state_id_1_13_2(&block(catalog::CHEST)),
            1729
        );
        assert_eq!(semantic_flattened_block_1_13_2(1752), block(catalog::CHEST));

        let chest_stack = ItemStack::new(catalog::CHEST, 3, 0);
        assert_eq!(flattened_item_id_1_13_2(&chest_stack), Some(149));
        assert_eq!(semantic_flattened_item_1_13_2(149, 3), chest_stack);
    }
}
