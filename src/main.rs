#![allow(clippy::multiple_crate_versions)]

pub(crate) mod cursor;
pub(crate) mod interaction;
pub(crate) mod player;
pub(crate) mod raycast;
pub(crate) mod world;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow, Window, WindowPlugin, WindowResolution};
use interaction::{
    HighlightTarget, SelectedBlock, block_edit_system, block_selection_system, highlight_system,
    spawn_block_highlighter, update_highlight_target_post_edit_system,
    update_highlight_target_pre_edit_system,
};
use player::{
    camera_look_system, camera_movement_system, lock_cursor, spawn_camera, toggle_cursor_grab,
};
use world::{
    BlockEntityIndex, BlockMesh, ChunkLoadSettings, RenderSyncQueue, TerrainSettings, VoxelWorld,
    WorldSaveDirectory, WorldSeed, create_block_materials, create_cube_mesh,
    initialize_visible_world, save_loaded_chunks_on_exit_system, spawn_directional_light,
    sync_block_render_system, sync_visible_chunks_system,
};

#[derive(SystemParam)]
struct SetupResources<'w, 's> {
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    cursor: Query<'w, 's, &'static mut CursorOptions, With<PrimaryWindow>>,
    voxel_world: ResMut<'w, VoxelWorld>,
    render_sync_queue: ResMut<'w, RenderSyncQueue>,
    world_seed: Res<'w, WorldSeed>,
    terrain_settings: Res<'w, TerrainSettings>,
    chunk_load_settings: Res<'w, ChunkLoadSettings>,
    world_save_directory: Res<'w, WorldSaveDirectory>,
}

impl SetupResources<'_, '_> {
    fn run(mut self) {
        lock_cursor(&mut self.cursor);

        let block_materials = create_block_materials(&mut self.materials);
        self.commands.insert_resource(block_materials.clone());

        let cube_mesh = create_cube_mesh(&mut self.meshes);
        self.commands.insert_resource(BlockMesh(cube_mesh.clone()));

        spawn_directional_light(&mut self.commands);
        spawn_camera(&mut self.commands);
        initialize_visible_world(
            &mut self.voxel_world,
            &mut self.render_sync_queue,
            *self.world_seed,
            &self.terrain_settings,
            &self.chunk_load_settings,
            &self.world_save_directory,
        );
        spawn_block_highlighter(&mut self.commands, &block_materials, &cube_mesh);
    }
}

fn main() {
    let chunk_load_settings = ChunkLoadSettings::from_env();
    let world_seed = WorldSeed::from_env();
    let terrain_settings = TerrainSettings::from_env();
    let world_save_directory = WorldSaveDirectory::from_seed(world_seed);

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "RevyCraft -- a Minecraft-like Client with Rust".into(),
                resolution: WindowResolution::new(1280, 720),
                ..Default::default()
            }),
            ..default()
        }))
        .insert_resource(chunk_load_settings)
        .insert_resource(VoxelWorld::new(chunk_load_settings.layout()))
        .insert_resource(BlockEntityIndex::default())
        .insert_resource(RenderSyncQueue::default())
        .insert_resource(HighlightTarget::default())
        .init_resource::<SelectedBlock>()
        .insert_resource(world_seed)
        .insert_resource(terrain_settings)
        .insert_resource(world_save_directory)
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                sync_visible_chunks_system,
                camera_movement_system,
                camera_look_system,
                update_highlight_target_pre_edit_system,
                block_edit_system,
                sync_block_render_system,
                update_highlight_target_post_edit_system,
                block_selection_system,
                highlight_system,
                toggle_cursor_grab,
            )
                .chain(),
        )
        .add_systems(Last, save_loaded_chunks_on_exit_system)
        .run();
}

fn setup(resources: SetupResources) {
    resources.run();
}
