use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::player::{INITIAL_CAMERA_EYE_POSITION, MainCamera};

use super::{BlockType, ChunkCoord, ChunkData, NEIGHBORS, VoxelWorld};

#[derive(Resource, Default)]
pub struct BlockEntityIndex(pub(crate) HashMap<IVec3, Entity>);

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
pub struct RenderSyncQueue(pub(crate) HashSet<IVec3>);

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

    pub(crate) fn mark_chunk(
        &mut self,
        world: &VoxelWorld,
        chunk_coord: ChunkCoord,
        chunk_data: &ChunkData,
    ) {
        for &local_coord in chunk_data.blocks.keys() {
            self.mark_with_neighbors(world.world_from_local(chunk_coord, local_coord));
        }
    }

    pub(super) fn drain(&mut self) -> std::collections::hash_set::Drain<'_, IVec3> {
        self.0.drain()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RenderSyncState {
    Missing,
    Hidden,
    Exposed(BlockType),
}

pub(super) fn render_sync_state_at(world: &VoxelWorld, coordinate: IVec3) -> RenderSyncState {
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
