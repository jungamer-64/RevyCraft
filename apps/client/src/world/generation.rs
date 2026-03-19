use bevy::app::AppExit;
use bevy::ecs::message::MessageReader;
use bevy::ecs::system::SystemParam;
use bevy::math::{DVec3, I64Vec2, I64Vec3};
use bevy::prelude::*;
use noise::{NoiseFn, Perlin};

use crate::player::{INITIAL_CAMERA_EYE_POSITION, MainCamera, WorldPosition};

use super::render::RenderSyncQueue;
use super::{
    Biome, ChunkCoord, ChunkData, ChunkLoadSettings, LocalBlockCoord, TerrainSettings, VoxelWorld,
    WorldLayout, WorldSaveDirectory, WorldSeed, save,
};

#[derive(SystemParam)]
pub struct ChunkStreamingResources<'w, 's> {
    camera_query: Query<'w, 's, &'static WorldPosition, With<MainCamera>>,
    voxel_world: ResMut<'w, VoxelWorld>,
    render_sync_queue: ResMut<'w, RenderSyncQueue>,
    world_seed: Res<'w, WorldSeed>,
    terrain_settings: Res<'w, TerrainSettings>,
    chunk_load_settings: Res<'w, ChunkLoadSettings>,
    world_save_directory: Res<'w, WorldSaveDirectory>,
}

impl ChunkStreamingResources<'_, '_> {
    fn run(mut self) {
        let Ok(camera_position) = self.camera_query.single() else {
            return;
        };

        sync_chunks_around_position(
            &mut self.voxel_world,
            &mut self.render_sync_queue,
            camera_position.0,
            *self.world_seed,
            &self.terrain_settings,
            &self.chunk_load_settings,
            &self.world_save_directory,
        );
    }
}

pub fn sync_visible_chunks_system(resources: ChunkStreamingResources) {
    resources.run();
}

#[derive(SystemParam)]
pub struct ExitSaveResources<'w, 's> {
    app_exit_events: MessageReader<'w, 's, AppExit>,
    voxel_world: Res<'w, VoxelWorld>,
    world_save_directory: Res<'w, WorldSaveDirectory>,
}

impl ExitSaveResources<'_, '_> {
    fn run(mut self) {
        if self.app_exit_events.read().next().is_none() {
            return;
        }

        self.voxel_world
            .save_modified_chunks(&self.world_save_directory);
    }
}

pub fn save_loaded_chunks_on_exit_system(resources: ExitSaveResources) {
    resources.run();
}

pub fn initialize_visible_world(
    voxel_world: &mut VoxelWorld,
    render_sync_queue: &mut RenderSyncQueue,
    seed: WorldSeed,
    terrain_settings: &TerrainSettings,
    chunk_load_settings: &ChunkLoadSettings,
    world_save_directory: &WorldSaveDirectory,
) {
    sync_chunks_around_position(
        voxel_world,
        render_sync_queue,
        INITIAL_CAMERA_EYE_POSITION,
        seed,
        terrain_settings,
        chunk_load_settings,
        world_save_directory,
    );
}

pub fn generate_chunk(
    seed: WorldSeed,
    chunk_coord: ChunkCoord,
    terrain_settings: &TerrainSettings,
    layout: WorldLayout,
) -> ChunkData {
    let noise = TerrainNoise::new(seed);
    let mut blocks = std::collections::HashMap::new();
    let chunk_size = layout.chunk_size();
    let chunk_base_x = chunk_coord.x * chunk_size;
    let chunk_base_z = chunk_coord.z * chunk_size;

    for local_x in 0..chunk_size {
        for local_z in 0..chunk_size {
            let world_x = chunk_base_x + local_x;
            let world_z = chunk_base_z + local_z;
            let biome = classify_biome(&noise, terrain_settings, world_x, world_z);
            let surface_height =
                surface_height_at(&noise, terrain_settings, layout, biome, world_x, world_z);

            for world_y in layout.vertical_min()..=surface_height {
                if should_carve_cave(
                    &noise,
                    terrain_settings,
                    world_x,
                    world_y,
                    world_z,
                    surface_height,
                ) {
                    continue;
                }

                blocks.insert(
                    LocalBlockCoord::new(local_x, world_y - layout.vertical_min(), local_z),
                    super::BlockData::new(TerrainSettings::block_type_at_height(
                        biome,
                        world_y,
                        surface_height,
                    )),
                );
            }
        }
    }

    ChunkData::generated(blocks)
}

fn sync_chunks_around_position(
    voxel_world: &mut VoxelWorld,
    render_sync_queue: &mut RenderSyncQueue,
    position: DVec3,
    seed: WorldSeed,
    terrain_settings: &TerrainSettings,
    chunk_load_settings: &ChunkLoadSettings,
    world_save_directory: &WorldSaveDirectory,
) {
    let Some(center_chunk) = ChunkCoord::from_world_position(position, voxel_world.layout()) else {
        return;
    };
    load_visible_chunks(
        voxel_world,
        render_sync_queue,
        center_chunk,
        seed,
        terrain_settings,
        chunk_load_settings,
        world_save_directory,
    );
    unload_distant_chunks(
        voxel_world,
        render_sync_queue,
        center_chunk,
        chunk_load_settings.unload_radius,
        world_save_directory,
    );
}

fn load_visible_chunks(
    voxel_world: &mut VoxelWorld,
    render_sync_queue: &mut RenderSyncQueue,
    center_chunk: ChunkCoord,
    seed: WorldSeed,
    terrain_settings: &TerrainSettings,
    chunk_load_settings: &ChunkLoadSettings,
    world_save_directory: &WorldSaveDirectory,
) {
    for z_offset in -chunk_load_settings.view_radius..=chunk_load_settings.view_radius {
        for x_offset in -chunk_load_settings.view_radius..=chunk_load_settings.view_radius {
            let chunk_coord = ChunkCoord::new(
                center_chunk.x + i64::from(x_offset),
                center_chunk.z + i64::from(z_offset),
            );
            if voxel_world.has_loaded_chunk(chunk_coord) {
                continue;
            }

            let chunk_data = load_or_generate_chunk(
                chunk_coord,
                seed,
                terrain_settings,
                voxel_world.layout(),
                world_save_directory,
            );
            render_sync_queue.mark_chunk(voxel_world.layout(), chunk_coord, &chunk_data);
            voxel_world.insert_chunk(chunk_coord, chunk_data);
        }
    }
}

fn unload_distant_chunks(
    voxel_world: &mut VoxelWorld,
    render_sync_queue: &mut RenderSyncQueue,
    center_chunk: ChunkCoord,
    unload_radius: i32,
    world_save_directory: &WorldSaveDirectory,
) {
    let loaded_chunk_coords: Vec<_> = voxel_world.loaded_chunk_coords().collect();

    for chunk_coord in loaded_chunk_coords {
        if chunk_coord.chebyshev_distance(center_chunk) <= i64::from(unload_radius) {
            continue;
        }

        if let Some(chunk_data) = voxel_world.unload_chunk(chunk_coord) {
            if chunk_data.modified
                && let Err(error) = save::save_chunk(
                    world_save_directory.path(),
                    chunk_coord,
                    &chunk_data,
                    voxel_world.layout(),
                )
            {
                bevy::log::warn!("failed to save chunk {chunk_coord:?}: {error}");
            }

            // Mark after removal so the next render sync resolves these blocks as Missing.
            // Coordinate conversion only depends on the static world layout.
            render_sync_queue.mark_chunk(voxel_world.layout(), chunk_coord, &chunk_data);
        }
    }
}

fn load_or_generate_chunk(
    chunk_coord: ChunkCoord,
    seed: WorldSeed,
    terrain_settings: &TerrainSettings,
    layout: WorldLayout,
    world_save_directory: &WorldSaveDirectory,
) -> ChunkData {
    match save::load_chunk(world_save_directory.path(), chunk_coord, layout) {
        Ok(Some(chunk_data)) => chunk_data,
        Ok(None) => generate_chunk(seed, chunk_coord, terrain_settings, layout),
        Err(error) => {
            bevy::log::warn!(
                "failed to load saved chunk {chunk_coord:?}; regenerating from seed instead: {error}"
            );
            generate_chunk(seed, chunk_coord, terrain_settings, layout)
        }
    }
}

pub(super) struct TerrainNoise {
    continental: Perlin,
    erosion: Perlin,
    detail: Perlin,
    temperature: Perlin,
    moisture: Perlin,
    cave_primary: Perlin,
    cave_secondary: Perlin,
}

impl TerrainNoise {
    pub(super) fn new(seed: WorldSeed) -> Self {
        Self {
            continental: seeded_perlin(seed, 0x11),
            erosion: seeded_perlin(seed, 0x23),
            detail: seeded_perlin(seed, 0x37),
            temperature: seeded_perlin(seed, 0x41),
            moisture: seeded_perlin(seed, 0x59),
            cave_primary: seeded_perlin(seed, 0x6B),
            cave_secondary: seeded_perlin(seed, 0x7F),
        }
    }
}

fn seeded_perlin(seed: WorldSeed, salt: u64) -> Perlin {
    Perlin::new(fold_seed(seed.0, salt))
}

const fn fold_seed(seed: u64, salt: u64) -> u32 {
    let mixed = seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let bytes = mixed.to_le_bytes();
    u32::from_le_bytes([
        bytes[0] ^ bytes[4],
        bytes[1] ^ bytes[5],
        bytes[2] ^ bytes[6],
        bytes[3] ^ bytes[7],
    ])
}

pub(super) fn classify_biome(
    noise: &TerrainNoise,
    terrain_settings: &TerrainSettings,
    world_x: i64,
    world_z: i64,
) -> Biome {
    let sample = I64Vec2::new(world_x, world_z).as_dvec2();
    let x = sample.x;
    let z = sample.y;
    let temperature = noise.temperature.get([
        x * terrain_settings.temperature_frequency,
        z * terrain_settings.temperature_frequency,
    ]);
    let moisture = noise.moisture.get([
        x * terrain_settings.moisture_frequency,
        z * terrain_settings.moisture_frequency,
    ]);
    let ruggedness = noise
        .detail
        .get([
            x * terrain_settings.detail_frequency,
            z * terrain_settings.detail_frequency,
        ])
        .mul_add(
            0.5,
            noise.erosion.get([
                x * terrain_settings.erosion_frequency,
                z * terrain_settings.erosion_frequency,
            ]),
        );

    if temperature - moisture > 0.35 && moisture < 0.05 {
        Biome::DryStone
    } else if ruggedness > 0.28 {
        Biome::Hills
    } else {
        Biome::Plains
    }
}

pub(super) fn surface_height_at(
    noise: &TerrainNoise,
    terrain_settings: &TerrainSettings,
    layout: WorldLayout,
    biome: Biome,
    world_x: i64,
    world_z: i64,
) -> i64 {
    let sample = I64Vec2::new(world_x, world_z).as_dvec2();
    let x = sample.x;
    let z = sample.y;
    let continentalness = noise.continental.get([
        x * terrain_settings.continental_frequency,
        z * terrain_settings.continental_frequency,
    ]);
    let erosion = noise.erosion.get([
        x * terrain_settings.erosion_frequency,
        z * terrain_settings.erosion_frequency,
    ]);
    let detail = noise.detail.get([
        x * terrain_settings.detail_frequency,
        z * terrain_settings.detail_frequency,
    ]);

    let biome_height_bias = match biome {
        Biome::Plains => -2.0,
        Biome::Hills => 4.0,
        Biome::DryStone => 1.5,
    };

    let height = detail.mul_add(
        terrain_settings.detail_height_scale,
        erosion.mul_add(
            terrain_settings.erosion_height_scale,
            continentalness.mul_add(
                terrain_settings.continental_height_scale,
                terrain_settings.base_height,
            ),
        ),
    ) + biome_height_bias;
    let clamp_range = I64Vec2::new(layout.vertical_min() + 4, layout.vertical_max() - 2).as_dvec2();
    let min_height = clamp_range.x;
    let max_height = clamp_range.y;

    rounded_height_to_i64(height.clamp(min_height, max_height))
}

pub(super) fn should_carve_cave(
    noise: &TerrainNoise,
    terrain_settings: &TerrainSettings,
    world_x: i64,
    world_y: i64,
    world_z: i64,
    surface_height: i64,
) -> bool {
    if world_y >= surface_height - terrain_settings.cave_surface_buffer {
        return false;
    }

    let sample = I64Vec3::new(world_x, world_y, world_z).as_dvec3();
    let x = sample.x * terrain_settings.cave_frequency;
    let y = sample.y * terrain_settings.cave_vertical_frequency;
    let z = sample.z * terrain_settings.cave_frequency;

    let primary = noise.cave_primary.get([x, y, z]).abs();
    let secondary = noise.cave_secondary.get([x * 1.7, y * 1.3, z * 1.7]);
    let density = primary + secondary * 0.35;

    density < terrain_settings.cave_threshold
}

#[allow(clippy::cast_possible_truncation)]
pub(super) const fn rounded_height_to_i64(height: f64) -> i64 {
    height.round() as i64
}
