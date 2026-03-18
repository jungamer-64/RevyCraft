#![allow(clippy::multiple_crate_versions)]

pub(crate) mod cursor;
pub(crate) mod interaction;
pub(crate) mod player;
pub(crate) mod raycast;
pub(crate) mod world;

use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow, Window, WindowPlugin, WindowResolution};
use interaction::{
    HighlightTarget, SelectedBlock, block_edit_system, block_selection_system, highlight_system,
    spawn_block_highlighter,
};
use player::{
    camera_look_system, camera_movement_system, lock_cursor, spawn_camera, toggle_cursor_grab,
};
use world::{
    BlockMesh, VoxelWorld, build_terrain, create_block_materials, create_cube_mesh,
    spawn_directional_light,
};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "RevyCraft -- a Minecraft Compatible Client with Rust".into(),
                resolution: WindowResolution::new(1280, 720),
                ..Default::default()
            }),
            ..default()
        }))
        .insert_resource(VoxelWorld::default())
        .insert_resource(HighlightTarget::default())
        .init_resource::<SelectedBlock>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                camera_movement_system,
                camera_look_system,
                block_edit_system,
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
