use bevy::math::DVec3;
use bevy::prelude::*;

use crate::world::{VoxelWorld, world_block_from_position};

pub fn raycast_voxel(
    world: &VoxelWorld,
    origin: DVec3,
    direction: DVec3,
    max_distance: f64,
) -> Option<(IVec3, IVec3)> {
    let dir = direction.normalize_or_zero();
    if dir == DVec3::ZERO {
        return None;
    }

    let mut current = world_block_from_position(origin);
    if world.contains_block(current) {
        return Some((current, pick_normal(dir)));
    }

    let step = compute_step(dir);
    let t_delta = compute_t_delta(dir);
    let origin_frac = origin - origin.floor();
    let mut t_max = compute_t_max(step, origin_frac, t_delta);

    raycast_voxel_traverse(world, &mut current, step, t_delta, &mut t_max, max_distance)
}

fn pick_normal(dir: DVec3) -> IVec3 {
    // This is only used when the ray origin already starts inside a solid block.
    // We still return the most-opposed axis so placement code gets a stable
    // face normal instead of a sentinel value in that edge case.
    let abs = DVec3::new(dir.x.abs(), dir.y.abs(), dir.z.abs());
    let step = compute_step(dir);
    if abs.x >= abs.y && abs.x >= abs.z {
        IVec3::new(-step.x, 0, 0)
    } else if abs.y >= abs.x && abs.y >= abs.z {
        IVec3::new(0, -step.y, 0)
    } else {
        IVec3::new(0, 0, -step.z)
    }
}

fn compute_step(dir: DVec3) -> IVec3 {
    IVec3::new(step_axis(dir.x), step_axis(dir.y), step_axis(dir.z))
}

fn step_axis(component: f64) -> i32 {
    if component > 0.0 {
        1
    } else if component < 0.0 {
        -1
    } else {
        0
    }
}

fn compute_t_delta(dir: DVec3) -> DVec3 {
    DVec3::new(
        reciprocal_abs_or_infinity(dir.x),
        reciprocal_abs_or_infinity(dir.y),
        reciprocal_abs_or_infinity(dir.z),
    )
}

fn reciprocal_abs_or_infinity(component: f64) -> f64 {
    if component.abs() > f64::EPSILON {
        1.0 / component.abs()
    } else {
        f64::INFINITY
    }
}

fn compute_t_max(step: IVec3, origin_frac: DVec3, t_delta: DVec3) -> DVec3 {
    DVec3::new(
        first_boundary_distance(step.x, origin_frac.x, t_delta.x),
        first_boundary_distance(step.y, origin_frac.y, t_delta.y),
        first_boundary_distance(step.z, origin_frac.z, t_delta.z),
    )
}

fn first_boundary_distance(step: i32, origin_frac: f64, t_delta: f64) -> f64 {
    if step == 0 {
        return f64::INFINITY;
    }

    if step > 0 {
        (1.0 - origin_frac) * t_delta
    } else {
        origin_frac * t_delta
    }
}

fn raycast_voxel_traverse(
    world: &VoxelWorld,
    current: &mut IVec3,
    step: IVec3,
    t_delta: DVec3,
    t_max: &mut DVec3,
    max_distance: f64,
) -> Option<(IVec3, IVec3)> {
    loop {
        let (t_next, axis) = next_step(*t_max);
        if t_next > max_distance {
            break;
        }

        let normal = advance_to_next_voxel(current, step, t_delta, t_max, axis);
        if world.contains_block(*current) {
            return Some((*current, normal));
        }
    }

    None
}

fn next_step(t_max: DVec3) -> (f64, usize) {
    if t_max.x < t_max.y {
        if t_max.x < t_max.z {
            (t_max.x, 0)
        } else {
            (t_max.z, 2)
        }
    } else if t_max.y < t_max.z {
        (t_max.y, 1)
    } else {
        (t_max.z, 2)
    }
}

fn advance_to_next_voxel(
    current: &mut IVec3,
    step: IVec3,
    t_delta: DVec3,
    t_max: &mut DVec3,
    axis: usize,
) -> IVec3 {
    match axis {
        0 => {
            current.x += step.x;
            t_max.x += t_delta.x;
            IVec3::new(-step.x, 0, 0)
        }
        1 => {
            current.y += step.y;
            t_max.y += t_delta.y;
            IVec3::new(0, -step.y, 0)
        }
        2 => {
            current.z += step.z;
            t_max.z += t_delta.z;
            IVec3::new(0, 0, -step.z)
        }
        _ => unreachable!("next_step only returns axis indices 0..=2"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::BlockType;

    #[test]
    fn raycast_hits_single_block_and_reports_face_normal() {
        let mut world = VoxelWorld::default();
        world.try_insert_block(IVec3::new(1, 0, 0), BlockType::Stone);

        let hit = raycast_voxel(
            &world,
            DVec3::new(0.25, 0.5, 0.5),
            DVec3::new(1.0, 0.0, 0.0),
            8.0,
        );

        assert_eq!(hit, Some((IVec3::new(1, 0, 0), IVec3::new(-1, 0, 0))));
    }

    #[test]
    fn raycast_stays_precise_beyond_f32_integer_resolution() {
        let mut world = VoxelWorld::default();
        world.try_insert_block(IVec3::new(16_777_218, 0, 0), BlockType::Stone);

        let hit = raycast_voxel(
            &world,
            DVec3::new(16_777_217.25, 0.5, 0.5),
            DVec3::new(1.0, 0.0, 0.0),
            8.0,
        );

        assert_eq!(
            hit,
            Some((IVec3::new(16_777_218, 0, 0), IVec3::new(-1, 0, 0)))
        );
    }
}
