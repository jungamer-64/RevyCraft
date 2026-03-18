#![allow(clippy::multiple_crate_versions)]

use bevy::ecs::system::SystemParam;
use bevy::input::{
    ButtonInput,
    keyboard::KeyCode,
    mouse::{AccumulatedMouseMotion, MouseButton},
};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowResolution};
use std::collections::HashMap;

/// Marker component to identify the main camera.
#[derive(Component)]
struct MainCamera;

/// Component holding first-person camera orientation and physics state.
#[derive(Component)]
struct PlayerCamera {
    yaw: f32,
    pitch: f32,
    velocity: Vec3,
    grounded: bool,
}

/// Types of blocks available in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BlockType {
    Grass,
    Dirt,
    Stone,
}

/// Neighbor offsets for voxel adjacency.
const NEIGHBORS: [IVec3; 6] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
];

// Player size and collision constants.
const EYE_HEIGHT: f32 = 1.62;
const PLAYER_HEIGHT: f32 = 1.875;
const PLAYER_RADIUS: f32 = 0.3;

// Physics constants.
const GRAVITY: f32 = 9.81;
const JUMP_SPEED: f32 = 5.0;
const MOVE_SPEED: f32 = 10.0;

// Used by the collision sampling (capsule approximation).
// The lowest sample should not be below the player's feet (i.e. below the radius).
const PLAYER_COLLISION_SAMPLE_Y: [f32; 3] =
    [PLAYER_RADIUS, PLAYER_HEIGHT * 0.5, PLAYER_HEIGHT - 0.1];

// Maximum distance to step in a single substep for vertical movement.
const MAX_Y_STEP: f32 = 0.4;

// Prevent runaway substepping in low framerate / high velocity scenarios.
const MAX_Y_SUBSTEPS: i32 = 20;

/// Holds block data for a given coordinate.
#[derive(Clone, Copy)]
struct BlockData {
    kind: BlockType,
    entity: Option<Entity>,
}

/// Holds the voxel world state.
///
/// We store all blocks in `blocks`, but only spawn a render entity for blocks
/// that are exposed (i.e. have an empty neighbor).
#[derive(Resource, Default)]
struct VoxelWorld {
    blocks: HashMap<IVec3, BlockData>,
}

/// Current block type selected for placement.
#[derive(Resource, Clone, Copy)]
struct SelectedBlock(BlockType);

/// Shared block materials for easy lookup.
#[derive(Resource, Clone)]
struct BlockMaterials {
    grass: Handle<StandardMaterial>,
    dirt: Handle<StandardMaterial>,
    stone: Handle<StandardMaterial>,
    highlight: Handle<StandardMaterial>,
}

/// Shared cube mesh handle used for every block.
#[derive(Resource)]
struct BlockMesh(Handle<Mesh>);

/// Cached raycast hit for the block highlight.
///
/// This avoids running the voxel raycast twice per frame just for highlighting.
#[derive(Resource, Default)]
struct HighlightTarget(Option<(IVec3, IVec3)>);

/// Marker component for the block highlight indicator.
#[derive(Component)]
struct BlockHighlighter;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bevycraft -- a Minecraft Compatible Client with Rust".into(),
                resolution: WindowResolution::new(1280, 720),
                ..Default::default()
            }),
            ..default()
        }))
        .insert_resource(VoxelWorld::default())
        .insert_resource(HighlightTarget::default())
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                camera_movement_system,
                camera_look_system,
                block_edit_system,
                update_raycast_system,
                block_selection_system,
                highlight_system,
                toggle_cursor_grab,
            )
                .chain(),
        )
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut voxel_world: ResMut<VoxelWorld>,
) {
    lock_cursor(&mut cursor);

    let block_materials = create_block_materials(&mut materials);
    commands.insert_resource(block_materials.clone());

    let cube_mesh = create_cube_mesh(&mut meshes);
    commands.insert_resource(BlockMesh(cube_mesh.clone()));

    commands.insert_resource(SelectedBlock(BlockType::Grass));

    build_terrain(
        &mut commands,
        &mut voxel_world,
        &cube_mesh,
        &block_materials,
    );
    spawn_directional_light(&mut commands);
    spawn_camera(&mut commands);
    spawn_block_highlighter(&mut commands, &block_materials, &cube_mesh);
}

fn lock_cursor(cursor: &mut Query<&mut CursorOptions, With<PrimaryWindow>>) {
    if let Ok(mut cursor) = cursor.single_mut() {
        cursor.visible = false;
        cursor.grab_mode = CursorGrabMode::Locked;
    }
}

fn create_block_materials(materials: &mut ResMut<Assets<StandardMaterial>>) -> BlockMaterials {
    let grass = materials.add(StandardMaterial {
        base_color: Color::srgb(0.35, 0.7, 0.25),
        perceptual_roughness: 0.9,
        ..Default::default()
    });
    let dirt = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.35, 0.25),
        perceptual_roughness: 0.9,
        ..Default::default()
    });
    let stone = materials.add(StandardMaterial {
        base_color: Color::srgb(0.5, 0.5, 0.55),
        perceptual_roughness: 0.95,
        ..Default::default()
    });

    let highlight = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 1.0, 1.0, 0.25),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..Default::default()
    });

    BlockMaterials {
        grass,
        dirt,
        stone,
        highlight,
    }
}

fn create_cube_mesh(meshes: &mut ResMut<Assets<Mesh>>) -> Handle<Mesh> {
    meshes.add(Mesh::from(Cuboid::new(1.0, 1.0, 1.0)))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn build_terrain(
    commands: &mut Commands,
    voxel_world: &mut VoxelWorld,
    cube_mesh: &Handle<Mesh>,
    block_materials: &BlockMaterials,
) {
    let chunk_size = 16;

    // First populate the terrain data model without spawning entities.
    for x in -chunk_size..chunk_size {
        for z in -chunk_size..chunk_size {
            // Simple height function for rolling hills.
            let height = ((x as f32 * 0.3).sin() + (z as f32 * 0.3).cos()).mul_add(2.5, 4.0);
            let height = height.round() as i32;

            for y in 0..=height {
                let block_type = if y == height {
                    BlockType::Grass
                } else if y >= height - 3 {
                    BlockType::Dirt
                } else {
                    BlockType::Stone
                };

                voxel_world.blocks.insert(
                    IVec3::new(x, y, z),
                    BlockData {
                        kind: block_type,
                        entity: None,
                    },
                );
            }
        }
    }

    // Now spawn entities only for exposed blocks.
    let coords: Vec<IVec3> = voxel_world.blocks.keys().copied().collect();
    for coord in coords {
        if is_exposed(&voxel_world.blocks, coord) {
            spawn_block_entity(commands, voxel_world, cube_mesh, block_materials, coord);
        }
    }
}

fn spawn_directional_light(commands: &mut Commands) {
    commands.spawn((
        DirectionalLight {
            shadows_enabled: true,
            illuminance: 10_000.0,
            ..default()
        },
        // Compose yaw/pitch so the X rotation is not overwritten.
        Transform::from_rotation(
            Quat::from_rotation_y(-std::f32::consts::FRAC_PI_8)
                * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_4),
        ),
    ));
}

fn spawn_camera(commands: &mut Commands) {
    // Start the camera looking roughly at the center of the terrain.
    // The camera orientation is driven by `PlayerCamera` (yaw/pitch), which makes
    // it easy to clamp pitch to prevent flipping.
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

fn spawn_block_highlighter(
    commands: &mut Commands,
    materials: &BlockMaterials,
    mesh: &Handle<Mesh>,
) {
    // A semi-transparent cube to show which block is being targeted.
    let highlight_material = materials.highlight.clone();

    commands
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(highlight_material),
            Transform::from_scale(Vec3::splat(1.01)),
            Visibility::Hidden,
            BlockHighlighter,
        ))
        .insert(Name::new("BlockHighlighter"));
}

fn block_material_for_type(
    materials: &BlockMaterials,
    block_type: BlockType,
) -> Handle<StandardMaterial> {
    match block_type {
        BlockType::Grass => materials.grass.clone(),
        BlockType::Dirt => materials.dirt.clone(),
        BlockType::Stone => materials.stone.clone(),
    }
}

fn spawn_block(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
    coordinate: IVec3,
    block_type: BlockType,
) {
    // If the block already exists, do nothing.
    if world.blocks.contains_key(&coordinate) {
        return;
    }

    // Store the block (but don't necessarily spawn an entity yet).
    world.blocks.insert(
        coordinate,
        BlockData {
            kind: block_type,
            entity: None,
        },
    );

    // If this block is exposed to air, spawn a render entity for it.
    if is_exposed(&world.blocks, coordinate) {
        spawn_block_entity(commands, world, mesh, materials, coordinate);
    }

    // Placing a block can hide neighboring blocks; remove their entities if they are no longer exposed.

    // Collect neighbors that should no longer have an entity (fully occluded).
    let mut to_despawn = Vec::new();
    for offset in NEIGHBORS {
        let neighbor_coord = coordinate + offset;
        if let Some(neighbor) = world.blocks.get(&neighbor_coord) {
            if neighbor.entity.is_some() && !is_exposed(&world.blocks, neighbor_coord) {
                to_despawn.push(neighbor_coord);
            }
        }
    }

    for coord in to_despawn {
        if let Some(neighbor) = world.blocks.get_mut(&coord) {
            if let Some(entity) = neighbor.entity.take() {
                commands.entity(entity).despawn();
            }
        }
    }
}

fn spawn_block_entity(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
    coordinate: IVec3,
) {
    if let Some(block_data) = world.blocks.get_mut(&coordinate) {
        if block_data.entity.is_some() {
            return;
        }

        let material_handle = block_material_for_type(materials, block_data.kind);
        let entity = commands
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material_handle),
                Transform::from_translation(coordinate.as_vec3()),
            ))
            .id();

        block_data.entity = Some(entity);
    }
}

fn is_exposed(blocks: &HashMap<IVec3, BlockData>, coord: IVec3) -> bool {
    NEIGHBORS
        .iter()
        .any(|offset| !blocks.contains_key(&(coord + *offset)))
}

fn remove_block(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    coordinate: &IVec3,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
) {
    if let Some(block_data) = world.blocks.remove(coordinate) {
        // Despawn the entity (if any).
        if let Some(entity) = block_data.entity {
            commands.entity(entity).despawn();
        }

        // Neighboring blocks may now be exposed, so ensure they have entities.
        for offset in NEIGHBORS {
            let neighbor_coord = *coordinate + offset;
            if world.blocks.contains_key(&neighbor_coord)
                && is_exposed(&world.blocks, neighbor_coord)
            {
                spawn_block_entity(commands, world, mesh, materials, neighbor_coord);
            }
        }
    }
}

fn camera_movement_system(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    cursor_options: Query<&CursorOptions, With<PrimaryWindow>>,
    mut query: Query<(&mut Transform, &mut PlayerCamera), With<MainCamera>>,
    voxel_world: Res<VoxelWorld>,
) {
    // Only move the camera when the cursor is locked (first-person mode).
    let Ok(cursor_options) = cursor_options.single() else {
        return;
    };
    if cursor_options.grab_mode != CursorGrabMode::Locked {
        return;
    }

    let Ok((mut transform, mut player)) = query.single_mut() else {
        return;
    };

    // Treat `transform.translation` as the eye position.
    // For collision, work in terms of the player's feet position.
    let mut foot_position = transform.translation - Vec3::Y * EYE_HEIGHT;

    // Input motion in the horizontal plane.
    let forward: Vec3 = transform.forward().into();
    let forward = forward.with_y(0.).normalize_or_zero();
    let right: Vec3 = transform.right().into();
    let right = right.with_y(0.).normalize_or_zero();

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

    let desired_velocity = if input_dir.length_squared() > 0.0 {
        input_dir.normalize() * MOVE_SPEED
    } else {
        Vec3::ZERO
    };

    // Apply horizontal velocity directly (no inertia for simplicity).
    player.velocity.x = desired_velocity.x;
    player.velocity.z = desired_velocity.z;

    // Gravity + jumping.
    if player.grounded && keyboard.just_pressed(KeyCode::Space) {
        player.velocity.y = JUMP_SPEED;
        player.grounded = false;
    }

    player.velocity.y -= GRAVITY * time.delta_secs();

    // Attempt to move, resolving collisions axis-by-axis.
    let delta = player.velocity * time.delta_secs();

    // X axis
    foot_position.x += delta.x;
    if collides(&voxel_world, foot_position) {
        foot_position.x -= delta.x;
        player.velocity.x = 0.0;
    }

    // Z axis
    foot_position.z += delta.z;
    if collides(&voxel_world, foot_position) {
        foot_position.z -= delta.z;
        player.velocity.z = 0.0;
    }

    // Y axis (substepped to avoid tunneling through thin platforms).
    let steps = (delta.y.abs() / MAX_Y_STEP).ceil() as i32;
    let steps = steps.clamp(1, MAX_Y_SUBSTEPS);
    let dy_step = delta.y / (steps as f32);

    let mut grounded = false;

    for _ in 0..steps {
        foot_position.y += dy_step;
        if collides(&voxel_world, foot_position) {
            // Revert and set grounded if we were moving downward / standing.
            foot_position.y -= dy_step;
            if player.velocity.y <= 0.0 {
                grounded = true;
            }
            player.velocity.y = 0.0;
            break;
        }
    }
    player.grounded = grounded;

    // Update camera (eye) position from foot position.
    transform.translation = foot_position + Vec3::Y * EYE_HEIGHT;
}

fn collides(world: &VoxelWorld, position: Vec3) -> bool {
    // Capsule collision approximation using 3 sampled spheres (foot/mid/head).
    // This reduces corner-sticking compared to a pure AABB.
    let samples = [
        position + Vec3::Y * PLAYER_COLLISION_SAMPLE_Y[0],
        position + Vec3::Y * PLAYER_COLLISION_SAMPLE_Y[1],
        position + Vec3::Y * PLAYER_COLLISION_SAMPLE_Y[2],
    ];

    // Determine the block search bounds once.
    let min = position + Vec3::new(-PLAYER_RADIUS, 0.0, -PLAYER_RADIUS);
    let max = position + Vec3::new(PLAYER_RADIUS, PLAYER_HEIGHT, PLAYER_RADIUS);

    let x_min = min.x.floor() as i32;
    let y_min = min.y.floor() as i32;
    let z_min = min.z.floor() as i32;

    let x_max = max.x.floor() as i32;
    let y_max = max.y.floor() as i32;
    let z_max = max.z.floor() as i32;

    for x in x_min..=x_max {
        for y in y_min..=y_max {
            for z in z_min..=z_max {
                if !world.blocks.contains_key(&IVec3::new(x, y, z)) {
                    continue;
                }

                let block_min = Vec3::new(x as f32, y as f32, z as f32);
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

fn player_collides_voxel(foot_position: Vec3, voxel: IVec3) -> bool {
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

    // Use strict inequality to avoid treating perfect contact as collision
    // (prevents permanent contact during floor sliding).
    sq_dist < radius * radius
}

fn camera_look_system(
    accumulated_mouse_motion: Res<AccumulatedMouseMotion>,
    cursor_options: Query<&CursorOptions, With<PrimaryWindow>>,
    mut query: Query<(&mut Transform, &mut PlayerCamera), With<MainCamera>>,
) {
    // Ignore mouse movement when the cursor isn't grabbed.
    let Ok(cursor_options) = cursor_options.single() else {
        return;
    };
    if cursor_options.grab_mode != CursorGrabMode::Locked {
        return;
    }

    let Ok((mut transform, mut camera)) = query.single_mut() else {
        return;
    };

    let delta = accumulated_mouse_motion.delta;
    if delta == Vec2::ZERO {
        return;
    }

    let sensitivity = 0.002;
    camera.yaw -= delta.x * sensitivity;
    camera.pitch = (camera.pitch - delta.y * sensitivity).clamp(
        -std::f32::consts::FRAC_PI_2 + 0.01,
        std::f32::consts::FRAC_PI_2 - 0.01,
    );

    transform.rotation = Quat::from_rotation_y(camera.yaw) * Quat::from_rotation_x(camera.pitch);
}

fn update_raycast_system(
    cursor_options: Query<&CursorOptions, With<PrimaryWindow>>,
    camera_query: Query<&Transform, With<MainCamera>>,
    voxel_world: Res<VoxelWorld>,
    mut highlight_target: ResMut<HighlightTarget>,
) {
    // Only update the raycast target in first-person mode.
    let Ok(cursor_options) = cursor_options.single() else {
        return;
    };
    if cursor_options.grab_mode != CursorGrabMode::Locked {
        *highlight_target = HighlightTarget(None);
        return;
    }

    let Ok(camera_transform) = camera_query.single() else {
        return;
    };

    let ray_origin = camera_transform.translation;
    let ray_direction: Vec3 = camera_transform.forward().into();

    *highlight_target =
        HighlightTarget(raycast_voxel(&voxel_world, ray_origin, ray_direction, 8.0));
}

fn block_selection_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut selected_block: ResMut<SelectedBlock>,
) {
    // Use 1/2/3 to pick a block type for placement.
    if keyboard.just_pressed(KeyCode::Digit1) {
        selected_block.0 = BlockType::Grass;
    }
    if keyboard.just_pressed(KeyCode::Digit2) {
        selected_block.0 = BlockType::Dirt;
    }
    if keyboard.just_pressed(KeyCode::Digit3) {
        selected_block.0 = BlockType::Stone;
    }
}

#[derive(SystemParam)]
struct BlockEditResources<'w, 's> {
    commands: Commands<'w, 's>,
    mouse_button: Res<'w, ButtonInput<MouseButton>>,
    cursor_options: Query<'w, 's, &'static CursorOptions, With<PrimaryWindow>>,
    camera_query: Query<'w, 's, &'static Transform, With<MainCamera>>,
    block_mesh: Res<'w, BlockMesh>,
    block_materials: Res<'w, BlockMaterials>,
    selected_block: Res<'w, SelectedBlock>,
    voxel_world: ResMut<'w, VoxelWorld>,
}

fn block_edit_system(mut resources: BlockEditResources) {
    // Only allow block edits while in first-person mode.
    let Ok(cursor_options) = resources.cursor_options.single() else {
        return;
    };
    if cursor_options.grab_mode != CursorGrabMode::Locked {
        return;
    }

    let Ok(transform) = resources.camera_query.single() else {
        return;
    };

    let left_pressed = resources.mouse_button.just_pressed(MouseButton::Left);
    let right_pressed = resources.mouse_button.just_pressed(MouseButton::Right);
    if !left_pressed && !right_pressed {
        return;
    }

    let ray_origin = transform.translation;
    let foot_position = ray_origin - Vec3::Y * EYE_HEIGHT;

    // Only raycast when the player is actually trying to edit blocks.
    let mut current_raycast = raycast_voxel(
        &resources.voxel_world,
        ray_origin,
        transform.forward().into(),
        8.0,
    );

    if left_pressed {
        if let Some((hit_block, _)) = current_raycast {
            // Remove the targeted block.
            remove_block(
                &mut resources.commands,
                &mut resources.voxel_world,
                &hit_block,
                &resources.block_mesh.0,
                &resources.block_materials,
            );

            // If we also need to place a block this frame, recompute the raycast
            // using the updated world state.
        } else if right_pressed {
            // No block to remove, but right mouse is pressed, so recompute the raycast.
            current_raycast = raycast_voxel(
                &resources.voxel_world,
                ray_origin,
                transform.forward().into(),
                8.0,
            );
        }
    }

    if right_pressed && let Some((hit_block, hit_normal)) = current_raycast {
        let place_pos = hit_block + hit_normal;
        // Prevent placing blocks where the player currently is.
        if !player_collides_voxel(foot_position, place_pos) {
            spawn_block(
                &mut resources.commands,
                &mut resources.voxel_world,
                &resources.block_mesh.0,
                &resources.block_materials,
                place_pos,
                resources.selected_block.0,
            );
        }
    }
}

fn highlight_system(
    mut highlighter_query: Query<
        (&mut Transform, &mut Visibility),
        (With<BlockHighlighter>, Without<MainCamera>),
    >,
    highlight_target: Res<HighlightTarget>,
) {
    let Ok((mut highlight_transform, mut visibility)) = highlighter_query.single_mut() else {
        return;
    };

    if let Some((hit_block, _)) = highlight_target.0 {
        highlight_transform.translation = hit_block.as_vec3();
        *visibility = Visibility::Visible;
    } else {
        *visibility = Visibility::Hidden;
    }
}

fn raycast_voxel(
    world: &VoxelWorld,
    origin: Vec3,
    direction: Vec3,
    max_distance: f32,
) -> Option<(IVec3, IVec3)> {
    let dir = direction.normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }

    let mut current = origin.floor().as_ivec3();
    if world.blocks.contains_key(&current) {
        return Some((current, pick_normal(dir)));
    }

    let step = compute_step(dir);
    let t_delta = compute_t_delta(dir);
    let origin_frac = origin - origin.floor();
    let mut t_max = compute_t_max(step, origin_frac, t_delta);

    raycast_voxel_traverse(world, &mut current, step, t_delta, &mut t_max, max_distance)
}

fn pick_normal(dir: Vec3) -> IVec3 {
    let abs = Vec3::new(dir.x.abs(), dir.y.abs(), dir.z.abs());
    if abs.x >= abs.y && abs.x >= abs.z {
        IVec3::new(-dir.x.signum() as i32, 0, 0)
    } else if abs.y >= abs.x && abs.y >= abs.z {
        IVec3::new(0, -dir.y.signum() as i32, 0)
    } else {
        IVec3::new(0, 0, -dir.z.signum() as i32)
    }
}

fn compute_step(dir: Vec3) -> IVec3 {
    IVec3::new(
        if dir.x > 0.0 {
            1
        } else if dir.x < 0.0 {
            -1
        } else {
            0
        },
        if dir.y > 0.0 {
            1
        } else if dir.y < 0.0 {
            -1
        } else {
            0
        },
        if dir.z > 0.0 {
            1
        } else if dir.z < 0.0 {
            -1
        } else {
            0
        },
    )
}

fn compute_t_delta(dir: Vec3) -> Vec3 {
    Vec3::new(
        if dir.x.abs() > f32::EPSILON {
            1.0 / dir.x.abs()
        } else {
            f32::INFINITY
        },
        if dir.y.abs() > f32::EPSILON {
            1.0 / dir.y.abs()
        } else {
            f32::INFINITY
        },
        if dir.z.abs() > f32::EPSILON {
            1.0 / dir.z.abs()
        } else {
            f32::INFINITY
        },
    )
}

fn compute_t_max(step: IVec3, origin_frac: Vec3, t_delta: Vec3) -> Vec3 {
    Vec3::new(
        if step.x != 0 {
            if step.x > 0 {
                (1.0 - origin_frac.x) * t_delta.x
            } else {
                origin_frac.x * t_delta.x
            }
        } else {
            f32::INFINITY
        },
        if step.y != 0 {
            if step.y > 0 {
                (1.0 - origin_frac.y) * t_delta.y
            } else {
                origin_frac.y * t_delta.y
            }
        } else {
            f32::INFINITY
        },
        if step.z != 0 {
            if step.z > 0 {
                (1.0 - origin_frac.z) * t_delta.z
            } else {
                origin_frac.z * t_delta.z
            }
        } else {
            f32::INFINITY
        },
    )
}

fn raycast_voxel_traverse(
    world: &VoxelWorld,
    current: &mut IVec3,
    step: IVec3,
    t_delta: Vec3,
    t_max: &mut Vec3,
    max_distance: f32,
) -> Option<(IVec3, IVec3)> {
    let mut traveled = 0.0;
    while traveled <= max_distance {
        let (t_next, axis) = if t_max.x < t_max.y {
            if t_max.x < t_max.z {
                (t_max.x, 0)
            } else {
                (t_max.z, 2)
            }
        } else if t_max.y < t_max.z {
            (t_max.y, 1)
        } else {
            (t_max.z, 2)
        };

        if t_next > max_distance {
            break;
        }

        traveled = t_next;
        let normal = match axis {
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
            _ => IVec3::ZERO,
        };

        if world.blocks.contains_key(current) {
            return Some((*current, normal));
        }
    }
    None
}

fn toggle_cursor_grab(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    // Release cursor with Escape.
    if keyboard.just_pressed(KeyCode::Escape) {
        if let Ok(mut cursor) = cursor.single_mut() {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        }
    }

    // Lock cursor back in and resume first-person control when clicking.
    if mouse_button.just_pressed(MouseButton::Left) {
        if let Ok(mut cursor) = cursor.single_mut() {
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        }
    }
}
