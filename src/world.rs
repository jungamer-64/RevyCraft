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
    pub(crate) entity: Option<Entity>,
}

impl BlockData {
    pub(crate) const fn new(kind: BlockType) -> Self {
        Self { kind, entity: None }
    }
}

#[derive(Resource, Default)]
pub struct VoxelWorld {
    pub(crate) blocks: HashMap<IVec3, BlockData>,
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

                insert_block_data(voxel_world, IVec3::new(x, y, z), block_type);
            }
        }
    }

    let coords: Vec<IVec3> = voxel_world.blocks.keys().copied().collect();
    for coord in coords {
        if is_exposed(&voxel_world.blocks, coord) {
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
    if world.blocks.contains_key(&coordinate) {
        return;
    }

    insert_block_data(world, coordinate, block_type);

    if is_exposed(&world.blocks, coordinate) {
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
    let Some(block_data) = world.blocks.get_mut(&coordinate) else {
        return;
    };
    if block_data.entity.is_some() {
        return;
    }

    let material_handle = block_material_for_type(materials, block_data.kind);
    let entity = commands
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material_handle),
            Transform::from_translation(coordinate.as_vec3()),
        ))
        .id();

    block_data.entity = Some(entity);
}

fn is_exposed(blocks: &HashMap<IVec3, BlockData>, coord: IVec3) -> bool {
    NEIGHBORS
        .iter()
        .any(|offset| !blocks.contains_key(&(coord + *offset)))
}

pub fn remove_block(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    coordinate: &IVec3,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
) {
    if let Some(block_data) = world.blocks.remove(coordinate) {
        if let Some(entity) = block_data.entity {
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

fn insert_block_data(world: &mut VoxelWorld, coordinate: IVec3, block_type: BlockType) {
    world.blocks.insert(coordinate, BlockData::new(block_type));
}

fn hidden_neighbor_coords(blocks: &HashMap<IVec3, BlockData>, coordinate: IVec3) -> Vec<IVec3> {
    let mut to_despawn = Vec::new();

    for offset in NEIGHBORS {
        let neighbor_coord = coordinate + offset;
        if let Some(neighbor) = blocks.get(&neighbor_coord)
            && neighbor.entity.is_some()
            && !is_exposed(blocks, neighbor_coord)
        {
            to_despawn.push(neighbor_coord);
        }
    }

    to_despawn
}

fn despawn_hidden_neighbor_entities(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    coordinate: IVec3,
) {
    for coord in hidden_neighbor_coords(&world.blocks, coordinate) {
        if let Some(neighbor) = world.blocks.get_mut(&coord)
            && let Some(entity) = neighbor.entity.take()
        {
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
        if world.blocks.contains_key(&neighbor_coord) && is_exposed(&world.blocks, neighbor_coord) {
            spawn_block_entity(commands, world, mesh, materials, neighbor_coord);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_is_exposed_when_any_face_touches_air() {
        let mut blocks = HashMap::new();
        blocks.insert(IVec3::ZERO, BlockData::new(BlockType::Stone));

        assert!(is_exposed(&blocks, IVec3::ZERO));
    }

    #[test]
    fn block_is_not_exposed_when_fully_enclosed() {
        let mut blocks = HashMap::new();
        blocks.insert(IVec3::ZERO, BlockData::new(BlockType::Stone));
        for offset in NEIGHBORS {
            blocks.insert(offset, BlockData::new(BlockType::Stone));
        }

        assert!(!is_exposed(&blocks, IVec3::ZERO));
    }
}
