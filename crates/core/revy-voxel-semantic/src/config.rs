use revy_voxel_model::BlockPos;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreConfig {
    pub level_name: String,
    pub seed: u64,
    pub max_players: u8,
    pub view_distance: u8,
    pub game_mode: u8,
    pub difficulty: u8,
    pub spawn: BlockPos,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            level_name: "world".to_string(),
            seed: 0,
            max_players: 20,
            view_distance: 2,
            game_mode: 0,
            difficulty: 1,
            spawn: BlockPos::new(0, 4, 0),
        }
    }
}
