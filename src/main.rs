use bevy::input::{
    ButtonInput,
    keyboard::KeyCode,
    mouse::{AccumulatedMouseMotion, MouseButton},
};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowResolution};
use std::collections::{HashMap, HashSet};

/// Marker component to identify the main camera.
#[derive(Component)]
struct MainCamera;

/// Holds the voxel world state (block positions + entity mapping).
#[derive(Resource, Default)]
struct VoxelWorld {
    blocks: HashSet<IVec3>,
    entities: HashMap<IVec3, Entity>,
}

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
                title: "Bevy 3D Minecraft-like Demo".into(),
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
                toggle_cursor_grab,
            ),
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
            let height = ((x as f32 * 0.3).sin() + (z as f32 * 0.3).cos()) * 2.5 + 4.0;
            let height = height.round() as i32;

            for y in 0..=height {
                spawn_block(
                    commands,
                    voxel_world,
                    cube_mesh,
                    block_materials,
                    IVec3::new(x, y, z),
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
        Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_4))
            .with_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_8)),
        GlobalTransform::default(),
    ));
}

fn spawn_camera(commands: &mut Commands) {
    commands
        .spawn((
            Camera3d::default(),
            Transform::from_xyz(0.0, 8.0, 15.0).looking_at(Vec3::new(0.0, 4.0, 0.0), Vec3::Y),
            GlobalTransform::default(),
        ))
        .insert(MainCamera);
}

fn block_material_for_height(materials: &BlockMaterials, y: i32) -> Handle<StandardMaterial> {
    // Top block in the terrain is grass, below some dirt, and deep blocks are stone.
    if y <= 1 {
        materials.stone.clone()
    } else if y <= 3 {
        materials.dirt.clone()
    } else {
        materials.grass.clone()
    }
}

fn spawn_block(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
    coordinate: IVec3,
) {
    if world.blocks.contains(&coordinate) {
        return;
    }

    let material_handle = block_material_for_height(materials, coordinate.y);
    let entity = commands
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material_handle),
            Transform::from_translation(coordinate.as_vec3()),
            GlobalTransform::default(),
        ))
        .id();

    world.blocks.insert(coordinate);
    world.entities.insert(coordinate, entity);
}

fn remove_block(commands: &mut Commands, world: &mut VoxelWorld, coordinate: IVec3) {
    if let Some(entity) = world.entities.remove(&coordinate) {
        commands.entity(entity).despawn();
        world.blocks.remove(&coordinate);
    }
}

fn camera_movement_system(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut query: Query<&mut Transform, With<MainCamera>>,
) {
    let mut transform = match query.single_mut() {
        Ok(transform) => transform,
        Err(_) => return,
    };

    let forward: Vec3 = transform.forward().into();
    let forward = forward.with_y(0.).normalize_or_zero();
    let right: Vec3 = transform.right().into();
    let right = right.with_y(0.).normalize_or_zero();

    let direction = [
        (KeyCode::KeyW, forward),
        (KeyCode::KeyS, -forward),
        (KeyCode::KeyA, -right),
        (KeyCode::KeyD, right),
        (KeyCode::Space, Vec3::Y),
        (KeyCode::ShiftLeft, -Vec3::Y),
    ]
    .iter()
    .fold(Vec3::ZERO, |acc, (key, dir)| {
        if keyboard.pressed(*key) {
            acc + *dir
        } else {
            acc
        }
    });

    if direction.length_squared() > 0.0 {
        let speed = 10.0;
        transform.translation += direction.normalize() * speed * time.delta_secs();
    }
}

fn camera_look_system(
    accumulated_mouse_motion: Res<AccumulatedMouseMotion>,
    cursor_options: Query<&CursorOptions, With<PrimaryWindow>>,
    mut query: Query<&mut Transform, With<MainCamera>>,
) {
    let mut transform = match query.single_mut() {
        Ok(transform) => transform,
        Err(_) => return,
    };

    let cursor_options = match cursor_options.single() {
        Ok(cursor_options) => cursor_options,
        Err(_) => return,
    };

    // Ignore mouse movement when the cursor isn't grabbed.
    if cursor_options.grab_mode != CursorGrabMode::Locked {
        return;
    }

    let delta = accumulated_mouse_motion.delta;
    if delta == Vec2::ZERO {
        return;
    }

    let sensitivity = 0.002;
    let yaw = Quat::from_rotation_y(-delta.x * sensitivity);
    let pitch = Quat::from_rotation_x(-delta.y * sensitivity);

    transform.rotation = (transform.rotation * yaw) * pitch;
}

fn block_edit_system(
    mut commands: Commands,
    mouse_button: Res<ButtonInput<MouseButton>>,
    camera_query: Query<&Transform, With<MainCamera>>,
    block_mesh: Res<BlockMesh>,
    block_materials: Res<BlockMaterials>,
    mut voxel_world: ResMut<VoxelWorld>,
) {
    let transform = match camera_query.single() {
        Ok(transform) => transform,
        Err(_) => return,
    };

    let ray_origin = transform.translation;
    let ray_direction = transform.forward().into();

    // Raycast into the voxel grid to find which block we're looking at.
    if let Some((hit, air)) = raycast_voxel(&voxel_world, ray_origin, ray_direction, 8.0, 0.1) {
        if mouse_button.just_pressed(MouseButton::Left) {
            // Remove a block.
            remove_block(&mut commands, &mut voxel_world, hit);
        }
        if mouse_button.just_pressed(MouseButton::Right) {
            // Place a block in the closest empty space.
            spawn_block(
                &mut commands,
                &mut voxel_world,
                &block_mesh.0,
                &block_materials,
                air,
            );
        }
    }
}

fn raycast_voxel(
    world: &VoxelWorld,
    origin: Vec3,
    direction: Vec3,
    max_distance: f32,
    step: f32,
) -> Option<(IVec3, IVec3)> {
    let mut previous = IVec3::new(
        origin.x.floor() as i32,
        origin.y.floor() as i32,
        origin.z.floor() as i32,
    );

    let dir = direction.normalize_or_zero();
    let steps = (max_distance / step).ceil() as usize;
    for i in 0..=steps {
        let dist = i as f32 * step;
        let position = origin + dir * dist;
        let coord = IVec3::new(
            position.x.floor() as i32,
            position.y.floor() as i32,
            position.z.floor() as i32,
        );

        if world.blocks.contains(&coord) {
            return Some((coord, previous));
        }

        previous = coord;
    }

    None
}

fn toggle_cursor_grab(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if keyboard.just_pressed(KeyCode::Escape) {
        if let Ok(mut cursor) = cursor.single_mut() {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        }
    }
    if keyboard.just_pressed(KeyCode::Enter) {
        if let Ok(mut cursor) = cursor.single_mut() {
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        }
    }
}
