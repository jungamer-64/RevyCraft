use bevy::ecs::system::SystemParam;
use bevy::input::{ButtonInput, keyboard::KeyCode, mouse::MouseButton};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

use crate::cursor::cursor_is_locked;
use crate::player::{EYE_HEIGHT, MainCamera, player_collides_voxel};
use crate::raycast::raycast_voxel;
use crate::world::{BlockMaterials, BlockMesh, BlockType, VoxelWorld, remove_block, spawn_block};

#[derive(Resource, Clone, Copy)]
pub struct SelectedBlock(pub(crate) BlockType);

impl Default for SelectedBlock {
    fn default() -> Self {
        Self(BlockType::Grass)
    }
}

#[derive(Resource, Default)]
pub struct HighlightTarget(pub(crate) Option<(IVec3, IVec3)>);

#[derive(Component)]
struct BlockHighlighter;

pub fn spawn_block_highlighter(
    commands: &mut Commands,
    materials: &BlockMaterials,
    mesh: &Handle<Mesh>,
) {
    let highlight_material = materials.highlight.clone();

    commands
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(highlight_material),
            Transform::from_scale(Vec3::splat(1.01)),
            Visibility::Hidden,
            BlockHighlighter,
        ))
        .insert(Name::new("BlockHighlighter"));
}

#[derive(SystemParam)]
pub struct BlockSelectionResources<'w> {
    keyboard: Res<'w, ButtonInput<KeyCode>>,
    selected_block: ResMut<'w, SelectedBlock>,
}

impl BlockSelectionResources<'_> {
    fn run(mut self) {
        if self.keyboard.just_pressed(KeyCode::Digit1) {
            self.selected_block.0 = BlockType::Grass;
        }
        if self.keyboard.just_pressed(KeyCode::Digit2) {
            self.selected_block.0 = BlockType::Dirt;
        }
        if self.keyboard.just_pressed(KeyCode::Digit3) {
            self.selected_block.0 = BlockType::Stone;
        }
    }
}

pub fn block_selection_system(resources: BlockSelectionResources) {
    resources.run();
}

#[derive(SystemParam)]
pub struct BlockEditResources<'w, 's> {
    commands: Commands<'w, 's>,
    mouse_button: Res<'w, ButtonInput<MouseButton>>,
    cursor_options: Query<'w, 's, &'static CursorOptions, With<PrimaryWindow>>,
    camera_query: Query<'w, 's, &'static Transform, With<MainCamera>>,
    block_mesh: Res<'w, BlockMesh>,
    block_materials: Res<'w, BlockMaterials>,
    selected_block: Res<'w, SelectedBlock>,
    voxel_world: ResMut<'w, VoxelWorld>,
    highlight_target: ResMut<'w, HighlightTarget>,
}

impl BlockEditResources<'_, '_> {
    fn run(self) {
        let Self {
            mut commands,
            mouse_button,
            cursor_options,
            camera_query,
            block_mesh,
            block_materials,
            selected_block,
            mut voxel_world,
            mut highlight_target,
        } = self;

        if !cursor_is_locked(&cursor_options) {
            *highlight_target = HighlightTarget(None);
            return;
        }

        let Ok(transform) = camera_query.single() else {
            *highlight_target = HighlightTarget(None);
            return;
        };

        let ray_origin = transform.translation;
        let ray_direction: Vec3 = transform.forward().into();
        let foot_position = ray_origin - Vec3::Y * EYE_HEIGHT;
        let mut current_raycast = raycast_voxel(&voxel_world, ray_origin, ray_direction, 8.0);
        let block_mesh = &block_mesh.0;
        let selected_block = selected_block.0;

        if let Some(edit_action) = current_edit_action(&mouse_button) {
            if edit_action.remove {
                // Intentionally resolve removal first so a same-frame
                // left+right click places against the newly exposed face.
                current_raycast = handle_block_removal(
                    &mut commands,
                    &mut voxel_world,
                    block_mesh,
                    &block_materials,
                    current_raycast,
                    ray_origin,
                    ray_direction,
                );
            }

            if edit_action.place
                && handle_block_placement(
                    &mut commands,
                    &mut voxel_world,
                    block_mesh,
                    &block_materials,
                    selected_block,
                    current_raycast,
                    foot_position,
                )
            {
                current_raycast = raycast_voxel(&voxel_world, ray_origin, ray_direction, 8.0);
            }
        }

        *highlight_target = HighlightTarget(current_raycast);
    }
}

pub fn block_edit_system(resources: BlockEditResources) {
    resources.run();
}

type HighlighterFilter = (With<BlockHighlighter>, Without<MainCamera>);
type HighlighterComponents = (&'static mut Transform, &'static mut Visibility);

#[derive(SystemParam)]
pub struct HighlightResources<'w, 's> {
    highlighter_query: Query<'w, 's, HighlighterComponents, HighlighterFilter>,
    highlight_target: Res<'w, HighlightTarget>,
}

impl HighlightResources<'_, '_> {
    fn run(mut self) {
        let Ok((mut highlight_transform, mut visibility)) = self.highlighter_query.single_mut()
        else {
            return;
        };

        if let Some((hit_block, _)) = self.highlight_target.0 {
            highlight_transform.translation = hit_block.as_vec3();
            *visibility = Visibility::Visible;
        } else {
            *visibility = Visibility::Hidden;
        }
    }
}

pub fn highlight_system(resources: HighlightResources) {
    resources.run();
}

#[derive(Clone, Copy)]
struct EditAction {
    remove: bool,
    place: bool,
}

fn current_edit_action(mouse_button: &ButtonInput<MouseButton>) -> Option<EditAction> {
    let remove = mouse_button.just_pressed(MouseButton::Left);
    let place = mouse_button.just_pressed(MouseButton::Right);

    if remove || place {
        Some(EditAction { remove, place })
    } else {
        None
    }
}

fn handle_block_removal(
    commands: &mut Commands,
    voxel_world: &mut VoxelWorld,
    block_mesh: &Handle<Mesh>,
    block_materials: &BlockMaterials,
    current_raycast: Option<(IVec3, IVec3)>,
    ray_origin: Vec3,
    ray_direction: Vec3,
) -> Option<(IVec3, IVec3)> {
    if let Some((hit_block, _)) = current_raycast {
        remove_block(
            commands,
            voxel_world,
            &hit_block,
            block_mesh,
            block_materials,
        );

        raycast_voxel(voxel_world, ray_origin, ray_direction, 8.0)
    } else {
        None
    }
}

fn handle_block_placement(
    commands: &mut Commands,
    voxel_world: &mut VoxelWorld,
    block_mesh: &Handle<Mesh>,
    block_materials: &BlockMaterials,
    selected_block: BlockType,
    current_raycast: Option<(IVec3, IVec3)>,
    foot_position: Vec3,
) -> bool {
    let Some((hit_block, hit_normal)) = current_raycast else {
        return false;
    };

    let place_pos = hit_block + hit_normal;
    if player_collides_voxel(foot_position, place_pos) {
        return false;
    }

    spawn_block(
        commands,
        voxel_world,
        block_mesh,
        block_materials,
        place_pos,
        selected_block,
    );

    true
}
