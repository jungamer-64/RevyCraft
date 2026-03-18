use bevy::ecs::system::SystemParam;
use bevy::input::{ButtonInput, keyboard::KeyCode, mouse::MouseButton};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

use crate::cursor::cursor_is_locked;
use crate::player::{EYE_HEIGHT, MainCamera, player_blocks_block_placement};
use crate::raycast::raycast_voxel;
use crate::world::{BlockMaterials, BlockType, RenderSyncQueue, VoxelWorld};

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
    mouse_button: Res<'w, ButtonInput<MouseButton>>,
    cursor_options: Query<'w, 's, &'static CursorOptions, With<PrimaryWindow>>,
    camera_query: Query<'w, 's, &'static Transform, With<MainCamera>>,
    selected_block: Res<'w, SelectedBlock>,
    voxel_world: ResMut<'w, VoxelWorld>,
    render_sync_queue: ResMut<'w, RenderSyncQueue>,
    highlight_target: ResMut<'w, HighlightTarget>,
}

impl BlockEditResources<'_, '_> {
    fn run(self) {
        let Self {
            mouse_button,
            cursor_options,
            camera_query,
            selected_block,
            mut voxel_world,
            mut render_sync_queue,
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
        let selected_block = selected_block.0;

        match current_edit_action(&mouse_button) {
            Some(EditAction::Remove) => {
                current_raycast = handle_block_removal(
                    &mut voxel_world,
                    &mut render_sync_queue,
                    current_raycast,
                    ray_origin,
                    ray_direction,
                );
            }
            Some(EditAction::Place) => {
                if handle_block_placement(
                    &mut voxel_world,
                    &mut render_sync_queue,
                    selected_block,
                    current_raycast,
                    foot_position,
                ) {
                    current_raycast = raycast_voxel(&voxel_world, ray_origin, ray_direction, 8.0);
                }
            }
            Some(EditAction::RemoveThenPlace) => {
                // Left+right in the same frame is treated as "replace the hit
                // block with the selected block on the newly exposed face".
                if let Some(next_raycast) = handle_block_removal(
                    &mut voxel_world,
                    &mut render_sync_queue,
                    current_raycast,
                    ray_origin,
                    ray_direction,
                ) {
                    current_raycast = Some(next_raycast);

                    if handle_block_placement(
                        &mut voxel_world,
                        &mut render_sync_queue,
                        selected_block,
                        current_raycast,
                        foot_position,
                    ) {
                        current_raycast =
                            raycast_voxel(&voxel_world, ray_origin, ray_direction, 8.0);
                    }
                } else {
                    current_raycast = None;
                }
            }
            None => {}
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
enum EditAction {
    Remove,
    Place,
    RemoveThenPlace,
}

fn current_edit_action(mouse_button: &ButtonInput<MouseButton>) -> Option<EditAction> {
    let remove = mouse_button.just_pressed(MouseButton::Left);
    let place = mouse_button.just_pressed(MouseButton::Right);

    match (remove, place) {
        (true, true) => Some(EditAction::RemoveThenPlace),
        (true, false) => Some(EditAction::Remove),
        (false, true) => Some(EditAction::Place),
        (false, false) => None,
    }
}

fn handle_block_removal(
    voxel_world: &mut VoxelWorld,
    render_sync_queue: &mut RenderSyncQueue,
    current_raycast: Option<(IVec3, IVec3)>,
    ray_origin: Vec3,
    ray_direction: Vec3,
) -> Option<(IVec3, IVec3)> {
    if let Some((hit_block, _)) = current_raycast {
        let _ = voxel_world.remove_block(hit_block);
        render_sync_queue.mark_with_neighbors(hit_block);

        raycast_voxel(voxel_world, ray_origin, ray_direction, 8.0)
    } else {
        None
    }
}

fn handle_block_placement(
    voxel_world: &mut VoxelWorld,
    render_sync_queue: &mut RenderSyncQueue,
    selected_block: BlockType,
    current_raycast: Option<(IVec3, IVec3)>,
    foot_position: Vec3,
) -> bool {
    let Some((hit_block, hit_normal)) = current_raycast else {
        return false;
    };

    let place_pos = hit_block + hit_normal;
    if player_blocks_block_placement(foot_position, place_pos) {
        return false;
    }

    if !voxel_world.set_block(place_pos, selected_block) {
        return false;
    }

    render_sync_queue.mark_with_neighbors(place_pos);

    true
}
