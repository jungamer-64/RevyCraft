use bevy::ecs::system::SystemParam;
use bevy::input::{
    ButtonInput,
    keyboard::KeyCode,
    mouse::{AccumulatedMouseMotion, MouseButton},
};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::cursor::cursor_is_locked;
use crate::world::VoxelWorld;

pub const EYE_HEIGHT: f32 = 1.62;
pub const INITIAL_CAMERA_EYE_POSITION: Vec3 = Vec3::new(0.0, 36.0, 0.0);

const PLAYER_HEIGHT: f32 = 1.8;
const PLAYER_HALF_WIDTH: f32 = 0.3;

const GRAVITY: f32 = 9.81;
const JUMP_SPEED: f32 = 5.0;
const MOVE_SPEED: f32 = 10.0;
const TERMINAL_VELOCITY: f32 = -50.0;

const MAX_AXIS_STEP: f32 = 0.4;
const MAX_AXIS_SUBSTEPS: i32 = 32;
const AXIS_SWEEP_REFINEMENT_STEPS: i32 = 10;
const SUPPORT_CHECK_DISTANCE: f32 = 0.002;

#[derive(Clone, Copy)]
enum CollisionBoundary {
    Exclusive,
    Inclusive,
}

#[derive(Clone, Copy)]
struct Aabb {
    min: Vec3,
    max: Vec3,
}

#[derive(Component)]
pub struct MainCamera;

#[derive(Component)]
struct CameraOrientation {
    yaw: f32,
    pitch: f32,
}

#[derive(Component)]
struct PlayerPhysics {
    velocity: Vec3,
    grounded: bool,
}

pub fn spawn_camera(commands: &mut Commands, render_origin_root: Entity) {
    let yaw = 0.0;
    let pitch = -0.2;
    let rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);

    commands.entity(render_origin_root).with_children(|parent| {
        parent
            .spawn((
                Camera3d::default(),
                Transform::from_translation(INITIAL_CAMERA_EYE_POSITION).with_rotation(rotation),
            ))
            .insert(MainCamera)
            .insert(CameraOrientation { yaw, pitch })
            .insert(PlayerPhysics {
                velocity: Vec3::ZERO,
                grounded: false,
            });
    });
}

pub fn lock_cursor(cursor: &mut Query<&mut CursorOptions, With<PrimaryWindow>>) {
    if let Ok(mut cursor) = cursor.single_mut() {
        apply_cursor_lock(&mut cursor);
    }
}

#[derive(SystemParam)]
pub struct CameraMovementResources<'w, 's> {
    time: Res<'w, Time>,
    keyboard: Res<'w, ButtonInput<KeyCode>>,
    cursor_options: Query<'w, 's, &'static CursorOptions, With<PrimaryWindow>>,
    query: Query<'w, 's, (&'static mut Transform, &'static mut PlayerPhysics), With<MainCamera>>,
    voxel_world: Res<'w, VoxelWorld>,
}

impl CameraMovementResources<'_, '_> {
    fn run(mut self) {
        if !cursor_is_locked(self.cursor_options.single().ok()) {
            return;
        }

        let Ok((mut transform, mut player)) = self.query.single_mut() else {
            return;
        };

        let mut foot_position = eye_to_foot_position(transform.translation);
        apply_horizontal_input(&self.keyboard, &transform, &mut player);
        integrate_vertical_velocity(&self.time, &self.keyboard, &mut player);

        let delta = player.velocity * self.time.delta_secs();
        resolve_horizontal_movement(&self.voxel_world, &mut foot_position, &mut player, delta);
        resolve_vertical_movement(&self.voxel_world, &mut foot_position, &mut player, delta.y);
        player.grounded = has_support_below(&self.voxel_world, foot_position);
        transform.translation = foot_to_eye_position(foot_position);
    }
}

pub fn camera_movement_system(resources: CameraMovementResources) {
    resources.run();
}

#[derive(SystemParam)]
pub struct CameraLookResources<'w, 's> {
    accumulated_mouse_motion: Res<'w, AccumulatedMouseMotion>,
    cursor_options: Query<'w, 's, &'static CursorOptions, With<PrimaryWindow>>,
    query:
        Query<'w, 's, (&'static mut Transform, &'static mut CameraOrientation), With<MainCamera>>,
}

impl CameraLookResources<'_, '_> {
    fn run(mut self) {
        if !cursor_is_locked(self.cursor_options.single().ok()) {
            return;
        }

        let Ok((mut transform, mut camera)) = self.query.single_mut() else {
            return;
        };

        let delta = self.accumulated_mouse_motion.delta;
        if delta == Vec2::ZERO {
            return;
        }

        let sensitivity = 0.002;
        camera.yaw -= delta.x * sensitivity;
        camera.pitch = delta.y.mul_add(-sensitivity, camera.pitch).clamp(
            -std::f32::consts::FRAC_PI_2 + 0.01,
            std::f32::consts::FRAC_PI_2 - 0.01,
        );
        transform.rotation =
            Quat::from_rotation_y(camera.yaw) * Quat::from_rotation_x(camera.pitch);
    }
}

pub fn camera_look_system(resources: CameraLookResources) {
    resources.run();
}

#[cfg(test)]
pub fn player_collides_voxel(foot_position: Vec3, voxel: IVec3) -> bool {
    aabbs_overlap(
        player_aabb(foot_position),
        voxel_aabb(voxel),
        CollisionBoundary::Exclusive,
    )
}

pub fn player_blocks_block_placement(foot_position: Vec3, voxel: IVec3) -> bool {
    // Placement uses the same occupied volume as movement, but treats exact
    // tangential contact as blocked so block placement stays conservative.
    aabbs_overlap(
        player_aabb(foot_position),
        voxel_aabb(voxel),
        CollisionBoundary::Inclusive,
    )
}

#[derive(SystemParam)]
pub struct CursorGrabResources<'w, 's> {
    keyboard: Res<'w, ButtonInput<KeyCode>>,
    mouse_button: Res<'w, ButtonInput<MouseButton>>,
    cursor: Query<'w, 's, &'static mut CursorOptions, With<PrimaryWindow>>,
}

impl CursorGrabResources<'_, '_> {
    fn run(mut self) {
        if self.keyboard.just_pressed(KeyCode::Escape)
            && let Ok(mut cursor) = self.cursor.single_mut()
        {
            release_cursor(&mut cursor);
        }

        if self.mouse_button.just_pressed(MouseButton::Left)
            && let Ok(mut cursor) = self.cursor.single_mut()
        {
            apply_cursor_lock(&mut cursor);
        }
    }
}

pub fn toggle_cursor_grab(resources: CursorGrabResources) {
    resources.run();
}

#[inline]
const fn apply_cursor_lock(cursor: &mut CursorOptions) {
    cursor.visible = false;
    cursor.grab_mode = CursorGrabMode::Locked;
}

#[inline]
const fn release_cursor(cursor: &mut CursorOptions) {
    cursor.visible = true;
    cursor.grab_mode = CursorGrabMode::None;
}

#[inline]
const fn eye_to_foot_position(eye_position: Vec3) -> Vec3 {
    Vec3::new(eye_position.x, eye_position.y - EYE_HEIGHT, eye_position.z)
}

#[inline]
const fn foot_to_eye_position(foot_position: Vec3) -> Vec3 {
    Vec3::new(
        foot_position.x,
        foot_position.y + EYE_HEIGHT,
        foot_position.z,
    )
}

fn apply_horizontal_input(
    keyboard: &ButtonInput<KeyCode>,
    transform: &Transform,
    player: &mut PlayerPhysics,
) {
    let desired_velocity = desired_horizontal_velocity(transform, keyboard);
    player.velocity.x = desired_velocity.x;
    player.velocity.z = desired_velocity.z;
}

fn desired_horizontal_velocity(transform: &Transform, keyboard: &ButtonInput<KeyCode>) -> Vec3 {
    let (forward, right) = movement_basis(transform);
    let mut input_dir = Vec3::ZERO;

    if keyboard.pressed(KeyCode::KeyW) {
        input_dir += forward;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        input_dir -= forward;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        input_dir -= right;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        input_dir += right;
    }

    if input_dir.length_squared() > 0.0 {
        input_dir.normalize() * MOVE_SPEED
    } else {
        Vec3::ZERO
    }
}

fn movement_basis(transform: &Transform) -> (Vec3, Vec3) {
    let forward: Vec3 = transform.forward().into();
    let right: Vec3 = transform.right().into();

    (
        forward.with_y(0.0).normalize_or_zero(),
        right.with_y(0.0).normalize_or_zero(),
    )
}

fn integrate_vertical_velocity(
    time: &Time,
    keyboard: &ButtonInput<KeyCode>,
    player: &mut PlayerPhysics,
) {
    if player.grounded {
        player.velocity.y = 0.0;
        if keyboard.just_pressed(KeyCode::Space) {
            player.velocity.y = JUMP_SPEED;
            player.grounded = false;
        }
        return;
    }

    player.velocity.y = GRAVITY
        .mul_add(-time.delta_secs(), player.velocity.y)
        .max(TERMINAL_VELOCITY);
}

fn resolve_horizontal_movement(
    world: &VoxelWorld,
    foot_position: &mut Vec3,
    player: &mut PlayerPhysics,
    delta: Vec3,
) {
    // Resolve each horizontal axis independently so movement can slide along
    // walls instead of stopping completely on corner contact.
    resolve_horizontal_axis(
        world,
        foot_position,
        &mut player.velocity.x,
        delta.x,
        Vec3::X,
    );
    resolve_horizontal_axis(
        world,
        foot_position,
        &mut player.velocity.z,
        delta.z,
        Vec3::Z,
    );
}

fn resolve_horizontal_axis(
    world: &VoxelWorld,
    foot_position: &mut Vec3,
    velocity: &mut f32,
    delta: f32,
    axis: Vec3,
) {
    let _ = resolve_axis_movement(world, foot_position, velocity, delta, axis);
}

fn resolve_vertical_movement(
    world: &VoxelWorld,
    foot_position: &mut Vec3,
    player: &mut PlayerPhysics,
    delta_y: f32,
) {
    let _ = resolve_axis_movement(
        world,
        foot_position,
        &mut player.velocity.y,
        delta_y,
        Vec3::Y,
    );
}

fn resolve_axis_movement(
    world: &VoxelWorld,
    foot_position: &mut Vec3,
    velocity: &mut f32,
    delta: f32,
    axis: Vec3,
) -> bool {
    let mut remaining = delta;

    for _ in 0..MAX_AXIS_SUBSTEPS {
        if remaining.abs() < f32::EPSILON {
            return false;
        }

        let step = remaining.clamp(-MAX_AXIS_STEP, MAX_AXIS_STEP);
        if move_to_axis_contact(world, foot_position, step, axis) {
            *velocity = 0.0;
            return true;
        }

        remaining -= step;
    }

    false
}

fn move_to_axis_contact(
    world: &VoxelWorld,
    foot_position: &mut Vec3,
    delta: f32,
    axis: Vec3,
) -> bool {
    if delta.abs() < f32::EPSILON {
        return false;
    }

    let start = *foot_position;
    *foot_position += axis * delta;
    if !collides(world, *foot_position) {
        return false;
    }

    *foot_position = start;

    let direction = delta.signum();
    let mut safe_distance = 0.0;
    let mut blocked_distance = delta.abs();

    for _ in 0..AXIS_SWEEP_REFINEMENT_STEPS {
        let candidate_distance = 0.5 * (safe_distance + blocked_distance);
        let candidate = start + axis * (direction * candidate_distance);
        if collides(world, candidate) {
            blocked_distance = candidate_distance;
        } else {
            safe_distance = candidate_distance;
        }
    }

    *foot_position = start + axis * (direction * safe_distance);
    true
}

fn has_support_below(world: &VoxelWorld, foot_position: Vec3) -> bool {
    let mut support_aabb = player_aabb(foot_position);
    support_aabb.min.y -= SUPPORT_CHECK_DISTANCE;
    support_aabb.max.y -= SUPPORT_CHECK_DISTANCE;
    aabb_collides_world(world, support_aabb, CollisionBoundary::Exclusive)
}

fn collides(world: &VoxelWorld, position: Vec3) -> bool {
    aabb_collides_world(world, player_aabb(position), CollisionBoundary::Exclusive)
}

fn player_aabb(foot_position: Vec3) -> Aabb {
    Aabb {
        min: foot_position + Vec3::new(-PLAYER_HALF_WIDTH, 0.0, -PLAYER_HALF_WIDTH),
        max: foot_position + Vec3::new(PLAYER_HALF_WIDTH, PLAYER_HEIGHT, PLAYER_HALF_WIDTH),
    }
}

fn voxel_aabb(voxel: IVec3) -> Aabb {
    let min = voxel.as_vec3();
    Aabb {
        min,
        max: min + Vec3::ONE,
    }
}

fn aabb_collides_world(world: &VoxelWorld, aabb: Aabb, boundary: CollisionBoundary) -> bool {
    let min = aabb.min.floor().as_ivec3();
    let max = aabb.max.floor().as_ivec3();

    for x in min.x..=max.x {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                let block_coord = IVec3::new(x, y, z);
                if !world.contains_block(block_coord) {
                    continue;
                }

                if aabbs_overlap(aabb, voxel_aabb(block_coord), boundary) {
                    return true;
                }
            }
        }
    }

    false
}

fn aabbs_overlap(lhs: Aabb, rhs: Aabb, boundary: CollisionBoundary) -> bool {
    match boundary {
        CollisionBoundary::Exclusive => {
            lhs.min.x < rhs.max.x
                && lhs.max.x > rhs.min.x
                && lhs.min.y < rhs.max.y
                && lhs.max.y > rhs.min.y
                && lhs.min.z < rhs.max.z
                && lhs.max.z > rhs.min.z
        }
        CollisionBoundary::Inclusive => {
            lhs.min.x <= rhs.max.x
                && lhs.max.x >= rhs.min.x
                && lhs.min.y <= rhs.max.y
                && lhs.max.y >= rhs.min.y
                && lhs.min.z <= rhs.max.z
                && lhs.max.z >= rhs.min.z
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{BlockType, VoxelWorld, WorldLayout};

    #[test]
    fn player_collides_when_aabb_overlaps_voxel() {
        assert!(player_collides_voxel(Vec3::new(0.2, 0.0, 0.2), IVec3::ZERO));
    }

    #[test]
    fn player_does_not_collide_when_clear_of_voxel() {
        assert!(!player_collides_voxel(
            Vec3::new(2.0, 0.0, 2.0),
            IVec3::ZERO
        ));
    }

    #[test]
    fn player_collides_on_diagonal_corner_overlap_like_minecraft() {
        assert!(player_collides_voxel(
            Vec3::new(1.29, 0.0, 1.29),
            IVec3::ZERO,
        ));
    }

    #[test]
    fn placement_collision_matches_player_volume() {
        assert!(player_blocks_block_placement(
            Vec3::new(0.2, 0.0, 0.2),
            IVec3::ZERO
        ));
    }

    #[test]
    fn aabb_overlap_uses_exclusive_movement_and_inclusive_placement_boundaries() {
        let player_box = Aabb {
            min: Vec3::new(1.0, 0.0, 0.2),
            max: Vec3::new(1.6, PLAYER_HEIGHT, 0.8),
        };

        assert!(!aabbs_overlap(
            player_box,
            voxel_aabb(IVec3::ZERO),
            CollisionBoundary::Exclusive,
        ));
        assert!(aabbs_overlap(
            player_box,
            voxel_aabb(IVec3::ZERO),
            CollisionBoundary::Inclusive,
        ));
    }

    #[test]
    fn support_check_requires_block_below_player() {
        let mut world = VoxelWorld::new(WorldLayout::default());
        assert!(world.try_insert_block(IVec3::new(0, 1, 0), BlockType::Stone));

        let foot_position = Vec3::new(1.35, 1.0, 0.5);
        assert!(!collides(&world, foot_position));
        assert!(!has_support_below(&world, foot_position));
    }

    #[test]
    fn vertical_resolution_stops_close_to_floor_contact() {
        let mut world = VoxelWorld::new(WorldLayout::default());
        assert!(world.try_insert_block(IVec3::ZERO, BlockType::Stone));

        let mut foot_position = Vec3::new(0.5, 1.2, 0.5);
        let mut player = PlayerPhysics {
            velocity: Vec3::new(0.0, -12.0, 0.0),
            grounded: false,
        };

        resolve_vertical_movement(&world, &mut foot_position, &mut player, -0.4);

        assert!((foot_position.y - 1.0).abs() < 0.01);
        assert!(player.velocity.y.abs() < f32::EPSILON);
        assert!(has_support_below(&world, foot_position));
    }
}
