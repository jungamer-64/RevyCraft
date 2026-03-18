use bevy::ecs::system::SystemParam;
use bevy::input::{
    ButtonInput,
    keyboard::KeyCode,
    mouse::{AccumulatedMouseMotion, MouseButton},
};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::world::VoxelWorld;

pub const EYE_HEIGHT: f32 = 1.62;
const PLAYER_HEIGHT: f32 = 1.8;
const PLAYER_RADIUS: f32 = 0.3;

const GRAVITY: f32 = 9.81;
const JUMP_SPEED: f32 = 5.0;
const MOVE_SPEED: f32 = 10.0;

const PLAYER_COLLISION_SAMPLE_Y: [f32; 3] =
    [PLAYER_RADIUS, PLAYER_HEIGHT * 0.5, PLAYER_HEIGHT - 0.1];
const MAX_Y_STEP: f32 = 0.4;
const MAX_Y_SUBSTEPS: i32 = 20;

#[derive(Component)]
pub struct MainCamera;

#[derive(Component)]
struct PlayerCamera {
    yaw: f32,
    pitch: f32,
    velocity: Vec3,
    grounded: bool,
}

pub fn spawn_camera(commands: &mut Commands) {
    let yaw = 0.0;
    let pitch = -0.2;
    let rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);

    commands
        .spawn((
            Camera3d::default(),
            Transform::from_xyz(0.0, 8.0, 15.0).with_rotation(rotation),
        ))
        .insert(MainCamera)
        .insert(PlayerCamera {
            yaw,
            pitch,
            velocity: Vec3::ZERO,
            grounded: false,
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
    query: Query<'w, 's, (&'static mut Transform, &'static mut PlayerCamera), With<MainCamera>>,
    voxel_world: Res<'w, VoxelWorld>,
}

impl CameraMovementResources<'_, '_> {
    fn run(mut self) {
        if !cursor_is_locked(&self.cursor_options) {
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
        player.grounded =
            resolve_vertical_movement(&self.voxel_world, &mut foot_position, &mut player, delta.y);
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
    query: Query<'w, 's, (&'static mut Transform, &'static mut PlayerCamera), With<MainCamera>>,
}

impl CameraLookResources<'_, '_> {
    fn run(mut self) {
        if !cursor_is_locked(&self.cursor_options) {
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

pub fn player_collides_voxel(foot_position: Vec3, voxel: IVec3) -> bool {
    let block_min = voxel.as_vec3();
    let block_max = block_min + Vec3::ONE;

    for &sample_y in &PLAYER_COLLISION_SAMPLE_Y {
        let sample_center = foot_position + Vec3::Y * sample_y;
        if sphere_aabb_collides(sample_center, PLAYER_RADIUS, block_min, block_max) {
            return true;
        }
    }

    false
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

fn cursor_is_locked(cursor_options: &Query<&CursorOptions, With<PrimaryWindow>>) -> bool {
    matches!(
        cursor_options.single(),
        Ok(cursor_options) if cursor_options.grab_mode == CursorGrabMode::Locked
    )
}

const fn apply_cursor_lock(cursor: &mut CursorOptions) {
    cursor.visible = false;
    cursor.grab_mode = CursorGrabMode::Locked;
}

const fn release_cursor(cursor: &mut CursorOptions) {
    cursor.visible = true;
    cursor.grab_mode = CursorGrabMode::None;
}

const fn eye_to_foot_position(eye_position: Vec3) -> Vec3 {
    Vec3::new(eye_position.x, eye_position.y - EYE_HEIGHT, eye_position.z)
}

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
    player: &mut PlayerCamera,
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
    player: &mut PlayerCamera,
) {
    if player.grounded && keyboard.just_pressed(KeyCode::Space) {
        player.velocity.y = JUMP_SPEED;
        player.grounded = false;
    }

    player.velocity.y -= GRAVITY * time.delta_secs();
}

fn resolve_horizontal_movement(
    world: &VoxelWorld,
    foot_position: &mut Vec3,
    player: &mut PlayerCamera,
    delta: Vec3,
) {
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
    *foot_position += axis * delta;
    if collides(world, *foot_position) {
        *foot_position -= axis * delta;
        *velocity = 0.0;
    }
}

fn resolve_vertical_movement(
    world: &VoxelWorld,
    foot_position: &mut Vec3,
    player: &mut PlayerCamera,
    delta_y: f32,
) -> bool {
    let mut grounded = false;
    let mut remaining_y = delta_y;

    for _ in 0..MAX_Y_SUBSTEPS {
        if remaining_y.abs() < f32::EPSILON {
            break;
        }

        let dy_step = remaining_y.clamp(-MAX_Y_STEP, MAX_Y_STEP);
        foot_position.y += dy_step;
        if collides(world, *foot_position) {
            foot_position.y -= dy_step;
            if player.velocity.y <= 0.0 {
                grounded = true;
            }
            player.velocity.y = 0.0;
            break;
        }

        remaining_y -= dy_step;
    }

    grounded
}

fn collides(world: &VoxelWorld, position: Vec3) -> bool {
    let samples = [
        position + Vec3::Y * PLAYER_COLLISION_SAMPLE_Y[0],
        position + Vec3::Y * PLAYER_COLLISION_SAMPLE_Y[1],
        position + Vec3::Y * PLAYER_COLLISION_SAMPLE_Y[2],
    ];

    let min = (position + Vec3::new(-PLAYER_RADIUS, 0.0, -PLAYER_RADIUS))
        .floor()
        .as_ivec3();
    let max = (position + Vec3::new(PLAYER_RADIUS, PLAYER_HEIGHT, PLAYER_RADIUS))
        .floor()
        .as_ivec3();

    for x in min.x..=max.x {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                let block_coord = IVec3::new(x, y, z);
                if !world.blocks.contains_key(&block_coord) {
                    continue;
                }

                let block_min = block_coord.as_vec3();
                let block_max = block_min + Vec3::ONE;

                for sample in samples {
                    if sphere_aabb_collides(sample, PLAYER_RADIUS, block_min, block_max) {
                        return true;
                    }
                }
            }
        }
    }

    false
}

fn sphere_aabb_collides(center: Vec3, radius: f32, aabb_min: Vec3, aabb_max: Vec3) -> bool {
    let mut sq_dist = 0.0;

    for i in 0..3 {
        let c = center[i];
        let min = aabb_min[i];
        let max = aabb_max[i];

        if c < min {
            sq_dist += (min - c) * (min - c);
        } else if c > max {
            sq_dist += (c - max) * (c - max);
        }
    }

    sq_dist < radius * radius
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_collides_when_capsule_overlaps_voxel() {
        assert!(player_collides_voxel(Vec3::new(0.2, 0.0, 0.2), IVec3::ZERO));
    }

    #[test]
    fn player_does_not_collide_when_clear_of_voxel() {
        assert!(!player_collides_voxel(
            Vec3::new(2.0, 0.0, 2.0),
            IVec3::ZERO
        ));
    }
}
