use mc_core::catalog::{
    BEDROCK, BRICKS, COBBLESTONE, DIRT, GLASS, GRASS_BLOCK, OAK_PLANKS, SAND, SANDSTONE, STONE,
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
        SAND => Some((12, damage)),
        GLASS => Some((20, damage)),
        SANDSTONE => Some((24, damage)),
        BRICKS => Some((45, damage)),
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
        12 if damage == 0 => SAND,
        20 => GLASS,
        24 if damage == 0 => SANDSTONE,
        45 => BRICKS,
        _ => return ItemStack::unsupported(count, damage),
    };
    ItemStack::new(key, count, damage)
}
