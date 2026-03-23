use crate::BlockState;

pub const STONE: &str = "minecraft:stone";
pub const DIRT: &str = "minecraft:dirt";
pub const GRASS_BLOCK: &str = "minecraft:grass_block";
pub const COBBLESTONE: &str = "minecraft:cobblestone";
pub const OAK_PLANKS: &str = "minecraft:oak_planks";
pub const BEDROCK: &str = "minecraft:bedrock";
pub const SAND: &str = "minecraft:sand";
pub const SANDSTONE: &str = "minecraft:sandstone";
pub const GLASS: &str = "minecraft:glass";
pub const BRICKS: &str = "minecraft:bricks";
pub const OAK_LOG: &str = "minecraft:oak_log";
pub const STICK: &str = "minecraft:stick";

#[must_use]
pub const fn starter_hotbar_item_keys() -> [&'static str; 9] {
    [
        STONE,
        DIRT,
        GRASS_BLOCK,
        COBBLESTONE,
        OAK_PLANKS,
        SAND,
        SANDSTONE,
        GLASS,
        BRICKS,
    ]
}

#[must_use]
pub fn placeable_block_state_from_item_key(key: &str) -> Option<BlockState> {
    match key {
        STONE => Some(BlockState::stone()),
        DIRT => Some(BlockState::dirt()),
        GRASS_BLOCK => Some(BlockState::grass_block()),
        COBBLESTONE => Some(BlockState::cobblestone()),
        OAK_PLANKS => Some(BlockState::oak_planks()),
        SAND => Some(BlockState::sand()),
        SANDSTONE => Some(BlockState::sandstone()),
        GLASS => Some(BlockState::glass()),
        BRICKS => Some(BlockState::bricks()),
        _ => None,
    }
}

#[must_use]
pub fn is_supported_placeable_item(key: &str) -> bool {
    placeable_block_state_from_item_key(key).is_some()
}

#[must_use]
pub fn is_supported_inventory_item(key: &str) -> bool {
    matches!(key, OAK_LOG | STICK) || is_supported_placeable_item(key)
}
