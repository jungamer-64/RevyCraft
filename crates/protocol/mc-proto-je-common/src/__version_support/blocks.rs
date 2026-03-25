use mc_core::catalog::{
    BEDROCK, BRICKS, CHEST, COBBLESTONE, DIRT, FURNACE, GLASS, GRASS_BLOCK, OAK_LOG, OAK_PLANKS,
    SAND, SANDSTONE, STICK, STONE,
};
use mc_core::{BlockState, ItemStack};

#[must_use]
pub fn legacy_block(state: &BlockState) -> (u16, u8) {
    match state.key.as_str() {
        STONE => (1, 0),
        GRASS_BLOCK => (2, 0),
        DIRT => (3, 0),
        COBBLESTONE => (4, 0),
        OAK_PLANKS => (5, 0),
        BEDROCK => (7, 0),
        SAND => (12, 0),
        GLASS => (20, 0),
        SANDSTONE => (24, 0),
        BRICKS => (45, 0),
        CHEST => (54, 0),
        FURNACE => (61, 0),
        _ => (0, 0),
    }
}

#[must_use]
pub fn legacy_block_state_id(state: &BlockState) -> i32 {
    let (block_id, metadata) = legacy_block(state);
    (i32::from(block_id) << 4) | i32::from(metadata)
}

#[must_use]
pub fn semantic_block(block_id: u16, metadata: u8) -> BlockState {
    match block_id {
        1 => BlockState::stone(),
        2 => BlockState::grass_block(),
        3 => BlockState::dirt(),
        4 => BlockState::cobblestone(),
        5 if metadata == 0 => BlockState::oak_planks(),
        7 => BlockState::bedrock(),
        12 if metadata == 0 => BlockState::sand(),
        20 => BlockState::glass(),
        24 if metadata == 0 => BlockState::sandstone(),
        45 => BlockState::bricks(),
        54 => BlockState::chest(),
        61 => BlockState::furnace(),
        _ => BlockState::air(),
    }
}

#[must_use]
pub fn legacy_item(stack: &ItemStack) -> Option<(i16, u16)> {
    let damage = stack.damage;
    match stack.key.as_str() {
        STONE => Some((1, damage)),
        GRASS_BLOCK => Some((2, damage)),
        DIRT => Some((3, damage)),
        COBBLESTONE => Some((4, damage)),
        OAK_PLANKS => Some((5, damage)),
        OAK_LOG => Some((17, damage)),
        SAND => Some((12, damage)),
        GLASS => Some((20, damage)),
        SANDSTONE => Some((24, damage)),
        BRICKS => Some((45, damage)),
        CHEST => Some((54, damage)),
        FURNACE => Some((61, damage)),
        STICK => Some((280, damage)),
        _ => None,
    }
}

#[must_use]
pub fn semantic_item(item_id: i16, damage: u16, count: u8) -> ItemStack {
    let key = match item_id {
        1 => STONE,
        2 => GRASS_BLOCK,
        3 => DIRT,
        4 => COBBLESTONE,
        5 if damage == 0 => OAK_PLANKS,
        17 if damage == 0 => OAK_LOG,
        12 if damage == 0 => SAND,
        20 => GLASS,
        24 if damage == 0 => SANDSTONE,
        45 => BRICKS,
        54 if damage == 0 => CHEST,
        61 if damage == 0 => FURNACE,
        280 if damage == 0 => STICK,
        _ => return ItemStack::unsupported(count, damage),
    };
    ItemStack::new(key, count, damage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chest_block_and_item_round_trip_through_legacy_ids() {
        assert_eq!(legacy_block(&BlockState::chest()), (54, 0));
        assert_eq!(semantic_block(54, 0), BlockState::chest());

        let chest_stack = ItemStack::new(CHEST, 3, 0);
        assert_eq!(legacy_item(&chest_stack), Some((54, 0)));
        assert_eq!(semantic_item(54, 0, 3), chest_stack);
    }

    #[test]
    fn furnace_block_and_item_round_trip_through_legacy_ids() {
        assert_eq!(legacy_block(&BlockState::furnace()), (61, 0));
        assert_eq!(semantic_block(61, 0), BlockState::furnace());

        let furnace_stack = ItemStack::new(FURNACE, 2, 0);
        assert_eq!(legacy_item(&furnace_stack), Some((61, 0)));
        assert_eq!(semantic_item(61, 0, 2), furnace_stack);
    }
}
