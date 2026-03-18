#[path = "world/generation.rs"]
mod generation;
#[path = "world/render.rs"]
pub mod render;
#[path = "world/save.rs"]
mod save;
#[cfg(test)]
#[path = "world/tests.rs"]
mod tests;

use bevy::math::{DVec3, I64Vec3};
use bevy::prelude::*;
use std::collections::HashMap;
use std::ops::{Add, AddAssign, Sub};
use std::path::{Path, PathBuf};

pub use self::generation::{
    initialize_visible_world, save_loaded_chunks_on_exit_system, sync_visible_chunks_system,
};
pub use self::render::{
    BlockEntityIndex, BlockMaterials, BlockMesh, RenderAnchor, RenderOriginRootEntity,
    RenderSyncQueue, create_block_materials, create_cube_mesh, spawn_directional_light,
    spawn_render_origin_root, sync_block_render_system, sync_block_world_transforms_system,
    sync_render_anchor_system, sync_render_origin_root_system,
};

const NEIGHBORS: [(i64, i64, i64); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

const DEFAULT_CHUNK_SIZE: i64 = 16;
const DEFAULT_VERTICAL_MIN: i64 = -24;
const DEFAULT_VERTICAL_MAX: i64 = 96;
const DEFAULT_VIEW_RADIUS: i32 = 2;
const DEFAULT_UNLOAD_RADIUS: i32 = 3;
const DEFAULT_WORLD_SEED: u64 = 0x5EED_CAFE_1234_5678;
const WORLD_SAVE_ROOT: &str = "worlds";
const I64_MIN_F64: f64 = -9_223_372_036_854_775_808.0;
const I64_UPPER_BOUND_F64: f64 = 9_223_372_036_854_775_808.0;

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
pub struct WorldBlockCoord(I64Vec3);

impl WorldBlockCoord {
    pub(crate) const fn new(x: i64, y: i64, z: i64) -> Self {
        Self(I64Vec3::new(x, y, z))
    }

    pub(crate) fn checked_from_world_position(position: DVec3) -> Option<Self> {
        let floored = position.floor();
        if !component_fits_i64(floored.x)
            || !component_fits_i64(floored.y)
            || !component_fits_i64(floored.z)
        {
            return None;
        }

        Some(Self(floored.as_i64vec3()))
    }

    pub(crate) const fn x(self) -> i64 {
        self.0.x
    }

    pub(crate) const fn y(self) -> i64 {
        self.0.y
    }

    pub(crate) const fn z(self) -> i64 {
        self.0.z
    }

    pub(crate) fn as_dvec3(self) -> DVec3 {
        self.0.as_dvec3()
    }
}

impl Add<I64Vec3> for WorldBlockCoord {
    type Output = Self;

    fn add(self, rhs: I64Vec3) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl AddAssign<I64Vec3> for WorldBlockCoord {
    fn add_assign(&mut self, rhs: I64Vec3) {
        self.0 += rhs;
    }
}

impl Sub<I64Vec3> for WorldBlockCoord {
    type Output = Self;

    fn sub(self, rhs: I64Vec3) -> Self::Output {
        Self(self.0 - rhs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalBlockCoord(I64Vec3);

impl LocalBlockCoord {
    pub(crate) const fn new(x: i64, y: i64, z: i64) -> Self {
        Self(I64Vec3::new(x, y, z))
    }

    pub(crate) const fn x(self) -> i64 {
        self.0.x
    }

    pub(crate) const fn y(self) -> i64 {
        self.0.y
    }

    pub(crate) const fn z(self) -> i64 {
        self.0.z
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkCoord {
    pub(crate) x: i64,
    pub(crate) z: i64,
}

impl ChunkCoord {
    pub(crate) const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }

    pub(crate) const fn from_world_block(coordinate: WorldBlockCoord, layout: WorldLayout) -> Self {
        Self {
            x: coordinate.x().div_euclid(layout.chunk_size),
            z: coordinate.z().div_euclid(layout.chunk_size),
        }
    }

    pub(crate) fn from_world_position(position: DVec3, layout: WorldLayout) -> Option<Self> {
        Some(Self::from_world_block(
            world_block_from_position(position)?,
            layout,
        ))
    }

    pub(crate) fn chebyshev_distance(self, other: Self) -> i64 {
        (self.x - other.x).abs().max((self.z - other.z).abs())
    }

    pub(crate) fn world_origin(self, layout: WorldLayout) -> DVec3 {
        I64Vec3::new(
            self.x * layout.chunk_size(),
            0,
            self.z * layout.chunk_size(),
        )
        .as_dvec3()
    }
}

#[inline]
pub fn world_block_from_position(position: DVec3) -> Option<WorldBlockCoord> {
    WorldBlockCoord::checked_from_world_position(position)
}

#[inline]
pub fn block_world_origin(coordinate: WorldBlockCoord) -> DVec3 {
    coordinate.as_dvec3()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkSaveVersion;

impl ChunkSaveVersion {
    pub(crate) const CURRENT: u32 = 2;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldLayout {
    chunk_size: i64,
    vertical_min: i64,
    vertical_max: i64,
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
    pub(crate) const fn new(chunk_size: i64, vertical_min: i64, vertical_max: i64) -> Self {
        Self {
            chunk_size,
            vertical_min,
            vertical_max,
        }
    }

    pub(crate) const fn chunk_size(self) -> i64 {
        self.chunk_size
    }

    pub(crate) const fn vertical_min(self) -> i64 {
        self.vertical_min
    }

    pub(crate) const fn vertical_max(self) -> i64 {
        self.vertical_max
    }

    #[cfg(test)]
    pub(crate) const fn vertical_span(self) -> i64 {
        self.vertical_max - self.vertical_min + 1
    }

    const fn contains_y(self, y: i64) -> bool {
        y >= self.vertical_min && y <= self.vertical_max
    }

    pub(crate) const fn local_from_world(
        self,
        coordinate: WorldBlockCoord,
    ) -> Option<(ChunkCoord, LocalBlockCoord)> {
        if !self.contains_y(coordinate.y()) {
            return None;
        }

        let chunk_coord = ChunkCoord::from_world_block(coordinate, self);
        Some((
            chunk_coord,
            LocalBlockCoord::new(
                coordinate.x().rem_euclid(self.chunk_size),
                coordinate.y() - self.vertical_min,
                coordinate.z().rem_euclid(self.chunk_size),
            ),
        ))
    }

    pub(crate) const fn world_from_local(
        self,
        chunk_coord: ChunkCoord,
        local: LocalBlockCoord,
    ) -> WorldBlockCoord {
        WorldBlockCoord::new(
            chunk_coord.x * self.chunk_size + local.x(),
            self.vertical_min + local.y(),
            chunk_coord.z * self.chunk_size + local.z(),
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

        let unload_radius = view_radius.saturating_add(1).max(DEFAULT_UNLOAD_RADIUS);

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

    pub(crate) fn path(&self) -> &Path {
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
    pub(crate) continental_frequency: f64,
    pub(crate) erosion_frequency: f64,
    pub(crate) detail_frequency: f64,
    pub(crate) temperature_frequency: f64,
    pub(crate) moisture_frequency: f64,
    pub(crate) cave_frequency: f64,
    pub(crate) cave_vertical_frequency: f64,
    pub(crate) cave_threshold: f64,
    pub(crate) cave_surface_buffer: i64,
    pub(crate) base_height: f64,
    pub(crate) continental_height_scale: f64,
    pub(crate) erosion_height_scale: f64,
    pub(crate) detail_height_scale: f64,
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

    const fn subsurface_depth(biome: Biome) -> i64 {
        match biome {
            Biome::Plains => 3,
            Biome::Hills => 4,
            Biome::DryStone => 2,
        }
    }

    const fn block_type_at_height(biome: Biome, y: i64, surface_height: i64) -> BlockType {
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
    pub(crate) blocks: HashMap<LocalBlockCoord, BlockData>,
    pub(crate) dirty: bool,
    pub(crate) modified: bool,
    pub(crate) generated_from_seed: bool,
}

impl ChunkData {
    pub(crate) const fn generated(blocks: HashMap<LocalBlockCoord, BlockData>) -> Self {
        Self {
            blocks,
            dirty: true,
            modified: false,
            generated_from_seed: true,
        }
    }

    pub(crate) const fn loaded(
        blocks: HashMap<LocalBlockCoord, BlockData>,
        generated_from_seed: bool,
    ) -> Self {
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

    pub(crate) fn contains_block(&self, coordinate: WorldBlockCoord) -> bool {
        self.block_kind(coordinate).is_some()
    }

    pub(crate) fn block_kind(&self, coordinate: WorldBlockCoord) -> Option<BlockType> {
        let (chunk_coord, local_coord) = self.layout.local_from_world(coordinate)?;
        self.chunks
            .get(&chunk_coord)
            .and_then(|chunk_data| chunk_data.blocks.get(&local_coord))
            .map(|block_data| block_data.kind)
    }

    pub(crate) fn try_insert_block(
        &mut self,
        coordinate: WorldBlockCoord,
        block_type: BlockType,
    ) -> bool {
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

    pub(crate) fn remove_block(&mut self, coordinate: WorldBlockCoord) -> Option<BlockData> {
        let (chunk_coord, local_coord) = self.layout.local_from_world(coordinate)?;
        let chunk_data = self.chunks.get_mut(&chunk_coord)?;
        let removed = chunk_data.blocks.remove(&local_coord)?;
        chunk_data.modified = true;
        chunk_data.dirty = true;
        Some(removed)
    }

    pub(crate) fn is_exposed(&self, coordinate: WorldBlockCoord) -> bool {
        self.contains_block(coordinate)
            && NEIGHBORS
                .iter()
                .any(|&(x, y, z)| !self.contains_block(coordinate + I64Vec3::new(x, y, z)))
    }

    pub(crate) fn has_loaded_chunk(&self, chunk_coord: ChunkCoord) -> bool {
        self.chunks.contains_key(&chunk_coord)
    }

    pub(crate) fn insert_chunk(&mut self, chunk_coord: ChunkCoord, chunk_data: ChunkData) {
        self.chunks.insert(chunk_coord, chunk_data);
    }

    pub(crate) fn unload_chunk(&mut self, chunk_coord: ChunkCoord) -> Option<ChunkData> {
        self.chunks.remove(&chunk_coord)
    }

    #[cfg(test)]
    pub(crate) fn loaded_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub(crate) fn loaded_chunk_coords(&self) -> impl Iterator<Item = ChunkCoord> + '_ {
        self.chunks.keys().copied()
    }

    pub(crate) fn save_modified_chunks(&self, save_directory: &WorldSaveDirectory) {
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

fn component_fits_i64(value: f64) -> bool {
    (I64_MIN_F64..I64_UPPER_BOUND_F64).contains(&value)
}
