use crate::world;
use mc_proto_common::ProtocolError;
use revy_voxel_semantic::EntityId;
use revy_voxel_semantic::{BlockFace, BlockPos, Vec3 as ModelVec3};

pub fn bedrock_actor_runtime_id(entity_id: EntityId) -> u64 {
    world::bedrock_actor_runtime_id(entity_id)
}

pub fn bedrock_actor_unique_id(entity_id: EntityId) -> i64 {
    world::bedrock_actor_unique_id(entity_id)
}

pub const fn block_face_from_i32(face: i32) -> Option<BlockFace> {
    world::block_face_from_i32(face)
}

pub fn block_pos_from_network(
    position: &bedrock_protocol::v662::types::NetworkBlockPosition,
) -> BlockPos {
    world::block_pos_from_network(position)
}

pub fn block_pos_to_network(
    position: BlockPos,
) -> bedrock_protocol::v662::types::NetworkBlockPosition {
    world::block_pos_to_network(position)
}

pub const fn protocol_error(message: &'static str) -> ProtocolError {
    world::protocol_error(message)
}

pub fn vec3_to_bedrock(position: ModelVec3) -> (f32, f32, f32) {
    world::vec3_to_bedrock(position)
}
