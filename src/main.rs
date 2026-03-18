#![allow(clippy::multiple_crate_versions)]

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

/// Component holding first-person camera orientation state.
#[derive(Component)]
struct PlayerCamera {
    yaw: f32,
    pitch: f32,
}

/// Types of blocks available in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BlockType {
    Grass,
    Dirt,
    Stone,
}

/// Holds the voxel world state (block positions + entity mapping).
///
/// Only the entity map is needed: `hashmap.contains_key` replaces the old
/// redundant `blocks` set.
#[derive(Resource, Default)]
struct VoxelWorld {
    entities: HashMap<IVec3, (Entity, BlockType)>,
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
}

/// Shared cube mesh handle used for every block.
#[derive(Resource)]
struct BlockMesh(Handle<Mesh>);

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Minecraft with Rust Demo".into(),
                resolution: WindowResolution::new(1280, 720),
                ..Default::default()
            }),
            ..default()
        }))
        .insert_resource(VoxelWorld::default())
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                camera_movement_system,
                camera_look_system,
                block_edit_system,
                block_selection_system,
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
}

fn lock_cursor(cursor: &mut Query<&mut CursorOptions, With<PrimaryWindow>>) {
    if let Ok(mut cursor) = cursor.single_mut() {
        cursor.visible = false;
        cursor.grab_mode = CursorGrabMode::Locked;
    }
}

fn create_block_materials(materials: &mut ResMut<Assets<StandardMaterial>>) -> BlockMaterials {
    BlockMaterials {
        grass: materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.7, 0.25),
            perceptual_roughness: 0.9,
            ..Default::default()
        }),
        dirt: materials.add(StandardMaterial {
            base_color: Color::srgb(0.45, 0.35, 0.25),
            perceptual_roughness: 0.9,
            ..Default::default()
        }),
        stone: materials.add(StandardMaterial {
            base_color: Color::srgb(0.5, 0.5, 0.55),
            perceptual_roughness: 0.95,
            ..Default::default()
        }),
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

                spawn_block(
                    commands,
                    voxel_world,
                    cube_mesh,
                    block_materials,
                    IVec3::new(x, y, z),
                    block_type,
                );
            }
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
        .insert(PlayerCamera { yaw, pitch });
}

fn block_material_for_type(materials: &BlockMaterials, block_type: BlockType) -> Handle<StandardMaterial> {
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
    if world.entities.contains_key(&coordinate) {
        return;
    }

    let material_handle = block_material_for_type(materials, block_type);
    let entity = commands
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material_handle),
            Transform::from_translation(coordinate.as_vec3()),
        ))
        .id();

    world.entities.insert(coordinate, (entity, block_type));
}

fn remove_block(commands: &mut Commands, world: &mut VoxelWorld, coordinate: &IVec3) {
    if let Some((entity, _)) = world.entities.remove(coordinate) {
        commands.entity(entity).despawn();
    }
}

fn camera_movement_system(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    cursor_options: Query<&CursorOptions, With<PrimaryWindow>>,
    mut query: Query<&mut Transform, With<MainCamera>>,
) {
    // Only move the camera when the cursor is locked (first-person mode).
    let Ok(cursor_options) = cursor_options.single() else {
        return;
    };
    if cursor_options.grab_mode != CursorGrabMode::Locked {
        return;
    }

    let Ok(mut transform) = query.single_mut() else {
        return;
    };

    let forward: Vec3 = transform.forward().into();
    let forward = forward.with_y(0.).normalize_or_zero();
    let right: Vec3 = transform.right().into();
    let right = right.with_y(0.).normalize_or_zero();

    let mut direction = Vec3::ZERO;
    if keyboard.pressed(KeyCode::KeyW) {
        direction += forward;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        direction -= forward;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        direction -= right;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        direction += right;
    }
    if keyboard.pressed(KeyCode::Space) {
        direction += Vec3::Y;
    }
    if keyboard.pressed(KeyCode::ShiftLeft) {
        direction -= Vec3::Y;
    }

    if direction.length_squared() > 0.0 {
        let speed = 10.0;
        transform.translation += direction.normalize() * speed * time.delta_secs();
    }
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

fn block_edit_system(
    mut commands: Commands,
    mouse_button: Res<ButtonInput<MouseButton>>,
    cursor_options: Query<&CursorOptions, With<PrimaryWindow>>,
    camera_query: Query<&Transform, With<MainCamera>>,
    block_mesh: Res<BlockMesh>,
    block_materials: Res<BlockMaterials>,
    selected_block: Res<SelectedBlock>,
    mut voxel_world: ResMut<VoxelWorld>,
) {
    // Only allow block edits while in first-person mode.
    let Ok(cursor_options) = cursor_options.single() else {
        return;
    };
    if cursor_options.grab_mode != CursorGrabMode::Locked {
        return;
    }

    let Ok(transform) = camera_query.single() else {
        return;
    };

    let ray_origin = transform.translation;
    let ray_direction: Vec3 = transform.forward().into();

    // Raycast into the voxel grid to find which block we're looking at.
    if let Some((hit_block, hit_normal)) =
        raycast_voxel(&voxel_world, ray_origin, ray_direction, 8.0)
    {
        if mouse_button.just_pressed(MouseButton::Left) {
            // Remove the targeted block.
            remove_block(&mut commands, &mut voxel_world, &hit_block);
        }

        if mouse_button.just_pressed(MouseButton::Right) {
            // Place a block next to the face we hit.
            let place_pos = hit_block + hit_normal;

            // Prevent placing blocks where the player currently is (feet and one block above).
            let camera_voxel = ray_origin.floor().as_ivec3();
            if place_pos == camera_voxel || place_pos == camera_voxel + IVec3::Y {
                return;
            }

            spawn_block(
                &mut commands,
                &mut voxel_world,
                &block_mesh.0,
                &block_materials,
                place_pos,
                selected_block.0,
            );
        }
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

    // Starting voxel coordinate.
    let mut current = origin.floor().as_ivec3();

    // If we start inside a block, report it immediately.
    if world.entities.contains_key(&current) {
        let normal = -dir.signum().as_ivec3();
        return Some((current, normal));
    }

    let step = IVec3::new(
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
    );

    let t_delta = Vec3::new(
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
    );

    let origin_frac = origin - origin.floor();
    let mut t_max = Vec3::new(
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
    );

    // Traverse voxels along the ray until we hit something or exceed max distance.
    let mut traveled = 0.0;
    while traveled <= max_distance {
        // Determine which axis we will step along next.
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

        // Step into the next voxel.
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

        if world.entities.contains_key(&current) {
            return Some((current, normal));
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
