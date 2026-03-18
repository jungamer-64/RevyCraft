use bevy::prelude::*;
use std::collections::HashMap;

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

#[derive(Clone, Copy)]
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
    block_entities: HashMap<IVec3, Entity>,
}

impl VoxelWorld {
    pub(crate) fn contains_block(&self, coordinate: IVec3) -> bool {
        self.blocks.contains_key(&coordinate)
    }

    pub(crate) fn set_block(&mut self, coordinate: IVec3, block_type: BlockType) {
        self.blocks.insert(coordinate, BlockData::new(block_type));
    }

    fn block_kind(&self, coordinate: IVec3) -> Option<BlockType> {
        self.blocks.get(&coordinate).map(|block_data| block_data.kind)
    }

    fn remove_block_data(&mut self, coordinate: &IVec3) -> Option<BlockData> {
        self.blocks.remove(coordinate)
    }

    fn is_exposed(&self, coordinate: IVec3) -> bool {
        self.contains_block(coordinate)
            && NEIGHBORS
                .iter()
                .any(|offset| !self.contains_block(coordinate + *offset))
    }

    fn all_block_coords(&self) -> Vec<IVec3> {
        self.blocks.keys().copied().collect()
    }

    fn has_entity(&self, coordinate: IVec3) -> bool {
        self.block_entities.contains_key(&coordinate)
    }

    fn insert_entity(&mut self, coordinate: IVec3, entity: Entity) {
        self.block_entities.insert(coordinate, entity);
    }

    fn remove_entity(&mut self, coordinate: &IVec3) -> Option<Entity> {
        self.block_entities.remove(coordinate)
    }

    fn hidden_neighbor_coords(&self, coordinate: IVec3) -> Vec<IVec3> {
        let mut to_despawn = Vec::new();

        for offset in NEIGHBORS {
            let neighbor_coord = coordinate + offset;
            if self.contains_block(neighbor_coord)
                && self.has_entity(neighbor_coord)
                && !self.is_exposed(neighbor_coord)
            {
                to_despawn.push(neighbor_coord);
            }
        }

        to_despawn
    }
}

#[derive(Resource, Clone)]
pub struct BlockMaterials {
    pub(crate) grass: Handle<StandardMaterial>,
    pub(crate) dirt: Handle<StandardMaterial>,
    pub(crate) stone: Handle<StandardMaterial>,
    pub(crate) highlight: Handle<StandardMaterial>,
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

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub fn build_terrain(
    commands: &mut Commands,
    voxel_world: &mut VoxelWorld,
    cube_mesh: &Handle<Mesh>,
    block_materials: &BlockMaterials,
) {
    let chunk_size = 16;

    for x in -chunk_size..chunk_size {
        for z in -chunk_size..chunk_size {
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

                voxel_world.set_block(IVec3::new(x, y, z), block_type);
            }
        }
    }

    // Terrain generation populates block data first, then spawns only exposed
    // faces in one pass so startup skips neighbor hide/reveal bookkeeping.
    for coord in voxel_world.all_block_coords() {
        if voxel_world.is_exposed(coord) {
            spawn_block_entity(commands, voxel_world, cube_mesh, block_materials, coord);
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

pub fn spawn_block(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
    coordinate: IVec3,
    block_type: BlockType,
) {
    if world.contains_block(coordinate) {
        return;
    }

    world.set_block(coordinate, block_type);

    if world.is_exposed(coordinate) {
        spawn_block_entity(commands, world, mesh, materials, coordinate);
    }

    despawn_hidden_neighbor_entities(commands, world, coordinate);
}

fn spawn_block_entity(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
    coordinate: IVec3,
) {
    let Some(block_type) = world.block_kind(coordinate) else {
        return;
    };
    if world.has_entity(coordinate) {
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

    world.insert_entity(coordinate, entity);
}

pub fn remove_block(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    coordinate: &IVec3,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
) {
    if world.remove_block_data(coordinate).is_some() {
        if let Some(entity) = world.remove_entity(coordinate) {
            commands.entity(entity).despawn();
        }

        expose_neighbor_entities(commands, world, coordinate, mesh, materials);
    }
}

fn block_material_for_type(
    materials: &BlockMaterials,
    block_type: BlockType,
) -> Handle<StandardMaterial> {
    match block_type {
        BlockType::Grass => materials.grass.clone(),
        BlockType::Dirt => materials.dirt.clone(),
        BlockType::Stone => materials.stone.clone(),
    }
}

fn despawn_hidden_neighbor_entities(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    coordinate: IVec3,
) {
    for coord in world.hidden_neighbor_coords(coordinate) {
        if let Some(entity) = world.remove_entity(&coord) {
            commands.entity(entity).despawn();
        }
    }
}

fn expose_neighbor_entities(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    coordinate: &IVec3,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
) {
    for offset in NEIGHBORS {
        let neighbor_coord = *coordinate + offset;
        if world.is_exposed(neighbor_coord) {
            spawn_block_entity(commands, world, mesh, materials, neighbor_coord);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_is_exposed_when_any_face_touches_air() {
        let mut world = VoxelWorld::default();
        world.set_block(IVec3::ZERO, BlockType::Stone);

        assert!(world.is_exposed(IVec3::ZERO));
    }

    #[test]
    fn block_is_not_exposed_when_fully_enclosed() {
        let mut world = VoxelWorld::default();
        world.set_block(IVec3::ZERO, BlockType::Stone);
        for offset in NEIGHBORS {
            world.set_block(offset, BlockType::Stone);
        }

        assert!(!world.is_exposed(IVec3::ZERO));
    }

    #[test]
    fn missing_block_is_not_exposed() {
        assert!(!VoxelWorld::default().is_exposed(IVec3::ZERO));
    }
}
