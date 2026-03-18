use super::generation::{
    TerrainNoise, classify_biome, generate_chunk, should_carve_cave, surface_height_at,
};
use super::render::world_position_to_render_translation;
use super::render::{BlockWorldCoord, RenderOriginRoot, RenderSyncState, render_sync_state_at};
use super::save::{chunk_path, load_chunk, save_chunk};
use super::*;
use crate::player::{MainCamera, WorldPosition};
use bevy::math::DVec3;
use bevy::transform::TransformPlugin;
use std::collections::HashMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn test_block_materials() -> BlockMaterials {
    BlockMaterials {
        grass: Handle::default(),
        dirt: Handle::default(),
        stone: Handle::default(),
        highlight: Handle::default(),
    }
}

fn test_root_dir(label: &str) -> PathBuf {
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    PathBuf::from("target").join("test-temp").join(format!(
        "revycraft-{label}-{}-{unique_suffix}",
        std::process::id()
    ))
}

fn insert_render_origin_root(app: &mut App, translation: Vec3) -> Entity {
    let render_origin_root = app
        .world_mut()
        .spawn((
            RenderOriginRoot,
            Transform::from_translation(translation),
            GlobalTransform::default(),
            Visibility::Visible,
            InheritedVisibility::VISIBLE,
            ViewVisibility::default(),
        ))
        .id();
    app.insert_resource(RenderOriginRootEntity(render_origin_root));
    app.insert_resource(RenderAnchor {
        chunk: ChunkCoord::new(0, 0),
    });
    render_origin_root
}

fn top_block_at(
    chunk_data: &ChunkData,
    layout: WorldLayout,
    local_x: i32,
    local_z: i32,
) -> Option<BlockType> {
    (0..layout.vertical_span()).rev().find_map(|local_y| {
        chunk_data
            .blocks
            .get(&IVec3::new(local_x, local_y, local_z))
            .map(|block_data| block_data.kind)
    })
}

fn setup_streaming_app(temp_dir: PathBuf) -> (App, WorldLayout) {
    let mut app = App::new();
    let chunk_load_settings = ChunkLoadSettings {
        view_radius: 0,
        unload_radius: 1,
        ..ChunkLoadSettings::default()
    };
    let layout = chunk_load_settings.layout();

    app.insert_resource(VoxelWorld::new(layout));
    app.insert_resource(BlockEntityIndex::default());
    app.insert_resource(RenderSyncQueue::default());
    app.insert_resource(BlockMesh(Handle::default()));
    app.insert_resource(test_block_materials());
    let _ = insert_render_origin_root(&mut app, Vec3::ZERO);
    app.insert_resource(WorldSeed(7));
    app.insert_resource(TerrainSettings::default());
    app.insert_resource(chunk_load_settings);
    app.insert_resource(WorldSaveDirectory(temp_dir));
    app.add_systems(
        Update,
        (
            sync_render_anchor_system,
            sync_visible_chunks_system,
            sync_block_render_system,
            sync_block_world_transforms_system,
            sync_render_origin_root_system,
        )
            .chain(),
    );
    app.world_mut().spawn((
        MainCamera,
        Transform::from_translation(Vec3::new(0.5, 36.0, 0.5)),
        WorldPosition(DVec3::new(0.5, 36.0, 0.5)),
    ));

    (app, layout)
}

fn move_main_camera(app: &mut App, translation: DVec3) {
    let mut camera = app
        .world_mut()
        .query_filtered::<(&mut Transform, &mut WorldPosition), With<MainCamera>>();
    let (mut transform, mut world_position) = camera
        .single_mut(app.world_mut())
        .expect("main camera should exist");
    world_position.0 = translation;
    transform.translation = translation.as_vec3();
}

#[test]
fn block_is_exposed_when_any_face_touches_air() {
    let mut world = VoxelWorld::default();
    let _ = world.try_insert_block(IVec3::ZERO, BlockType::Stone);

    assert!(world.is_exposed(IVec3::ZERO));
}

#[test]
fn block_is_not_exposed_when_fully_enclosed() {
    let mut world = VoxelWorld::default();
    let _ = world.try_insert_block(IVec3::ZERO, BlockType::Stone);
    for offset in NEIGHBORS {
        let _ = world.try_insert_block(offset, BlockType::Stone);
    }

    assert!(!world.is_exposed(IVec3::ZERO));
}

#[test]
fn render_sync_queue_marks_coordinate_and_neighbors_once() {
    let mut queue = RenderSyncQueue::default();
    queue.mark_with_neighbors(IVec3::ZERO);
    queue.mark_with_neighbors(IVec3::ZERO);

    assert_eq!(queue.0.len(), 7);
    assert!(queue.0.contains(&IVec3::ZERO));
    for offset in NEIGHBORS {
        assert!(queue.0.contains(&offset));
    }

    let drained_count = queue.drain().count();
    assert_eq!(drained_count, 7);
    assert!(queue.0.is_empty());
}

#[test]
fn render_sync_state_reports_missing_hidden_and_exposed() {
    let mut missing_world = VoxelWorld::default();
    assert_eq!(
        render_sync_state_at(&missing_world, IVec3::ZERO),
        RenderSyncState::Missing
    );

    let _ = missing_world.try_insert_block(IVec3::ZERO, BlockType::Grass);
    assert_eq!(
        render_sync_state_at(&missing_world, IVec3::ZERO),
        RenderSyncState::Exposed(BlockType::Grass)
    );

    let mut hidden_world = VoxelWorld::default();
    let _ = hidden_world.try_insert_block(IVec3::ZERO, BlockType::Stone);
    for offset in NEIGHBORS {
        let _ = hidden_world.try_insert_block(offset, BlockType::Stone);
    }

    assert_eq!(
        render_sync_state_at(&hidden_world, IVec3::ZERO),
        RenderSyncState::Hidden
    );
}

#[test]
fn generate_chunk_is_deterministic_for_seed_and_coordinate() {
    let seed = WorldSeed(42);
    let layout = WorldLayout::default();
    let settings = TerrainSettings::default();

    let first = generate_chunk(seed, ChunkCoord::new(2, -1), &settings, layout);
    let second = generate_chunk(seed, ChunkCoord::new(2, -1), &settings, layout);

    assert_eq!(first, second);
}

#[test]
fn generated_terrain_varies_by_biome_and_height_profile() {
    let seed = WorldSeed(17);
    let settings = TerrainSettings::default();
    let noise = TerrainNoise::new(seed);
    let layout = WorldLayout::default();
    let mut saw_grass_surface = false;
    let mut saw_stone_surface = false;
    let mut min_height = i32::MAX;
    let mut max_height = i32::MIN;

    for world_x in (-96..=96).step_by(6_usize) {
        for world_z in (-96..=96).step_by(6_usize) {
            let biome = classify_biome(&noise, &settings, world_x, world_z);
            let surface_height =
                surface_height_at(&noise, &settings, layout, biome, world_x, world_z);
            min_height = min_height.min(surface_height);
            max_height = max_height.max(surface_height);

            match TerrainSettings::block_type_at_height(biome, surface_height, surface_height) {
                BlockType::Grass => saw_grass_surface = true,
                BlockType::Stone => saw_stone_surface = true,
                BlockType::Dirt => {}
            }
        }
    }

    assert!(saw_grass_surface);
    assert!(saw_stone_surface);
    assert!(max_height - min_height >= 8);
}

#[test]
fn cave_generation_preserves_surface_buffer() {
    let seed = WorldSeed(99);
    let layout = WorldLayout::default();
    let settings = TerrainSettings::default();
    let noise = TerrainNoise::new(seed);

    for world_x in -4..=4 {
        for world_z in -4..=4 {
            let biome = classify_biome(&noise, &settings, world_x, world_z);
            let surface_height =
                surface_height_at(&noise, &settings, layout, biome, world_x, world_z);

            for world_y in surface_height - settings.cave_surface_buffer..=surface_height {
                assert!(!should_carve_cave(
                    &noise,
                    &settings,
                    world_x,
                    world_y,
                    world_z,
                    surface_height,
                ));
            }
        }
    }
}

#[test]
fn world_coordinate_mapping_crosses_chunk_boundaries_correctly() {
    let layout = WorldLayout::default();

    let positive = layout.local_from_world(IVec3::new(16, 0, 16));
    let negative = layout.local_from_world(IVec3::new(-1, 0, -1));

    assert_eq!(
        positive,
        Some((ChunkCoord::new(1, 1), IVec3::new(0, 24, 0)))
    );
    assert_eq!(
        negative,
        Some((ChunkCoord::new(-1, -1), IVec3::new(15, 24, 15)))
    );
}

#[test]
fn chunk_coord_from_far_world_position_preserves_precision() {
    let layout = WorldLayout::default();
    let position = DVec3::new(16_777_217.25, 0.0, -16_777_232.75);

    let chunk_coord = ChunkCoord::from_world_position(position, layout);

    assert_eq!(chunk_coord, ChunkCoord::new(1_048_576, -1_048_578));
}

#[test]
fn try_insert_and_remove_work_across_chunk_boundaries() {
    let mut world = VoxelWorld::default();
    let left = IVec3::new(-1, 0, -1);
    let right = IVec3::new(16, 0, 16);

    assert!(world.try_insert_block(left, BlockType::Grass));
    assert!(world.try_insert_block(right, BlockType::Stone));
    assert_eq!(world.block_kind(left), Some(BlockType::Grass));
    assert_eq!(world.block_kind(right), Some(BlockType::Stone));
    assert_eq!(
        world.remove_block(left).map(|block_data| block_data.kind),
        Some(BlockType::Grass)
    );
    assert!(!world.contains_block(left));
}

#[test]
fn save_and_load_round_trip_chunk_data() {
    let root_dir = test_root_dir("save-load");
    let layout = WorldLayout::default();
    let chunk_coord = ChunkCoord::new(1, -2);
    let mut chunk_data = ChunkData::loaded(HashMap::new(), true);
    chunk_data
        .blocks
        .insert(IVec3::new(0, 24, 0), BlockData::new(BlockType::Stone));
    chunk_data
        .blocks
        .insert(IVec3::new(5, 30, 7), BlockData::new(BlockType::Grass));

    save_chunk(&root_dir, chunk_coord, &chunk_data, layout).unwrap_or_else(|error| {
        panic!("failed to save chunk for round-trip test: {error}");
    });
    let loaded = load_chunk(&root_dir, chunk_coord, layout).unwrap_or_else(|error| {
        panic!("failed to load chunk for round-trip test: {error}");
    });

    assert_eq!(loaded, Some(chunk_data));

    let _ = fs::remove_dir_all(root_dir);
}

#[test]
fn sync_system_spawns_entity_for_dirty_exposed_block() {
    let mut app = App::new();
    app.insert_resource(VoxelWorld::default());
    app.insert_resource(BlockEntityIndex::default());
    app.insert_resource(RenderSyncQueue::default());
    app.insert_resource(BlockMesh(Handle::default()));
    app.insert_resource(test_block_materials());
    let _ = insert_render_origin_root(&mut app, Vec3::ZERO);
    app.add_systems(Update, sync_block_render_system);

    {
        let mut world = app.world_mut().resource_mut::<VoxelWorld>();
        let _ = world.try_insert_block(IVec3::ZERO, BlockType::Stone);
    }
    app.world_mut()
        .resource_mut::<RenderSyncQueue>()
        .mark(IVec3::ZERO);

    app.update();

    assert_eq!(app.world().resource::<BlockEntityIndex>().0.len(), 1);
}

#[test]
fn sync_system_despawns_entity_for_dirty_hidden_block() {
    let mut app = App::new();
    app.insert_resource(VoxelWorld::default());
    app.insert_resource(BlockEntityIndex::default());
    app.insert_resource(RenderSyncQueue::default());
    app.insert_resource(BlockMesh(Handle::default()));
    app.insert_resource(test_block_materials());
    let _ = insert_render_origin_root(&mut app, Vec3::ZERO);
    app.add_systems(Update, sync_block_render_system);

    {
        let mut world = app.world_mut().resource_mut::<VoxelWorld>();
        let _ = world.try_insert_block(IVec3::ZERO, BlockType::Stone);
    }
    app.world_mut()
        .resource_mut::<RenderSyncQueue>()
        .mark(IVec3::ZERO);
    app.update();

    {
        let mut world = app.world_mut().resource_mut::<VoxelWorld>();
        for offset in NEIGHBORS {
            let _ = world.try_insert_block(offset, BlockType::Stone);
        }
    }
    app.world_mut()
        .resource_mut::<RenderSyncQueue>()
        .mark(IVec3::ZERO);
    app.update();

    assert!(app.world().resource::<BlockEntityIndex>().0.is_empty());
}

#[test]
fn sync_system_reveals_neighbor_after_block_removal() {
    let mut app = App::new();
    app.insert_resource(VoxelWorld::default());
    app.insert_resource(BlockEntityIndex::default());
    app.insert_resource(RenderSyncQueue::default());
    app.insert_resource(BlockMesh(Handle::default()));
    app.insert_resource(test_block_materials());
    let _ = insert_render_origin_root(&mut app, Vec3::ZERO);
    app.add_systems(Update, sync_block_render_system);

    let hidden_target = IVec3::X;
    {
        let mut world = app.world_mut().resource_mut::<VoxelWorld>();
        let _ = world.try_insert_block(hidden_target, BlockType::Stone);
        for offset in NEIGHBORS {
            let _ = world.try_insert_block(hidden_target + offset, BlockType::Stone);
        }
    }
    app.world_mut()
        .resource_mut::<RenderSyncQueue>()
        .mark(hidden_target);
    app.update();

    assert!(
        !app.world()
            .resource::<BlockEntityIndex>()
            .0
            .contains_key(&hidden_target)
    );

    {
        let mut world = app.world_mut().resource_mut::<VoxelWorld>();
        let _ = world.remove_block(IVec3::ZERO);
    }
    app.world_mut()
        .resource_mut::<RenderSyncQueue>()
        .mark_with_neighbors(IVec3::ZERO);
    app.update();

    assert!(
        app.world()
            .resource::<BlockEntityIndex>()
            .0
            .contains_key(&hidden_target)
    );
}

#[test]
fn chunk_streaming_saves_modified_chunks_and_reloads_them() {
    let root_dir = test_root_dir("streaming");
    let (mut app, layout) = setup_streaming_app(root_dir.clone());

    app.update();
    assert_eq!(app.world().resource::<VoxelWorld>().loaded_chunk_count(), 1);

    let placed_block = IVec3::new(0, layout.vertical_max() - 1, 0);
    {
        let mut world = app.world_mut().resource_mut::<VoxelWorld>();
        let _ = world.try_insert_block(placed_block, BlockType::Stone);
    }
    app.world_mut()
        .resource_mut::<RenderSyncQueue>()
        .mark_with_neighbors(placed_block);
    app.update();

    move_main_camera(&mut app, DVec3::new(48.5, 36.0, 0.5));
    app.update();

    assert_eq!(app.world().resource::<VoxelWorld>().loaded_chunk_count(), 1);
    assert!(chunk_path(&root_dir, ChunkCoord::new(0, 0)).exists());

    move_main_camera(&mut app, DVec3::new(0.5, 36.0, 0.5));
    app.update();

    assert!(
        app.world()
            .resource::<VoxelWorld>()
            .contains_block(placed_block)
    );
    assert!(
        app.world()
            .resource::<BlockEntityIndex>()
            .0
            .contains_key(&placed_block)
    );

    let _ = fs::remove_dir_all(root_dir);
}

#[test]
fn chunk_load_marks_entities_for_render_sync() {
    let root_dir = test_root_dir("render-sync");
    let (mut app, _) = setup_streaming_app(root_dir.clone());

    app.update();

    assert!(!app.world().resource::<BlockEntityIndex>().0.is_empty());

    let _ = fs::remove_dir_all(root_dir);
}

#[test]
fn top_block_sampler_can_find_surface_material() {
    let seed = WorldSeed(1234);
    let layout = WorldLayout::default();
    let settings = TerrainSettings::default();
    let chunk_data = generate_chunk(seed, ChunkCoord::new(0, 0), &settings, layout);

    assert!(matches!(
        top_block_at(&chunk_data, layout, 0, 0),
        Some(BlockType::Grass | BlockType::Stone)
    ));
}

#[test]
fn render_origin_root_tracks_negative_camera_world_position() {
    let mut app = App::new();
    app.insert_resource(VoxelWorld::default());
    let _ = insert_render_origin_root(&mut app, Vec3::ZERO);
    app.world_mut().spawn((
        MainCamera,
        Transform::from_translation(Vec3::new(8.0, 3.0, -2.0)),
        WorldPosition(DVec3::new(8.0, 3.0, -2.0)),
    ));
    app.add_systems(Update, sync_render_origin_root_system);

    app.update();

    let mut root_query = app
        .world_mut()
        .query_filtered::<&Transform, With<RenderOriginRoot>>();
    let root_transform = root_query
        .single(app.world_mut())
        .expect("render origin root should exist");
    assert_eq!(root_transform.translation, Vec3::new(-8.0, -3.0, 2.0));
}

#[test]
fn block_global_transform_becomes_camera_relative() {
    let mut app = App::new();
    app.add_plugins(TransformPlugin);
    app.insert_resource(VoxelWorld::default());
    app.insert_resource(BlockEntityIndex::default());
    app.insert_resource(RenderSyncQueue::default());
    app.insert_resource(BlockMesh(Handle::default()));
    app.insert_resource(test_block_materials());
    let render_origin_root = insert_render_origin_root(&mut app, Vec3::ZERO);
    app.add_systems(
        Update,
        (
            sync_render_anchor_system,
            sync_block_render_system,
            sync_block_world_transforms_system,
            sync_render_origin_root_system,
        )
            .chain(),
    );
    app.add_systems(Startup, move |mut commands: Commands| {
        commands.entity(render_origin_root).with_children(|parent| {
            parent.spawn((
                MainCamera,
                Camera3d::default(),
                Transform::from_translation(Vec3::new(24.0, 0.0, 0.0)),
                WorldPosition(DVec3::new(24.0, 0.0, 0.0)),
            ));
        });
    });

    {
        let mut world = app.world_mut().resource_mut::<VoxelWorld>();
        let _ = world.try_insert_block(IVec3::new(26, 0, 0), BlockType::Stone);
    }
    app.world_mut()
        .resource_mut::<RenderSyncQueue>()
        .mark(IVec3::new(26, 0, 0));

    app.update();

    let block_entity = *app
        .world()
        .resource::<BlockEntityIndex>()
        .0
        .get(&IVec3::new(26, 0, 0))
        .expect("block entity should be indexed");
    let global_transform = app
        .world()
        .get::<GlobalTransform>(block_entity)
        .expect("block entity should have a global transform");
    assert_eq!(global_transform.translation(), Vec3::new(2.0, 0.0, 0.0));

    let mut camera_query = app
        .world_mut()
        .query_filtered::<(&Transform, &WorldPosition), With<MainCamera>>();
    let (camera_transform, world_position) = camera_query
        .single(app.world_mut())
        .expect("camera should exist");
    assert_eq!(world_position.0, DVec3::new(24.0, 0.0, 0.0));
    assert_eq!(camera_transform.translation, Vec3::new(8.0, 0.0, 0.0));
}

#[test]
fn block_local_translation_rebases_when_render_anchor_changes() {
    let mut app = App::new();
    app.insert_resource(VoxelWorld::default());
    app.insert_resource(RenderAnchor {
        chunk: ChunkCoord::new(0, 0),
    });
    app.world_mut()
        .spawn((BlockWorldCoord(IVec3::new(26, 0, 0)), Transform::default()));
    app.add_systems(Update, sync_block_world_transforms_system);

    app.world_mut().resource_mut::<RenderAnchor>().chunk = ChunkCoord::new(1, 0);
    app.update();

    let mut block_query = app
        .world_mut()
        .query_filtered::<(&BlockWorldCoord, &Transform), Without<MainCamera>>();
    let (_, block_transform) = block_query
        .single(app.world_mut())
        .expect("block should exist");
    assert_eq!(
        block_transform.translation,
        world_position_to_render_translation(
            block_world_origin(IVec3::new(26, 0, 0)),
            RenderAnchor {
                chunk: ChunkCoord::new(1, 0),
            },
            WorldLayout::default(),
        )
    );
}
