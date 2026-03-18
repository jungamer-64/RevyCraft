use bevy::ecs::system::SystemParam;
use bevy::input::{ButtonInput, keyboard::KeyCode, mouse::MouseButton};
use bevy::math::{DVec3, I64Vec3};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

use crate::cursor::cursor_is_locked;
use crate::player::{EYE_HEIGHT, MainCamera, WorldPosition, player_blocks_block_placement};
use crate::raycast::raycast_voxel;
use crate::world::render::world_position_to_render_translation;
use crate::world::{
    BlockMaterials, BlockType, RenderAnchor, RenderSyncQueue, VoxelWorld, WorldBlockCoord,
    block_world_origin,
};

#[derive(Resource, Clone, Copy)]
pub struct SelectedBlock(pub(crate) BlockType);

impl Default for SelectedBlock {
    fn default() -> Self {
        Self(BlockType::Grass)
    }
}

#[derive(Resource, Default)]
pub struct HighlightTarget(pub(crate) Option<(WorldBlockCoord, I64Vec3)>);

#[derive(Component)]
struct BlockHighlighter;

pub fn spawn_block_highlighter(
    commands: &mut Commands,
    materials: &BlockMaterials,
    mesh: &Handle<Mesh>,
    render_origin_root: Entity,
) {
    let highlight_material = materials.highlight.clone();

    commands.entity(render_origin_root).with_children(|parent| {
        parent
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(highlight_material),
                Transform::from_scale(Vec3::splat(1.01)),
                Visibility::Hidden,
                BlockHighlighter,
            ))
            .insert(Name::new("BlockHighlighter"));
    });
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
pub struct HighlightTargetUpdateResources<'w, 's> {
    cursor_options: Query<'w, 's, &'static CursorOptions, With<PrimaryWindow>>,
    camera_query: Query<'w, 's, (&'static Transform, &'static WorldPosition), With<MainCamera>>,
    voxel_world: Res<'w, VoxelWorld>,
    highlight_target: ResMut<'w, HighlightTarget>,
}

impl HighlightTargetUpdateResources<'_, '_> {
    fn run(mut self) {
        self.highlight_target.0 = compute_highlight_target(
            self.cursor_options.single().ok(),
            self.camera_query.single().ok(),
            &self.voxel_world,
        );
    }
}

pub fn update_highlight_target_pre_edit_system(resources: HighlightTargetUpdateResources) {
    resources.run();
}

pub fn update_highlight_target_post_edit_system(resources: HighlightTargetUpdateResources) {
    resources.run();
}

#[derive(SystemParam)]
pub struct BlockEditResources<'w, 's> {
    mouse_button: Res<'w, ButtonInput<MouseButton>>,
    cursor_options: Query<'w, 's, &'static CursorOptions, With<PrimaryWindow>>,
    camera_query: Query<'w, 's, (&'static Transform, &'static WorldPosition), With<MainCamera>>,
    selected_block: Res<'w, SelectedBlock>,
    highlight_target: Res<'w, HighlightTarget>,
    voxel_world: ResMut<'w, VoxelWorld>,
    render_sync_queue: ResMut<'w, RenderSyncQueue>,
}

impl BlockEditResources<'_, '_> {
    fn run(self) {
        let Self {
            mouse_button,
            cursor_options,
            camera_query,
            selected_block,
            highlight_target,
            mut voxel_world,
            mut render_sync_queue,
        } = self;

        let Some(edit_action) = current_edit_action(&mouse_button) else {
            return;
        };

        if !cursor_is_locked(cursor_options.single().ok()) {
            return;
        }

        let Some(edit_context) = build_block_edit_context(
            camera_query.single().ok(),
            highlight_target.0,
            selected_block.0,
        ) else {
            return;
        };

        apply_edit_action(
            edit_action,
            edit_context,
            &mut voxel_world,
            &mut render_sync_queue,
        );
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
    render_anchor: Res<'w, RenderAnchor>,
    voxel_world: Res<'w, VoxelWorld>,
}

impl HighlightResources<'_, '_> {
    fn run(mut self) {
        let Ok((mut highlight_transform, mut visibility)) = self.highlighter_query.single_mut()
        else {
            return;
        };

        if let Some((hit_block, _)) = self.highlight_target.0 {
            highlight_transform.translation = world_position_to_render_translation(
                block_world_origin(hit_block),
                *self.render_anchor,
                self.voxel_world.layout(),
            );
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

#[derive(Clone, Copy)]
struct BlockEditContext {
    ray_origin: DVec3,
    ray_direction: DVec3,
    foot_position: DVec3,
    current_raycast: Option<(WorldBlockCoord, I64Vec3)>,
    selected_block: BlockType,
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

fn build_block_edit_context(
    camera_transform: Option<(&Transform, &WorldPosition)>,
    current_raycast: Option<(WorldBlockCoord, I64Vec3)>,
    selected_block: BlockType,
) -> Option<BlockEditContext> {
    let (transform, world_position) = camera_transform?;
    let ray_origin = world_position.0;
    let ray_direction: Vec3 = transform.forward().into();

    Some(BlockEditContext {
        ray_origin,
        ray_direction: ray_direction.as_dvec3(),
        foot_position: ray_origin - DVec3::Y * EYE_HEIGHT,
        current_raycast,
        selected_block,
    })
}

fn apply_edit_action(
    edit_action: EditAction,
    context: BlockEditContext,
    voxel_world: &mut VoxelWorld,
    render_sync_queue: &mut RenderSyncQueue,
) {
    match edit_action {
        EditAction::Remove => {
            let _ =
                remove_highlighted_block(voxel_world, render_sync_queue, context.current_raycast);
        }
        EditAction::Place => {
            let _ = place_block_at_target(
                voxel_world,
                render_sync_queue,
                context.selected_block,
                context.current_raycast,
                context.foot_position,
            );
        }
        EditAction::RemoveThenPlace => {
            // Left+right in the same frame is treated as "replace the hit
            // block with the selected block on the newly exposed face".
            if remove_highlighted_block(voxel_world, render_sync_queue, context.current_raycast)
                .is_some()
            {
                let replacement_target =
                    raycast_voxel(voxel_world, context.ray_origin, context.ray_direction, 8.0);
                let _ = place_block_at_target(
                    voxel_world,
                    render_sync_queue,
                    context.selected_block,
                    replacement_target,
                    context.foot_position,
                );
            }
        }
    }
}

fn compute_highlight_target(
    cursor_options: Option<&CursorOptions>,
    camera_transform: Option<(&Transform, &WorldPosition)>,
    voxel_world: &VoxelWorld,
) -> Option<(WorldBlockCoord, I64Vec3)> {
    if !cursor_is_locked(cursor_options) {
        return None;
    }

    let (transform, world_position) = camera_transform?;
    let ray_origin = world_position.0;
    let ray_direction: Vec3 = transform.forward().into();
    raycast_voxel(voxel_world, ray_origin, ray_direction.as_dvec3(), 8.0)
}

fn remove_highlighted_block(
    voxel_world: &mut VoxelWorld,
    render_sync_queue: &mut RenderSyncQueue,
    current_raycast: Option<(WorldBlockCoord, I64Vec3)>,
) -> Option<WorldBlockCoord> {
    let (hit_block, _) = current_raycast?;
    let _ = voxel_world.remove_block(hit_block)?;
    render_sync_queue.mark_with_neighbors(hit_block);
    Some(hit_block)
}

fn place_block_at_target(
    voxel_world: &mut VoxelWorld,
    render_sync_queue: &mut RenderSyncQueue,
    selected_block: BlockType,
    current_raycast: Option<(WorldBlockCoord, I64Vec3)>,
    foot_position: DVec3,
) -> bool {
    let Some((hit_block, hit_normal)) = current_raycast else {
        return false;
    };

    let place_pos = hit_block + hit_normal;
    if player_blocks_block_placement(foot_position, place_pos) {
        return false;
    }

    if !voxel_world.try_insert_block(place_pos, selected_block) {
        return false;
    }

    render_sync_queue.mark_with_neighbors(place_pos);

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::transform::TransformPlugin;
    use bevy::window::CursorGrabMode;

    use crate::world::render::RenderOriginRoot;
    use crate::world::{
        BlockEntityIndex, BlockMesh, ChunkCoord, RenderAnchor, RenderOriginRootEntity,
        sync_block_render_system, sync_render_origin_root_system,
    };

    fn world_block(x: i64, y: i64, z: i64) -> WorldBlockCoord {
        WorldBlockCoord::new(x, y, z)
    }

    fn test_block_materials() -> BlockMaterials {
        BlockMaterials {
            grass: Handle::default(),
            dirt: Handle::default(),
            stone: Handle::default(),
            highlight: Handle::default(),
        }
    }

    fn setup_interaction_app(cursor_grab_mode: CursorGrabMode) -> App {
        let mut app = App::new();
        app.insert_resource(ButtonInput::<MouseButton>::default());
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.insert_resource(VoxelWorld::default());
        app.insert_resource(RenderSyncQueue::default());
        app.insert_resource(BlockEntityIndex::default());
        app.insert_resource(BlockMesh(Handle::default()));
        app.insert_resource(test_block_materials());
        app.insert_resource(HighlightTarget::default());
        app.init_resource::<SelectedBlock>();
        app.insert_resource(RenderAnchor {
            chunk: ChunkCoord::new(0, 0),
        });
        let render_origin_root = app
            .world_mut()
            .spawn((
                RenderOriginRoot,
                Transform::default(),
                GlobalTransform::default(),
                Visibility::Visible,
                InheritedVisibility::VISIBLE,
                ViewVisibility::default(),
            ))
            .id();
        app.insert_resource(RenderOriginRootEntity(render_origin_root));
        app.add_systems(
            Update,
            (
                update_highlight_target_pre_edit_system,
                block_edit_system,
                sync_block_render_system,
                update_highlight_target_post_edit_system,
            )
                .chain(),
        );

        app.world_mut().spawn((
            PrimaryWindow,
            CursorOptions {
                grab_mode: cursor_grab_mode,
                ..Default::default()
            },
        ));
        app.world_mut().spawn((
            MainCamera,
            Transform::from_xyz(1.25, 3.62, 0.5).looking_to(Vec3::X, Vec3::Y),
            WorldPosition(DVec3::new(1.25, 3.62, 0.5)),
        ));

        app
    }

    #[test]
    fn post_edit_highlight_tracks_newly_placed_block() {
        let mut app = setup_interaction_app(CursorGrabMode::Locked);
        {
            let mut world = app.world_mut().resource_mut::<VoxelWorld>();
            let _ = world.try_insert_block(world_block(3, 3, 0), BlockType::Stone);
        }
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);

        app.update();

        let world = app.world().resource::<VoxelWorld>();
        assert_eq!(
            world.block_kind(world_block(2, 3, 0)),
            Some(BlockType::Grass)
        );
        assert_eq!(
            app.world().resource::<HighlightTarget>().0,
            Some((world_block(2, 3, 0), I64Vec3::new(-1, 0, 0)))
        );
    }

    #[test]
    fn remove_then_place_updates_highlight_same_frame() {
        let mut app = setup_interaction_app(CursorGrabMode::Locked);
        {
            let mut world = app.world_mut().resource_mut::<VoxelWorld>();
            let _ = world.try_insert_block(world_block(3, 3, 0), BlockType::Stone);
            let _ = world.try_insert_block(world_block(5, 3, 0), BlockType::Stone);
        }
        {
            let mut mouse_buttons = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
            mouse_buttons.press(MouseButton::Left);
            mouse_buttons.press(MouseButton::Right);
        }

        app.update();

        let world = app.world().resource::<VoxelWorld>();
        assert!(!world.contains_block(world_block(3, 3, 0)));
        assert_eq!(
            world.block_kind(world_block(4, 3, 0)),
            Some(BlockType::Grass)
        );
        assert_eq!(
            app.world().resource::<HighlightTarget>().0,
            Some((world_block(4, 3, 0), I64Vec3::new(-1, 0, 0)))
        );
    }

    #[test]
    fn unlocked_cursor_keeps_highlight_target_none() {
        let mut app = setup_interaction_app(CursorGrabMode::None);
        {
            let mut world = app.world_mut().resource_mut::<VoxelWorld>();
            let _ = world.try_insert_block(world_block(3, 3, 0), BlockType::Stone);
        }
        app.world_mut().insert_resource(HighlightTarget(Some((
            world_block(99, 99, 99),
            I64Vec3::new(1, 0, 0),
        ))));

        app.update();

        assert_eq!(app.world().resource::<HighlightTarget>().0, None);
    }

    #[test]
    fn unlocked_cursor_blocks_edits_even_with_stale_highlight() {
        let mut app = App::new();
        app.insert_resource(ButtonInput::<MouseButton>::default());
        app.insert_resource(VoxelWorld::default());
        app.insert_resource(RenderSyncQueue::default());
        app.insert_resource(HighlightTarget(Some((world_block(3, 3, 0), I64Vec3::X))));
        app.insert_resource(SelectedBlock(BlockType::Grass));
        app.add_systems(Update, block_edit_system);

        app.world_mut().spawn((
            PrimaryWindow,
            CursorOptions {
                grab_mode: CursorGrabMode::None,
                ..Default::default()
            },
        ));
        app.world_mut().spawn((
            MainCamera,
            Transform::from_xyz(1.25, 3.62, 0.5).looking_to(Vec3::X, Vec3::Y),
            WorldPosition(DVec3::new(1.25, 3.62, 0.5)),
        ));
        app.world_mut()
            .resource_mut::<VoxelWorld>()
            .try_insert_block(world_block(3, 3, 0), BlockType::Stone);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);

        app.update();

        assert_eq!(
            app.world()
                .resource::<VoxelWorld>()
                .block_kind(world_block(3, 3, 0)),
            Some(BlockType::Stone)
        );
    }

    #[test]
    fn highlighter_global_transform_is_camera_relative() {
        let mut app = App::new();
        app.add_plugins(TransformPlugin);
        app.insert_resource(VoxelWorld::default());
        app.insert_resource(HighlightTarget(Some((world_block(10, 0, 0), I64Vec3::X))));
        app.insert_resource(RenderAnchor {
            chunk: ChunkCoord::new(0, 0),
        });
        let render_origin_root = app
            .world_mut()
            .spawn((
                RenderOriginRoot,
                Transform::default(),
                GlobalTransform::default(),
                Visibility::Visible,
                InheritedVisibility::VISIBLE,
                ViewVisibility::default(),
            ))
            .id();
        app.insert_resource(RenderOriginRootEntity(render_origin_root));
        app.add_systems(
            Update,
            (highlight_system, sync_render_origin_root_system).chain(),
        );
        let materials = test_block_materials();
        let mesh = Handle::default();
        app.add_systems(Startup, move |mut commands: Commands| {
            commands.entity(render_origin_root).with_children(|parent| {
                parent.spawn((
                    MainCamera,
                    Camera3d::default(),
                    Transform::from_translation(Vec3::new(8.0, 0.0, 0.0)),
                    WorldPosition(DVec3::new(8.0, 0.0, 0.0)),
                ));
            });
            spawn_block_highlighter(&mut commands, &materials, &mesh, render_origin_root);
        });

        app.update();

        let mut highlighter_query = app
            .world_mut()
            .query_filtered::<&GlobalTransform, With<BlockHighlighter>>();
        let highlighter_transform = highlighter_query
            .single(app.world_mut())
            .expect("highlighter should exist");
        assert_eq!(
            highlighter_transform.translation(),
            Vec3::new(2.0, 0.0, 0.0)
        );
    }
}
