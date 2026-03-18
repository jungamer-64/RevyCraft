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
        self.blocks
            .get(&coordinate)
            .map(|block_data| block_data.kind)
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

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn build_demo_terrain(
        &mut self,
        commands: &mut Commands,
        render_assets: &BlockRenderAssets,
        chunk_size: i32,
    ) {
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

                    self.set_block(IVec3::new(x, y, z), block_type);
                }
            }
        }

        // Terrain generation populates block data first, then spawns only exposed
        // faces in one pass so startup skips neighbor hide/reveal bookkeeping.
        for coord in self.all_block_coords() {
            if self.is_exposed(coord) {
                self.spawn_entity_for_block(commands, render_assets, coord);
            }
        }
    }

    fn place_block(
        &mut self,
        commands: &mut Commands,
        render_assets: &BlockRenderAssets,
        coordinate: IVec3,
        block_type: BlockType,
    ) {
        if self.contains_block(coordinate) {
            return;
        }

        self.set_block(coordinate, block_type);

        if self.is_exposed(coordinate) {
            self.spawn_entity_for_block(commands, render_assets, coordinate);
        }

        self.despawn_hidden_neighbor_entities(commands, coordinate);
    }

    fn remove_block(
        &mut self,
        commands: &mut Commands,
        render_assets: &BlockRenderAssets,
        coordinate: &IVec3,
    ) {
        if self.remove_block_data(coordinate).is_some() {
            if let Some(entity) = self.remove_entity(coordinate) {
                commands.entity(entity).despawn();
            }

            self.expose_neighbor_entities(commands, render_assets, *coordinate);
        }
    }

    fn spawn_entity_for_block(
        &mut self,
        commands: &mut Commands,
        render_assets: &BlockRenderAssets,
        coordinate: IVec3,
    ) {
        let Some(block_type) = self.block_kind(coordinate) else {
            return;
        };
        if self.has_entity(coordinate) {
            return;
        }

        let entity = commands
            .spawn((
                Mesh3d(render_assets.mesh.clone()),
                MeshMaterial3d(render_assets.material_for(block_type)),
                Transform::from_translation(coordinate.as_vec3()),
            ))
            .id();

        self.insert_entity(coordinate, entity);
    }

    fn despawn_hidden_neighbor_entities(&mut self, commands: &mut Commands, coordinate: IVec3) {
        for coord in self.hidden_neighbor_coords(coordinate) {
            if let Some(entity) = self.remove_entity(&coord) {
                commands.entity(entity).despawn();
            }
        }
    }

    fn expose_neighbor_entities(
        &mut self,
        commands: &mut Commands,
        render_assets: &BlockRenderAssets,
        coordinate: IVec3,
    ) {
        for offset in NEIGHBORS {
            let neighbor_coord = coordinate + offset;
            if self.is_exposed(neighbor_coord) {
                self.spawn_entity_for_block(commands, render_assets, neighbor_coord);
            }
        }
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

struct BlockRenderAssets<'a> {
    mesh: &'a Handle<Mesh>,
    materials: &'a BlockMaterials,
}

impl<'a> BlockRenderAssets<'a> {
    const fn new(mesh: &'a Handle<Mesh>, materials: &'a BlockMaterials) -> Self {
        Self { mesh, materials }
    }

    fn material_for(&self, block_type: BlockType) -> Handle<StandardMaterial> {
        match block_type {
            BlockType::Grass => self.materials.grass.clone(),
            BlockType::Dirt => self.materials.dirt.clone(),
            BlockType::Stone => self.materials.stone.clone(),
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
pub fn build_terrain(
    commands: &mut Commands,
    voxel_world: &mut VoxelWorld,
    cube_mesh: &Handle<Mesh>,
    block_materials: &BlockMaterials,
) {
    voxel_world.build_demo_terrain(
        commands,
        &BlockRenderAssets::new(cube_mesh, block_materials),
        16,
    );
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
    world.place_block(
        commands,
        &BlockRenderAssets::new(mesh, materials),
        coordinate,
        block_type,
    );
}

pub fn remove_block(
    commands: &mut Commands,
    world: &mut VoxelWorld,
    coordinate: &IVec3,
    mesh: &Handle<Mesh>,
    materials: &BlockMaterials,
) {
    world.remove_block(
        commands,
        &BlockRenderAssets::new(mesh, materials),
        coordinate,
    );
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
