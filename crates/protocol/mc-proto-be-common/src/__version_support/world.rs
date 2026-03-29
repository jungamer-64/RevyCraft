use crate::world;
use mc_proto_common::ProtocolError;
use revy_voxel_core::EntityId;
use revy_voxel_model::{BlockFace, BlockPos, Vec3 as ModelVec3};
use vek::Vec3;

pub fn bedrock_actor_id(entity_id: EntityId) -> u64 {
    world::bedrock_actor_id(entity_id)
}

pub const fn block_face_from_i32(face: i32) -> Option<BlockFace> {
    world::block_face_from_i32(face)
}

pub fn block_pos_from_network(
    position: &bedrockrs_proto::v662::types::NetworkBlockPosition,
) -> BlockPos {
    world::block_pos_from_network(position)
}

pub fn block_pos_to_network(
    position: BlockPos,
) -> bedrockrs_proto::v662::types::NetworkBlockPosition {
    world::block_pos_to_network(position)
}

pub const fn protocol_error(message: &'static str) -> ProtocolError {
    world::protocol_error(message)
}

pub fn vec3_to_bedrock(position: ModelVec3) -> Vec3<f32> {
    world::vec3_to_bedrock(position)
}
