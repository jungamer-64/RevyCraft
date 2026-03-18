use bevy::ecs::system::SystemParam;
use bevy::math::DVec3;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::player::{MainCamera, WorldPosition};

use super::{
    BlockType, ChunkCoord, ChunkData, NEIGHBORS, VoxelWorld, WorldLayout, block_world_origin,
};

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

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockWorldCoord(pub(crate) IVec3);

#[derive(Resource, Clone, Copy)]
pub struct RenderOriginRootEntity(pub(crate) Entity);

#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderAnchor {
    pub(crate) chunk: ChunkCoord,
}

impl RenderAnchor {
    pub(crate) fn from_world_position(position: DVec3, layout: WorldLayout) -> Self {
        Self {
            chunk: ChunkCoord::from_world_position(position, layout),
        }
    }

    pub(crate) fn origin(self, layout: WorldLayout) -> DVec3 {
        self.chunk.world_origin(layout)
    }
}

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

pub fn world_position_to_render_translation(
    world_position: DVec3,
    render_anchor: RenderAnchor,
    layout: WorldLayout,
) -> Vec3 {
    (world_position - render_anchor.origin(layout)).as_vec3()
}

fn block_to_render_translation(
    coordinate: IVec3,
    render_anchor: RenderAnchor,
    layout: WorldLayout,
) -> Vec3 {
    world_position_to_render_translation(block_world_origin(coordinate), render_anchor, layout)
}

fn spawn_block_entity(
    commands: &mut Commands,
    render_origin_root: RenderOriginRootEntity,
    entity_index: &mut BlockEntityIndex,
    mesh: &Handle<Mesh>,
    material: Handle<StandardMaterial>,
    coordinate: IVec3,
    translation: Vec3,
) {
    let mut spawned_entity = None;
    commands
        .entity(render_origin_root.0)
        .with_children(|parent| {
            spawned_entity = Some(
                parent
                    .spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material),
                        Transform::from_translation(translation),
                        BlockWorldCoord(coordinate),
                    ))
                    .id(),
            );
        });

    let entity = spawned_entity.expect("block entity spawn should produce a child entity");
    entity_index.insert(coordinate, entity);
}
#[derive(SystemParam)]
pub struct RenderSyncResources<'w, 's> {
    commands: Commands<'w, 's>,
    voxel_world: Res<'w, VoxelWorld>,
    render_origin_root: Res<'w, RenderOriginRootEntity>,
    render_anchor: Res<'w, RenderAnchor>,
    block_entity_index: ResMut<'w, BlockEntityIndex>,
    render_sync_queue: ResMut<'w, RenderSyncQueue>,
    block_mesh: Res<'w, BlockMesh>,
    block_materials: Res<'w, BlockMaterials>,
}

impl RenderSyncResources<'_, '_> {
    fn run(mut self) {
        let layout = self.voxel_world.layout();
        for coordinate in self.render_sync_queue.drain() {
            if let Some(entity) = self.block_entity_index.remove(coordinate) {
                self.commands.entity(entity).despawn();
            }

            if let RenderSyncState::Exposed(block_type) =
                render_sync_state_at(&self.voxel_world, coordinate)
            {
                spawn_block_entity(
                    &mut self.commands,
                    *self.render_origin_root,
                    &mut self.block_entity_index,
                    &self.block_mesh.0,
                    self.block_materials.material_for(block_type),
                    coordinate,
                    block_to_render_translation(coordinate, *self.render_anchor, layout),
                );
            }
        }
    }
}

pub fn sync_block_render_system(resources: RenderSyncResources) {
    resources.run();
}

#[derive(SystemParam)]
pub struct RenderOriginSyncResources<'w, 's> {
    voxel_world: Res<'w, VoxelWorld>,
    render_anchor: Res<'w, RenderAnchor>,
    camera_query: Query<'w, 's, (&'static mut Transform, &'static WorldPosition), With<MainCamera>>,
    root_query:
        Query<'w, 's, &'static mut Transform, (With<RenderOriginRoot>, Without<MainCamera>)>,
}

impl RenderOriginSyncResources<'_, '_> {
    fn run(mut self) {
        let Ok((mut camera_transform, world_position)) = self.camera_query.single_mut() else {
            return;
        };
        let Ok(mut root_transform) = self.root_query.single_mut() else {
            return;
        };

        camera_transform.translation = world_position_to_render_translation(
            world_position.0,
            *self.render_anchor,
            self.voxel_world.layout(),
        );
        root_transform.translation = -camera_transform.translation;
    }
}

pub fn sync_render_origin_root_system(resources: RenderOriginSyncResources) {
    resources.run();
}

#[derive(SystemParam)]
pub struct RenderAnchorSyncResources<'w, 's> {
    voxel_world: Res<'w, VoxelWorld>,
    render_anchor: ResMut<'w, RenderAnchor>,
    camera_query: Query<'w, 's, &'static WorldPosition, With<MainCamera>>,
}

impl RenderAnchorSyncResources<'_, '_> {
    fn run(mut self) {
        let Ok(world_position) = self.camera_query.single() else {
            return;
        };

        let next_anchor =
            RenderAnchor::from_world_position(world_position.0, self.voxel_world.layout());
        if *self.render_anchor != next_anchor {
            *self.render_anchor = next_anchor;
        }
    }
}

pub fn sync_render_anchor_system(resources: RenderAnchorSyncResources) {
    resources.run();
}

#[derive(SystemParam)]
pub struct BlockTransformSyncResources<'w, 's> {
    voxel_world: Res<'w, VoxelWorld>,
    render_anchor: Res<'w, RenderAnchor>,
    block_query: Query<'w, 's, (&'static BlockWorldCoord, &'static mut Transform)>,
}

impl BlockTransformSyncResources<'_, '_> {
    fn run(mut self) {
        if !self.render_anchor.is_changed() {
            return;
        }

        let layout = self.voxel_world.layout();
        for (block_world_coord, mut transform) in &mut self.block_query {
            transform.translation =
                block_to_render_translation(block_world_coord.0, *self.render_anchor, layout);
        }
    }
}

pub fn sync_block_world_transforms_system(resources: BlockTransformSyncResources) {
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

pub fn spawn_render_origin_root(
    commands: &mut Commands,
    initial_camera_translation: Vec3,
) -> Entity {
    commands
        .spawn((
            Name::new("RenderOriginRoot"),
            RenderOriginRoot,
            Transform::from_translation(-initial_camera_translation),
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
