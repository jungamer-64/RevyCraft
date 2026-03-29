use crate::PlayerId;
use revy_voxel_model::{DimensionId, PlayerInventory, Vec3};
use serde::{Deserialize, Serialize};

pub(crate) use revy_voxel_model::InteractionHand;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    pub id: PlayerId,
    pub username: String,
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub dimension: DimensionId,
    pub health: f32,
    pub food: i16,
    pub food_saturation: f32,
    pub inventory: PlayerInventory,
    pub selected_hotbar_slot: u8,
}
