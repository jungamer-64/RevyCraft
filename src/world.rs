use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

const NEIGHBORS: [IVec3; 6] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
];

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

#[derive(Resource, Default)]
pub struct VoxelWorld {
    pub(crate) blocks: HashMap<IVec3, BlockData>,
}

impl VoxelWorld {
    pub(crate) fn contains_block(&self, coordinate: IVec3) -> bool {
        self.blocks.contains_key(&coordinate)
    }

    pub(crate) fn block_kind(&self, coordinate: IVec3) -> Option<BlockType> {
        self.blocks
            .get(&coordinate)
            .map(|block_data| block_data.kind)
    }

    // This is intentionally insert-only so placement can reject overwriting
    // existing solids without mutating the current block.
    pub(crate) fn try_insert_block(&mut self, coordinate: IVec3, block_type: BlockType) -> bool {
        if self.contains_block(coordinate) {
            return false;
        }

        self.blocks.insert(coordinate, BlockData::new(block_type));
        true
    }

    pub(crate) fn remove_block(&mut self, coordinate: IVec3) -> Option<BlockData> {
        self.blocks.remove(&coordinate)
    }

    pub(crate) fn is_exposed(&self, coordinate: IVec3) -> bool {
        self.contains_block(coordinate)
            && NEIGHBORS
                .iter()
                .any(|offset| !self.contains_block(coordinate + *offset))
    }

    pub(crate) fn iter_block_coords(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.blocks.keys().copied()
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

    pub(crate) fn mark_all_blocks(&mut self, world: &VoxelWorld) {
        self.0.extend(world.iter_block_coords());
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
    entity_index: &mut BlockEntityIndex,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
    coordinate: IVec3,
    block_type: BlockType,
) {
    let entity = commands
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(materials.material_for(block_type)),
            Transform::from_translation(coordinate.as_vec3()),
        ))
        .id();

    entity_index.insert(coordinate, entity);
}

fn sync_dirty_block(
    commands: &mut Commands,
    world: &VoxelWorld,
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

#[derive(Resource, Clone)]
pub struct TerrainSettings {
    pub(crate) chunk_half_size: i32,
    horizontal_frequency: f32,
    height_amplitude: f32,
    base_height: f32,
    surface_depth: i32,
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
            chunk_half_size: 16,
            horizontal_frequency: 0.3,
            height_amplitude: 2.5,
            base_height: 4.0,
            surface_depth: 3,
        }
    }

    pub const fn plains() -> Self {
        Self {
            chunk_half_size: 16,
            horizontal_frequency: 0.18,
            height_amplitude: 1.25,
            base_height: 3.0,
            surface_depth: 2,
        }
    }

    pub const fn rugged() -> Self {
        Self {
            chunk_half_size: 16,
            horizontal_frequency: 0.42,
            height_amplitude: 4.0,
            base_height: 5.0,
            surface_depth: 4,
        }
    }

    // Terrain height is intentionally quantized to integer block levels after
    // applying the float noise profile.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn surface_height(&self, x: i32, z: i32) -> i32 {
        let noise = (x as f32 * self.horizontal_frequency).sin()
            + (z as f32 * self.horizontal_frequency).cos();
        noise
            .mul_add(self.height_amplitude, self.base_height)
            .round() as i32
    }

    const fn block_type_at_height(&self, y: i32, surface_height: i32) -> BlockType {
        if y == surface_height {
            BlockType::Grass
        } else if y >= surface_height - self.surface_depth {
            BlockType::Dirt
        } else {
            BlockType::Stone
        }
    }
}

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

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub fn populate_terrain(voxel_world: &mut VoxelWorld, terrain_settings: &TerrainSettings) {
    for x in -terrain_settings.chunk_half_size..terrain_settings.chunk_half_size {
        for z in -terrain_settings.chunk_half_size..terrain_settings.chunk_half_size {
            let height = terrain_settings.surface_height(x, z);

            for y in 0..=height {
                let block_type = terrain_settings.block_type_at_height(y, height);
                let _ = voxel_world.try_insert_block(IVec3::new(x, y, z), block_type);
            }
        }
    }
}

pub fn spawn_directional_light(commands: &mut Commands) {
    commands.spawn((
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_block_materials() -> BlockMaterials {
        BlockMaterials {
            grass: Handle::default(),
            dirt: Handle::default(),
            stone: Handle::default(),
            highlight: Handle::default(),
        }
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
    fn missing_block_is_not_exposed() {
        assert!(!VoxelWorld::default().is_exposed(IVec3::ZERO));
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
    fn populate_terrain_only_updates_block_data() {
        let mut world = VoxelWorld::default();
        let entity_index = BlockEntityIndex::default();

        populate_terrain(&mut world, &TerrainSettings::default());

        assert!(!world.blocks.is_empty());
        assert!(entity_index.0.is_empty());
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
    fn sync_system_spawns_entity_for_dirty_exposed_block() {
        let mut app = App::new();
        app.insert_resource(VoxelWorld::default());
        app.insert_resource(BlockEntityIndex::default());
        app.insert_resource(RenderSyncQueue::default());
        app.insert_resource(BlockMesh(Handle::default()));
        app.insert_resource(test_block_materials());
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
    fn terrain_surface_height_matches_default_profile() {
        let settings = TerrainSettings::default();

        assert_eq!(settings.surface_height(0, 0), 7);
        assert_eq!(settings.surface_height(10, 0), 7);
    }

    #[test]
    fn terrain_layers_use_expected_block_types() {
        let settings = TerrainSettings::default();
        let surface_height = 7;

        assert_eq!(
            settings.block_type_at_height(surface_height, surface_height),
            BlockType::Grass
        );
        assert_eq!(
            settings.block_type_at_height(surface_height - 2, surface_height),
            BlockType::Dirt
        );
        assert_eq!(
            settings.block_type_at_height(surface_height - 4, surface_height),
            BlockType::Stone
        );
    }

    #[test]
    fn terrain_presets_change_height_profile() {
        let plains = TerrainSettings::plains();
        let rugged = TerrainSettings::rugged();

        assert!(rugged.surface_height(4, 0) > plains.surface_height(4, 0));
    }

    #[test]
    fn try_insert_block_rejects_existing_block() {
        let mut world = VoxelWorld::default();

        assert!(world.try_insert_block(IVec3::ZERO, BlockType::Grass));
        assert!(!world.try_insert_block(IVec3::ZERO, BlockType::Stone));
        assert_eq!(world.block_kind(IVec3::ZERO), Some(BlockType::Grass));
    }
}
