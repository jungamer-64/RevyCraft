use mc_content_canonical::catalog;
use revy_voxel_model::BlockState;

pub(crate) const BEDROCK_26_3_RUNTIME_ID_STONE: u32 = 2_532;
pub(crate) const BEDROCK_26_3_RUNTIME_ID_COBBLESTONE: u32 = 5_088;
pub(crate) const BEDROCK_26_3_RUNTIME_ID_SAND: u32 = 6_234;
pub(crate) const BEDROCK_26_3_RUNTIME_ID_BRICKS: u32 = 7_455;
pub(crate) const BEDROCK_26_3_RUNTIME_ID_DIRT: u32 = 9_852;
pub(crate) const BEDROCK_26_3_RUNTIME_ID_GRASS_BLOCK: u32 = 11_062;
pub(crate) const BEDROCK_26_3_RUNTIME_ID_GLASS: u32 = 11_998;
pub(crate) const BEDROCK_26_3_RUNTIME_ID_AIR: u32 = 12_530;
pub(crate) const BEDROCK_26_3_RUNTIME_ID_BEDROCK: u32 = 13_079;
pub(crate) const BEDROCK_26_3_RUNTIME_ID_OAK_PLANKS: u32 = 14_388;

pub(crate) fn block_runtime_id(block: &BlockState) -> u32 {
    match block.key.as_str() {
        catalog::COBBLESTONE => BEDROCK_26_3_RUNTIME_ID_COBBLESTONE,
        catalog::SAND => BEDROCK_26_3_RUNTIME_ID_SAND,
        catalog::BRICKS => BEDROCK_26_3_RUNTIME_ID_BRICKS,
        catalog::DIRT => BEDROCK_26_3_RUNTIME_ID_DIRT,
        catalog::GRASS_BLOCK => BEDROCK_26_3_RUNTIME_ID_GRASS_BLOCK,
        catalog::GLASS => BEDROCK_26_3_RUNTIME_ID_GLASS,
        catalog::AIR => BEDROCK_26_3_RUNTIME_ID_AIR,
        catalog::BEDROCK => BEDROCK_26_3_RUNTIME_ID_BEDROCK,
        catalog::OAK_PLANKS => BEDROCK_26_3_RUNTIME_ID_OAK_PLANKS,
        _ => BEDROCK_26_3_RUNTIME_ID_STONE,
    }
}
