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
    INITIAL_CAMERA_EYE_POSITION, camera_look_system, camera_movement_system, lock_cursor,
    spawn_camera, toggle_cursor_grab,
};
use world::{
    BlockEntityIndex, BlockMesh, ChunkLoadSettings, RenderAnchor, RenderOriginRootEntity,
    RenderSyncQueue, TerrainSettings, VoxelWorld, WorldSaveDirectory, WorldSeed,
    create_block_materials, create_cube_mesh, initialize_visible_world,
    save_loaded_chunks_on_exit_system, spawn_directional_light, spawn_render_origin_root,
    sync_block_render_system, sync_block_world_transforms_system, sync_render_anchor_system,
    sync_render_origin_root_system, sync_visible_chunks_system,
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
    render_anchor: Res<'w, RenderAnchor>,
}

impl SetupResources<'_, '_> {
    fn run(mut self) {
        lock_cursor(&mut self.cursor);

        let block_materials = create_block_materials(&mut self.materials);
        self.commands.insert_resource(block_materials.clone());

        let cube_mesh = create_cube_mesh(&mut self.meshes);
        self.commands.insert_resource(BlockMesh(cube_mesh.clone()));

        let initial_camera_translation = world::render::world_position_to_render_translation(
            INITIAL_CAMERA_EYE_POSITION,
            *self.render_anchor,
            self.voxel_world.layout(),
        );
        let render_origin_root =
            spawn_render_origin_root(&mut self.commands, initial_camera_translation);
        self.commands
            .insert_resource(RenderOriginRootEntity(render_origin_root));

        spawn_directional_light(&mut self.commands, render_origin_root);
        spawn_camera(
            &mut self.commands,
            render_origin_root,
            initial_camera_translation,
        );
        initialize_visible_world(
            &mut self.voxel_world,
            &mut self.render_sync_queue,
            *self.world_seed,
            &self.terrain_settings,
            &self.chunk_load_settings,
            &self.world_save_directory,
        );
        spawn_block_highlighter(
            &mut self.commands,
            &block_materials,
            &cube_mesh,
            render_origin_root,
        );
    }
}

fn main() {
    let chunk_load_settings = ChunkLoadSettings::from_env();
    let world_seed = WorldSeed::from_env();
    let terrain_settings = TerrainSettings::from_env();
    let world_save_directory = WorldSaveDirectory::from_seed(world_seed);
    let render_anchor = RenderAnchor::from_world_position(
        INITIAL_CAMERA_EYE_POSITION,
        chunk_load_settings.layout(),
    )
    .expect("initial camera position should be within supported world coordinate range");

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "RevyCraft -- a Minecraft Compatible Client with Bevy Engine".into(),
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
        .insert_resource(render_anchor)
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                camera_movement_system,
                camera_look_system,
                sync_render_anchor_system,
                sync_visible_chunks_system,
                update_highlight_target_pre_edit_system,
                block_edit_system,
                sync_block_render_system,
                sync_block_world_transforms_system,
                update_highlight_target_post_edit_system,
                block_selection_system,
                highlight_system,
                sync_render_origin_root_system,
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
