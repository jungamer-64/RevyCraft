#[path = "world/save.rs"]
mod save;

use bevy::app::AppExit;
use bevy::ecs::message::MessageReader;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use noise::{NoiseFn, Perlin};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::player::{INITIAL_CAMERA_EYE_POSITION, MainCamera};

const NEIGHBORS: [IVec3; 6] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
];

const DEFAULT_CHUNK_SIZE: i32 = 16;
const DEFAULT_VERTICAL_MIN: i32 = -24;
const DEFAULT_VERTICAL_MAX: i32 = 96;
const DEFAULT_VIEW_RADIUS: i32 = 2;
const DEFAULT_UNLOAD_RADIUS: i32 = 3;
const DEFAULT_WORLD_SEED: u64 = 0x5EED_CAFE_1234_5678;
const WORLD_SAVE_ROOT: &str = "worlds";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockType {
    Grass,
    Dirt,
    Stone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockData {
    pub(crate) kind: BlockType,
}

impl BlockData {
    pub(crate) const fn new(kind: BlockType) -> Self {
        Self { kind }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkCoord {
    pub(crate) x: i32,
    pub(crate) z: i32,
}

impl ChunkCoord {
    pub(crate) const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    const fn from_world_block(coordinate: IVec3, layout: WorldLayout) -> Self {
        Self {
            x: coordinate.x.div_euclid(layout.chunk_size),
            z: coordinate.z.div_euclid(layout.chunk_size),
        }
    }

    fn from_world_position(position: Vec3, layout: WorldLayout) -> Self {
        Self::from_world_block(position.floor().as_ivec3(), layout)
    }

    fn chebyshev_distance(self, other: Self) -> i32 {
        (self.x - other.x).abs().max((self.z - other.z).abs())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkSaveVersion;

impl ChunkSaveVersion {
    pub(crate) const CURRENT: u32 = 1;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldLayout {
    chunk_size: i32,
    vertical_min: i32,
    vertical_max: i32,
}

impl Default for WorldLayout {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            vertical_min: DEFAULT_VERTICAL_MIN,
            vertical_max: DEFAULT_VERTICAL_MAX,
        }
    }
}

impl WorldLayout {
    pub(crate) const fn new(chunk_size: i32, vertical_min: i32, vertical_max: i32) -> Self {
        Self {
            chunk_size,
            vertical_min,
            vertical_max,
        }
    }

    pub(crate) const fn chunk_size(self) -> i32 {
        self.chunk_size
    }

    pub(crate) const fn vertical_min(self) -> i32 {
        self.vertical_min
    }

    pub(crate) const fn vertical_max(self) -> i32 {
        self.vertical_max
    }

    #[cfg(test)]
    pub(crate) const fn vertical_span(self) -> i32 {
        self.vertical_max - self.vertical_min + 1
    }

    const fn contains_y(self, y: i32) -> bool {
        y >= self.vertical_min && y <= self.vertical_max
    }

    const fn local_from_world(self, coordinate: IVec3) -> Option<(ChunkCoord, IVec3)> {
        if !self.contains_y(coordinate.y) {
            return None;
        }

        let chunk_coord = ChunkCoord::from_world_block(coordinate, self);
        Some((
            chunk_coord,
            IVec3::new(
                coordinate.x.rem_euclid(self.chunk_size),
                coordinate.y - self.vertical_min,
                coordinate.z.rem_euclid(self.chunk_size),
            ),
        ))
    }

    const fn world_from_local(self, chunk_coord: ChunkCoord, local: IVec3) -> IVec3 {
        IVec3::new(
            chunk_coord.x * self.chunk_size + local.x,
            self.vertical_min + local.y,
            chunk_coord.z * self.chunk_size + local.z,
        )
    }
}

#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkLoadSettings {
    layout: WorldLayout,
    pub(crate) view_radius: i32,
    pub(crate) unload_radius: i32,
}

impl Default for ChunkLoadSettings {
    fn default() -> Self {
        Self {
            layout: WorldLayout::default(),
            view_radius: DEFAULT_VIEW_RADIUS,
            unload_radius: DEFAULT_UNLOAD_RADIUS,
        }
    }
}

impl ChunkLoadSettings {
    pub fn from_env() -> Self {
        let view_radius = std::env::var("BEVY_VIEW_RADIUS")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(DEFAULT_VIEW_RADIUS)
            .max(0);

        let unload_radius = (view_radius + 1).max(DEFAULT_UNLOAD_RADIUS);

        Self {
            view_radius,
            unload_radius,
            ..Self::default()
        }
    }

    pub(crate) const fn layout(self) -> WorldLayout {
        self.layout
    }
}

#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldSeed(pub(crate) u64);

impl Default for WorldSeed {
    fn default() -> Self {
        Self(DEFAULT_WORLD_SEED)
    }
}

impl WorldSeed {
    pub fn from_env() -> Self {
        std::env::var("BEVY_WORLD_SEED")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map_or_else(Self::default, Self)
    }
}

#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct WorldSaveDirectory(PathBuf);

impl WorldSaveDirectory {
    pub fn from_seed(seed: WorldSeed) -> Self {
        Self(PathBuf::from(WORLD_SAVE_ROOT).join(format!("seed-{}", seed.0)))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Biome {
    Plains,
    Hills,
    DryStone,
}

#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct TerrainSettings {
    continental_frequency: f64,
    erosion_frequency: f64,
    detail_frequency: f64,
    temperature_frequency: f64,
    moisture_frequency: f64,
    cave_frequency: f64,
    cave_vertical_frequency: f64,
    cave_threshold: f64,
    cave_surface_buffer: i32,
    base_height: f64,
    continental_height_scale: f64,
    erosion_height_scale: f64,
    detail_height_scale: f64,
}

impl Default for TerrainSettings {
    fn default() -> Self {
        Self::rolling_hills()
    }
}

impl TerrainSettings {
    pub fn from_env() -> Self {
        match std::env::var("BEVY_TERRAIN_PRESET")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("plains") => Self::plains(),
            Some("rugged") => Self::rugged(),
            _ => Self::rolling_hills(),
        }
    }

    pub const fn rolling_hills() -> Self {
        Self {
            continental_frequency: 0.009,
            erosion_frequency: 0.022,
            detail_frequency: 0.055,
            temperature_frequency: 0.004,
            moisture_frequency: 0.0045,
            cave_frequency: 0.045,
            cave_vertical_frequency: 0.055,
            cave_threshold: 0.2,
            cave_surface_buffer: 4,
            base_height: 20.0,
            continental_height_scale: 11.0,
            erosion_height_scale: 5.5,
            detail_height_scale: 3.0,
        }
    }

    pub const fn plains() -> Self {
        Self {
            continental_frequency: 0.007,
            erosion_frequency: 0.018,
            detail_frequency: 0.04,
            temperature_frequency: 0.0035,
            moisture_frequency: 0.004,
            cave_frequency: 0.04,
            cave_vertical_frequency: 0.05,
            cave_threshold: 0.18,
            cave_surface_buffer: 5,
            base_height: 16.0,
            continental_height_scale: 8.0,
            erosion_height_scale: 3.0,
            detail_height_scale: 1.8,
        }
    }

    pub const fn rugged() -> Self {
        Self {
            continental_frequency: 0.012,
            erosion_frequency: 0.03,
            detail_frequency: 0.08,
            temperature_frequency: 0.005,
            moisture_frequency: 0.005,
            cave_frequency: 0.05,
            cave_vertical_frequency: 0.06,
            cave_threshold: 0.22,
            cave_surface_buffer: 4,
            base_height: 24.0,
            continental_height_scale: 15.0,
            erosion_height_scale: 8.0,
            detail_height_scale: 4.5,
        }
    }

    const fn subsurface_depth(biome: Biome) -> i32 {
        match biome {
            Biome::Plains => 3,
            Biome::Hills => 4,
            Biome::DryStone => 2,
        }
    }

    const fn block_type_at_height(biome: Biome, y: i32, surface_height: i32) -> BlockType {
        if y == surface_height {
            match biome {
                Biome::DryStone => BlockType::Stone,
                Biome::Plains | Biome::Hills => BlockType::Grass,
            }
        } else if y >= surface_height - Self::subsurface_depth(biome) {
            match biome {
                Biome::DryStone => BlockType::Stone,
                Biome::Plains | Biome::Hills => BlockType::Dirt,
            }
        } else {
            BlockType::Stone
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChunkData {
    pub(crate) blocks: HashMap<IVec3, BlockData>,
    pub(crate) dirty: bool,
    pub(crate) modified: bool,
    pub(crate) generated_from_seed: bool,
}

impl ChunkData {
    const fn generated(blocks: HashMap<IVec3, BlockData>) -> Self {
        Self {
            blocks,
            dirty: true,
            modified: false,
            generated_from_seed: true,
        }
    }

    const fn loaded(blocks: HashMap<IVec3, BlockData>, generated_from_seed: bool) -> Self {
        Self {
            blocks,
            dirty: false,
            modified: false,
            generated_from_seed,
        }
    }
}

#[derive(Resource, Debug)]
pub struct VoxelWorld {
    layout: WorldLayout,
    chunks: HashMap<ChunkCoord, ChunkData>,
}

impl Default for VoxelWorld {
    fn default() -> Self {
        Self::new(WorldLayout::default())
    }
}

impl VoxelWorld {
    pub(crate) fn new(layout: WorldLayout) -> Self {
        Self {
            layout,
            chunks: HashMap::default(),
        }
    }

    pub(crate) const fn layout(&self) -> WorldLayout {
        self.layout
    }

    pub(crate) fn contains_block(&self, coordinate: IVec3) -> bool {
        self.block_kind(coordinate).is_some()
    }

    pub(crate) fn block_kind(&self, coordinate: IVec3) -> Option<BlockType> {
        let (chunk_coord, local_coord) = self.layout.local_from_world(coordinate)?;
        self.chunks
            .get(&chunk_coord)
            .and_then(|chunk_data| chunk_data.blocks.get(&local_coord))
            .map(|block_data| block_data.kind)
    }

    pub(crate) fn try_insert_block(&mut self, coordinate: IVec3, block_type: BlockType) -> bool {
        let Some((chunk_coord, local_coord)) = self.layout.local_from_world(coordinate) else {
            return false;
        };

        let chunk_data = self.chunks.entry(chunk_coord).or_default();
        if chunk_data.blocks.contains_key(&local_coord) {
            return false;
        }

        chunk_data
            .blocks
            .insert(local_coord, BlockData::new(block_type));
        chunk_data.modified = true;
        chunk_data.dirty = true;
        true
    }

    pub(crate) fn remove_block(&mut self, coordinate: IVec3) -> Option<BlockData> {
        let (chunk_coord, local_coord) = self.layout.local_from_world(coordinate)?;
        let chunk_data = self.chunks.get_mut(&chunk_coord)?;
        let removed = chunk_data.blocks.remove(&local_coord)?;
        chunk_data.modified = true;
        chunk_data.dirty = true;
        Some(removed)
    }

    pub(crate) fn is_exposed(&self, coordinate: IVec3) -> bool {
        self.contains_block(coordinate)
            && NEIGHBORS
                .iter()
                .any(|offset| !self.contains_block(coordinate + *offset))
    }

    fn has_loaded_chunk(&self, chunk_coord: ChunkCoord) -> bool {
        self.chunks.contains_key(&chunk_coord)
    }

    fn insert_chunk(&mut self, chunk_coord: ChunkCoord, chunk_data: ChunkData) {
        self.chunks.insert(chunk_coord, chunk_data);
    }

    fn unload_chunk(&mut self, chunk_coord: ChunkCoord) -> Option<ChunkData> {
        self.chunks.remove(&chunk_coord)
    }

    #[cfg(test)]
    pub(crate) fn loaded_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    fn loaded_chunk_coords(&self) -> impl Iterator<Item = ChunkCoord> + '_ {
        self.chunks.keys().copied()
    }

    const fn world_from_local(&self, chunk_coord: ChunkCoord, local: IVec3) -> IVec3 {
        self.layout.world_from_local(chunk_coord, local)
    }

    fn save_modified_chunks(&self, save_directory: &WorldSaveDirectory) {
        for (&chunk_coord, chunk_data) in &self.chunks {
            if !chunk_data.modified {
                continue;
            }

            if let Err(error) =
                save::save_chunk(save_directory.path(), chunk_coord, chunk_data, self.layout)
            {
                bevy::log::warn!("failed to save chunk {chunk_coord:?}: {error}");
            }
        }
    }
}

#[derive(Resource, Default)]
pub struct BlockEntityIndex(HashMap<IVec3, Entity>);

impl BlockEntityIndex {
    fn insert(&mut self, coordinate: IVec3, entity: Entity) {
        self.0.insert(coordinate, entity);
    }

    fn remove(&mut self, coordinate: IVec3) -> Option<Entity> {
        self.0.remove(&coordinate)
    }
}

#[derive(Component)]
pub struct RenderOriginRoot;

#[derive(Resource, Clone, Copy)]
pub struct RenderOriginRootEntity(pub(crate) Entity);

#[derive(Resource, Default)]
pub struct RenderSyncQueue(HashSet<IVec3>);

impl RenderSyncQueue {
    pub(crate) fn mark(&mut self, coordinate: IVec3) {
        self.0.insert(coordinate);
    }

    pub(crate) fn mark_with_neighbors(&mut self, coordinate: IVec3) {
        self.mark(coordinate);
        for offset in NEIGHBORS {
            self.mark(coordinate + offset);
        }
    }

    fn mark_chunk(&mut self, world: &VoxelWorld, chunk_coord: ChunkCoord, chunk_data: &ChunkData) {
        for &local_coord in chunk_data.blocks.keys() {
            self.mark_with_neighbors(world.world_from_local(chunk_coord, local_coord));
        }
    }

    fn drain(&mut self) -> std::collections::hash_set::Drain<'_, IVec3> {
        self.0.drain()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RenderSyncState {
    Missing,
    Hidden,
    Exposed(BlockType),
}

fn render_sync_state_at(world: &VoxelWorld, coordinate: IVec3) -> RenderSyncState {
    match world.block_kind(coordinate) {
        None => RenderSyncState::Missing,
        Some(block_type) if world.is_exposed(coordinate) => RenderSyncState::Exposed(block_type),
        Some(_) => RenderSyncState::Hidden,
    }
}

fn spawn_block_entity(
    commands: &mut Commands,
    render_origin_root: RenderOriginRootEntity,
    entity_index: &mut BlockEntityIndex,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
    coordinate: IVec3,
    block_type: BlockType,
) {
    let mut spawned_entity = None;
    commands
        .entity(render_origin_root.0)
        .with_children(|parent| {
            spawned_entity = Some(
                parent
                    .spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(materials.material_for(block_type)),
                        Transform::from_translation(coordinate.as_vec3()),
                    ))
                    .id(),
            );
        });

    let entity = spawned_entity.expect("block entity spawn should produce a child entity");

    entity_index.insert(coordinate, entity);
}

fn sync_dirty_block(
    commands: &mut Commands,
    world: &VoxelWorld,
    render_origin_root: RenderOriginRootEntity,
    entity_index: &mut BlockEntityIndex,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
    coordinate: IVec3,
) {
    if let Some(entity) = entity_index.remove(coordinate) {
        commands.entity(entity).despawn();
    }

    if let RenderSyncState::Exposed(block_type) = render_sync_state_at(world, coordinate) {
        spawn_block_entity(
            commands,
            render_origin_root,
            entity_index,
            mesh,
            materials,
            coordinate,
            block_type,
        );
    }
}

#[derive(SystemParam)]
pub struct RenderSyncResources<'w, 's> {
    commands: Commands<'w, 's>,
    voxel_world: Res<'w, VoxelWorld>,
    render_origin_root: Res<'w, RenderOriginRootEntity>,
    block_entity_index: ResMut<'w, BlockEntityIndex>,
    render_sync_queue: ResMut<'w, RenderSyncQueue>,
    block_mesh: Res<'w, BlockMesh>,
    block_materials: Res<'w, BlockMaterials>,
}

impl RenderSyncResources<'_, '_> {
    fn run(mut self) {
        for coordinate in self.render_sync_queue.drain() {
            sync_dirty_block(
                &mut self.commands,
                &self.voxel_world,
                *self.render_origin_root,
                &mut self.block_entity_index,
                &self.block_mesh.0,
                &self.block_materials,
                coordinate,
            );
        }
    }
}

pub fn sync_block_render_system(resources: RenderSyncResources) {
    resources.run();
}

#[derive(SystemParam)]
pub struct RenderOriginSyncResources<'w, 's> {
    camera_query: Query<'w, 's, &'static Transform, With<MainCamera>>,
    root_query:
        Query<'w, 's, &'static mut Transform, (With<RenderOriginRoot>, Without<MainCamera>)>,
}

impl RenderOriginSyncResources<'_, '_> {
    fn run(mut self) {
        let Ok(camera_transform) = self.camera_query.single() else {
            return;
        };
        let Ok(mut root_transform) = self.root_query.single_mut() else {
            return;
        };

        root_transform.translation = -camera_transform.translation;
    }
}

pub fn sync_render_origin_root_system(resources: RenderOriginSyncResources) {
    resources.run();
}

#[derive(SystemParam)]
pub struct ChunkStreamingResources<'w, 's> {
    camera_query: Query<'w, 's, &'static Transform, With<MainCamera>>,
    voxel_world: ResMut<'w, VoxelWorld>,
    render_sync_queue: ResMut<'w, RenderSyncQueue>,
    world_seed: Res<'w, WorldSeed>,
    terrain_settings: Res<'w, TerrainSettings>,
    chunk_load_settings: Res<'w, ChunkLoadSettings>,
    world_save_directory: Res<'w, WorldSaveDirectory>,
}

impl ChunkStreamingResources<'_, '_> {
    fn run(mut self) {
        let Ok(camera_transform) = self.camera_query.single() else {
            return;
        };

        sync_chunks_around_position(
            &mut self.voxel_world,
            &mut self.render_sync_queue,
            camera_transform.translation,
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

#[derive(Resource, Clone)]
pub struct BlockMaterials {
    pub(crate) grass: Handle<StandardMaterial>,
    pub(crate) dirt: Handle<StandardMaterial>,
    pub(crate) stone: Handle<StandardMaterial>,
    pub(crate) highlight: Handle<StandardMaterial>,
}

impl BlockMaterials {
    fn material_for(&self, block_type: BlockType) -> Handle<StandardMaterial> {
        match block_type {
            BlockType::Grass => self.grass.clone(),
            BlockType::Dirt => self.dirt.clone(),
            BlockType::Stone => self.stone.clone(),
        }
    }
}

#[derive(Resource)]
pub struct BlockMesh(pub(crate) Handle<Mesh>);

pub fn create_block_materials(materials: &mut ResMut<Assets<StandardMaterial>>) -> BlockMaterials {
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

pub fn create_cube_mesh(meshes: &mut ResMut<Assets<Mesh>>) -> Handle<Mesh> {
    meshes.add(Mesh::from(Cuboid::new(1.0, 1.0, 1.0)))
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

pub fn spawn_render_origin_root(commands: &mut Commands) -> Entity {
    commands
        .spawn((
            Name::new("RenderOriginRoot"),
            RenderOriginRoot,
            Transform::from_translation(-INITIAL_CAMERA_EYE_POSITION),
            GlobalTransform::default(),
            Visibility::Visible,
            InheritedVisibility::VISIBLE,
            ViewVisibility::default(),
        ))
        .id()
}

pub fn spawn_directional_light(commands: &mut Commands, render_origin_root: Entity) {
    commands.entity(render_origin_root).with_children(|parent| {
        parent.spawn((
            DirectionalLight {
                shadows_enabled: true,
                illuminance: 10_000.0,
                ..default()
            },
            Transform::from_rotation(
                Quat::from_rotation_y(-std::f32::consts::FRAC_PI_8)
                    * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_4),
            ),
        ));
    });
}

pub fn generate_chunk(
    seed: WorldSeed,
    chunk_coord: ChunkCoord,
    terrain_settings: &TerrainSettings,
    layout: WorldLayout,
) -> ChunkData {
    let noise = TerrainNoise::new(seed);
    let mut blocks = HashMap::new();
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
                    IVec3::new(local_x, world_y - layout.vertical_min(), local_z),
                    BlockData::new(TerrainSettings::block_type_at_height(
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
    position: Vec3,
    seed: WorldSeed,
    terrain_settings: &TerrainSettings,
    chunk_load_settings: &ChunkLoadSettings,
    world_save_directory: &WorldSaveDirectory,
) {
    let center_chunk = ChunkCoord::from_world_position(position, voxel_world.layout());
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
            let chunk_coord = ChunkCoord::new(center_chunk.x + x_offset, center_chunk.z + z_offset);
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
            render_sync_queue.mark_chunk(voxel_world, chunk_coord, &chunk_data);
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
        if chunk_coord.chebyshev_distance(center_chunk) <= unload_radius {
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

            render_sync_queue.mark_chunk(voxel_world, chunk_coord, &chunk_data);
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

struct TerrainNoise {
    continental: Perlin,
    erosion: Perlin,
    detail: Perlin,
    temperature: Perlin,
    moisture: Perlin,
    cave_primary: Perlin,
    cave_secondary: Perlin,
}

impl TerrainNoise {
    fn new(seed: WorldSeed) -> Self {
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

fn classify_biome(
    noise: &TerrainNoise,
    terrain_settings: &TerrainSettings,
    world_x: i32,
    world_z: i32,
) -> Biome {
    let x = f64::from(world_x);
    let z = f64::from(world_z);
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

fn surface_height_at(
    noise: &TerrainNoise,
    terrain_settings: &TerrainSettings,
    layout: WorldLayout,
    biome: Biome,
    world_x: i32,
    world_z: i32,
) -> i32 {
    let x = f64::from(world_x);
    let z = f64::from(world_z);
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
    let min_height = f64::from(layout.vertical_min() + 4);
    let max_height = f64::from(layout.vertical_max() - 2);

    rounded_height_to_i32(height.clamp(min_height, max_height))
}

fn should_carve_cave(
    noise: &TerrainNoise,
    terrain_settings: &TerrainSettings,
    world_x: i32,
    world_y: i32,
    world_z: i32,
    surface_height: i32,
) -> bool {
    if world_y >= surface_height - terrain_settings.cave_surface_buffer {
        return false;
    }

    let x = f64::from(world_x) * terrain_settings.cave_frequency;
    let y = f64::from(world_y) * terrain_settings.cave_vertical_frequency;
    let z = f64::from(world_z) * terrain_settings.cave_frequency;

    let primary = noise.cave_primary.get([x, y, z]).abs();
    let secondary = noise.cave_secondary.get([x * 1.7, y * 1.3, z * 1.7]);
    let density = primary + secondary * 0.35;

    density < terrain_settings.cave_threshold
}

fn rounded_height_to_i32(height: f64) -> i32 {
    let rounded = height.round();
    let mut quantized_height = 0;

    if rounded.is_sign_negative() {
        loop {
            if f64::from(quantized_height) <= rounded {
                break quantized_height;
            }
            quantized_height -= 1;
        }
    } else {
        loop {
            if f64::from(quantized_height) >= rounded {
                break quantized_height;
            }
            quantized_height += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::transform::TransformPlugin;
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

    fn unique_temp_dir(label: &str) -> PathBuf {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!(
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
        let temp_dir = unique_temp_dir("save-load");
        let layout = WorldLayout::default();
        let chunk_coord = ChunkCoord::new(1, -2);
        let mut chunk_data = ChunkData::loaded(HashMap::new(), true);
        chunk_data
            .blocks
            .insert(IVec3::new(0, 24, 0), BlockData::new(BlockType::Stone));
        chunk_data
            .blocks
            .insert(IVec3::new(5, 30, 7), BlockData::new(BlockType::Grass));

        save::save_chunk(&temp_dir, chunk_coord, &chunk_data, layout).unwrap_or_else(|error| {
            panic!("failed to save chunk for round-trip test: {error}");
        });
        let loaded = save::load_chunk(&temp_dir, chunk_coord, layout).unwrap_or_else(|error| {
            panic!("failed to load chunk for round-trip test: {error}");
        });

        assert_eq!(loaded, Some(chunk_data));

        let _ = fs::remove_dir_all(temp_dir);
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
        let temp_dir = unique_temp_dir("streaming");
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
        app.insert_resource(WorldSaveDirectory(temp_dir.clone()));
        app.add_systems(
            Update,
            (sync_visible_chunks_system, sync_block_render_system).chain(),
        );
        app.world_mut().spawn((
            MainCamera,
            Transform::from_translation(Vec3::new(0.5, 36.0, 0.5)),
        ));

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

        {
            let mut camera = app
                .world_mut()
                .query_filtered::<&mut Transform, With<MainCamera>>();
            let mut transform = camera
                .single_mut(app.world_mut())
                .expect("main camera should exist");
            transform.translation = Vec3::new(48.5, 36.0, 0.5);
        }
        app.update();

        assert_eq!(app.world().resource::<VoxelWorld>().loaded_chunk_count(), 1);
        assert!(save::chunk_path(&temp_dir, ChunkCoord::new(0, 0)).exists());

        {
            let mut camera = app
                .world_mut()
                .query_filtered::<&mut Transform, With<MainCamera>>();
            let mut transform = camera
                .single_mut(app.world_mut())
                .expect("main camera should exist");
            transform.translation = Vec3::new(0.5, 36.0, 0.5);
        }
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

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn chunk_load_marks_entities_for_render_sync() {
        let temp_dir = unique_temp_dir("render-sync");
        let mut app = App::new();
        let chunk_load_settings = ChunkLoadSettings {
            view_radius: 0,
            unload_radius: 1,
            ..ChunkLoadSettings::default()
        };

        app.insert_resource(VoxelWorld::new(chunk_load_settings.layout()));
        app.insert_resource(BlockEntityIndex::default());
        app.insert_resource(RenderSyncQueue::default());
        app.insert_resource(BlockMesh(Handle::default()));
        app.insert_resource(test_block_materials());
        let _ = insert_render_origin_root(&mut app, Vec3::ZERO);
        app.insert_resource(WorldSeed(11));
        app.insert_resource(TerrainSettings::default());
        app.insert_resource(chunk_load_settings);
        app.insert_resource(WorldSaveDirectory(temp_dir.clone()));
        app.add_systems(
            Update,
            (sync_visible_chunks_system, sync_block_render_system).chain(),
        );
        app.world_mut().spawn((
            MainCamera,
            Transform::from_translation(Vec3::new(0.5, 36.0, 0.5)),
        ));

        app.update();

        assert!(!app.world().resource::<BlockEntityIndex>().0.is_empty());

        let _ = fs::remove_dir_all(temp_dir);
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
        let _ = insert_render_origin_root(&mut app, Vec3::ZERO);
        app.world_mut().spawn((
            MainCamera,
            Transform::from_translation(Vec3::new(8.0, 3.0, -2.0)),
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
        let render_origin_root = insert_render_origin_root(&mut app, -Vec3::new(8.0, 0.0, 0.0));
        app.add_systems(
            Update,
            (sync_block_render_system, sync_render_origin_root_system).chain(),
        );
        app.add_systems(Startup, move |mut commands: Commands| {
            commands.entity(render_origin_root).with_children(|parent| {
                parent.spawn((
                    MainCamera,
                    Camera3d::default(),
                    Transform::from_translation(Vec3::new(8.0, 0.0, 0.0)),
                ));
            });
        });

        {
            let mut world = app.world_mut().resource_mut::<VoxelWorld>();
            let _ = world.try_insert_block(IVec3::new(10, 0, 0), BlockType::Stone);
        }
        app.world_mut()
            .resource_mut::<RenderSyncQueue>()
            .mark(IVec3::new(10, 0, 0));

        app.update();

        let block_entity = *app
            .world()
            .resource::<BlockEntityIndex>()
            .0
            .get(&IVec3::new(10, 0, 0))
            .expect("block entity should be indexed");
        let global_transform = app
            .world()
            .get::<GlobalTransform>(block_entity)
            .expect("block entity should have a global transform");
        assert_eq!(global_transform.translation(), Vec3::new(2.0, 0.0, 0.0));

        let mut camera_query = app
            .world_mut()
            .query_filtered::<&Transform, With<MainCamera>>();
        let camera_transform = camera_query
            .single(app.world_mut())
            .expect("camera should exist");
        assert_eq!(camera_transform.translation, Vec3::new(8.0, 0.0, 0.0));
    }
}
