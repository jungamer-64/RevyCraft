use crate::{BlockState, ItemStack};

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
pub const CHEST: &str = "minecraft:chest";
pub const FURNACE: &str = "minecraft:furnace";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolClass {
    Pickaxe,
    Shovel,
    Axe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MiningToolSpec {
    pub class: ToolClass,
    pub tier: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiningBlockSpec {
    pub hardness: f32,
    pub preferred_tool: Option<ToolClass>,
}

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
        CHEST => Some(BlockState::chest()),
        FURNACE => Some(BlockState::furnace()),
        _ => None,
    }
}

#[must_use]
pub fn is_supported_placeable_item(key: &str) -> bool {
    placeable_block_state_from_item_key(key).is_some()
}

#[must_use]
pub fn is_supported_inventory_item(key: &str) -> bool {
    matches!(key, OAK_LOG | STICK | CHEST | FURNACE) || is_supported_placeable_item(key)
}

#[must_use]
pub fn survival_drop_for_block(block: &BlockState) -> Option<crate::ItemStack> {
    let key = match block.key.as_str() {
        STONE => COBBLESTONE,
        GRASS_BLOCK => DIRT,
        DIRT => DIRT,
        COBBLESTONE => COBBLESTONE,
        OAK_PLANKS => OAK_PLANKS,
        SAND => SAND,
        SANDSTONE => SANDSTONE,
        BRICKS => BRICKS,
        CHEST => CHEST,
        FURNACE => FURNACE,
        GLASS => return None,
        _ => return None,
    };
    Some(crate::ItemStack::new(key, 1, 0))
}

#[must_use]
pub fn mining_spec_for_block(block: &BlockState) -> Option<MiningBlockSpec> {
    Some(match block.key.as_str() {
        STONE => MiningBlockSpec {
            hardness: 1.5,
            preferred_tool: Some(ToolClass::Pickaxe),
        },
        GRASS_BLOCK => MiningBlockSpec {
            hardness: 0.6,
            preferred_tool: Some(ToolClass::Shovel),
        },
        DIRT => MiningBlockSpec {
            hardness: 0.5,
            preferred_tool: Some(ToolClass::Shovel),
        },
        COBBLESTONE => MiningBlockSpec {
            hardness: 2.0,
            preferred_tool: Some(ToolClass::Pickaxe),
        },
        OAK_PLANKS => MiningBlockSpec {
            hardness: 2.0,
            preferred_tool: Some(ToolClass::Axe),
        },
        SAND => MiningBlockSpec {
            hardness: 0.5,
            preferred_tool: Some(ToolClass::Shovel),
        },
        SANDSTONE => MiningBlockSpec {
            hardness: 0.8,
            preferred_tool: Some(ToolClass::Pickaxe),
        },
        GLASS => MiningBlockSpec {
            hardness: 0.3,
            preferred_tool: None,
        },
        BRICKS => MiningBlockSpec {
            hardness: 2.0,
            preferred_tool: Some(ToolClass::Pickaxe),
        },
        CHEST => MiningBlockSpec {
            hardness: 2.5,
            preferred_tool: Some(ToolClass::Axe),
        },
        FURNACE => MiningBlockSpec {
            hardness: 3.5,
            preferred_tool: Some(ToolClass::Pickaxe),
        },
        BEDROCK => return None,
        _ => return None,
    })
}

#[must_use]
pub const fn tool_spec_for_item(_item: Option<&ItemStack>) -> Option<MiningToolSpec> {
    None
}

#[must_use]
pub fn survival_mining_duration_ms(
    block: &BlockState,
    _tool: Option<MiningToolSpec>,
) -> Option<u64> {
    let spec = mining_spec_for_block(block)?;
    let ticks = (spec.hardness * 30.0).ceil() as u64;
    Some(ticks.max(1) * 50)
}
