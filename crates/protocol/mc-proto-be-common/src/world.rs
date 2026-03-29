use mc_proto_common::ProtocolError;
use num_traits::ToPrimitive;
use revy_voxel_core::EntityId;
use revy_voxel_model::{BlockFace, BlockPos, Vec3 as ModelVec3};
use vek::Vec3;

#[must_use]
pub(crate) fn block_pos_to_network(
    position: BlockPos,
) -> bedrockrs_proto::v662::types::NetworkBlockPosition {
    bedrockrs_proto::v662::types::NetworkBlockPosition {
        x: position.x,
        y: position.y.max(0).cast_unsigned(),
        z: position.z,
    }
}

#[must_use]
pub(crate) fn block_pos_from_network(
    position: &bedrockrs_proto::v662::types::NetworkBlockPosition,
) -> BlockPos {
    BlockPos::new(
        position.x,
        i32::try_from(position.y).unwrap_or(i32::MAX),
        position.z,
    )
}

#[must_use]
pub(crate) fn vec3_to_bedrock(position: ModelVec3) -> Vec3<f32> {
    Vec3::new(
        f64_to_bedrock_component(position.x),
        f64_to_bedrock_component(position.y),
        f64_to_bedrock_component(position.z),
    )
}

#[must_use]
pub(crate) const fn protocol_error(message: &'static str) -> ProtocolError {
    ProtocolError::InvalidPacket(message)
}

#[must_use]
pub(crate) const fn block_face_from_i32(face: i32) -> Option<BlockFace> {
    match face {
        0 => Some(BlockFace::Bottom),
        1 => Some(BlockFace::Top),
        2 => Some(BlockFace::North),
        3 => Some(BlockFace::South),
        4 => Some(BlockFace::West),
        5 => Some(BlockFace::East),
        _ => None,
    }
}

#[must_use]
pub(crate) fn bedrock_actor_id(entity_id: EntityId) -> u64 {
    u64::try_from(entity_id.0).expect("bedrock entity id should be non-negative")
}

fn f64_to_bedrock_component(value: f64) -> f32 {
    value
        .to_f32()
        .expect("bedrock position component should fit into f32")
}
