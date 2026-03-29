#![allow(clippy::multiple_crate_versions)]

pub mod ids {
    pub const PLAYER: &str = "canonical:player";
    pub const CRAFTING_TABLE: &str = "canonical:crafting_table";
    pub const CHEST_27: &str = "canonical:chest_27";
    pub const FURNACE: &str = "canonical:furnace";

    pub const CHEST_BLOCK_ENTITY: &str = "canonical:chest";
    pub const FURNACE_BLOCK_ENTITY: &str = "canonical:furnace";

    pub const FURNACE_BURN_LEFT: &str = "canonical:furnace.burn_left";
    pub const FURNACE_BURN_MAX: &str = "canonical:furnace.burn_max";
    pub const FURNACE_COOK_PROGRESS: &str = "canonical:furnace.cook_progress";
    pub const FURNACE_COOK_TOTAL: &str = "canonical:furnace.cook_total";
}

pub mod catalog;
mod gameplay;

pub use self::gameplay::{
    canonical_content, creative_starter_inventory, default_block_entity_for_block,
    default_block_entity_for_kind, default_chunk, item_supported_for_inventory,
    placeable_block_state_from_item_key,
};
