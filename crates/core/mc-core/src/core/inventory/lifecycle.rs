use super::super::{DroppedItemEntity, OnlinePlayer, ServerCore};
use super::crafting::{
    recompute_crafting_result_for_active_container, recompute_player_crafting_result,
};
use super::furnace::{normalize_furnace_window, tick_furnace_window};
use super::state::{
    CHEST_LOCAL_SLOT_COUNT, CRAFTING_TABLE_LOCAL_SLOT_COUNT, ChestWindowState, FurnaceWindowState,
    OpenInventoryWindow, OpenInventoryWindowState,
};
use super::sync::{inventory_diff_events, property_diff_events, property_events};
use super::util::merge_stack_into_player_inventory;
use crate::catalog;
use crate::events::{CoreEvent, EventTarget, TargetedEvent};
use crate::inventory::{
    InventoryContainer, InventorySlot, InventoryWindowContents, ItemStack, PlayerInventory,
};
use crate::world::{BlockEntityState, BlockPos, BlockState, Vec3};
use crate::{EntityId, PlayerId, PlayerSnapshot};

const DROPPED_ITEM_PICKUP_RADIUS_SQUARED: f64 = 1.5 * 1.5;
const DROPPED_ITEM_PHYSICS_STEP_MS: u64 = 50;
const DROPPED_ITEM_GRAVITY_PER_STEP: f64 = 0.04;
const DROPPED_ITEM_DRAG: f64 = 0.98;
const DROPPED_ITEM_REST_HEIGHT: f64 = 0.25;

impl ServerCore {
    pub fn open_crafting_table(
        &mut self,
        player_id: PlayerId,
        window_id: u8,
        title: impl Into<String>,
    ) -> Vec<TargetedEvent> {
        self.open_non_player_window(
            player_id,
            OpenInventoryWindow {
                window_id,
                container: InventoryContainer::CraftingTable,
                state: OpenInventoryWindowState::CraftingTable {
                    slots: vec![None; CRAFTING_TABLE_LOCAL_SLOT_COUNT],
                },
            },
            title,
        )
    }

    pub fn open_furnace(
        &mut self,
        player_id: PlayerId,
        window_id: u8,
        title: impl Into<String>,
    ) -> Vec<TargetedEvent> {
        self.open_non_player_window(
            player_id,
            OpenInventoryWindow {
                window_id,
                container: InventoryContainer::Furnace,
                state: OpenInventoryWindowState::Furnace(FurnaceWindowState::new_virtual()),
            },
            title,
        )
    }

    pub fn open_chest(
        &mut self,
        player_id: PlayerId,
        window_id: u8,
        title: impl Into<String>,
    ) -> Vec<TargetedEvent> {
        self.open_non_player_window(
            player_id,
            OpenInventoryWindow {
                window_id,
                container: InventoryContainer::Chest,
                state: OpenInventoryWindowState::Chest(ChestWindowState::new_virtual(
                    CHEST_LOCAL_SLOT_COUNT,
                )),
            },
            title,
        )
    }

    pub(in crate::core) fn open_world_chest(
        &mut self,
        player_id: PlayerId,
        position: BlockPos,
    ) -> Vec<TargetedEvent> {
        if self.block_at(position).key.as_str() != catalog::CHEST {
            return Vec::new();
        }
        let slots = self
            .block_entities
            .entry(position)
            .or_insert_with(|| BlockEntityState::chest(CHEST_LOCAL_SLOT_COUNT))
            .chest_slots()
            .expect("chest block entity should expose slots")
            .to_vec();
        let Some(window_id) = self.allocate_non_player_window_id(player_id) else {
            return Vec::new();
        };
        self.open_non_player_window(
            player_id,
            OpenInventoryWindow {
                window_id,
                container: InventoryContainer::Chest,
                state: OpenInventoryWindowState::Chest(ChestWindowState::new_block(
                    position, slots,
                )),
            },
            "Chest",
        )
    }

    pub(in crate::core) fn open_world_furnace(
        &mut self,
        player_id: PlayerId,
        position: BlockPos,
    ) -> Vec<TargetedEvent> {
        if self.block_at(position).key.as_str() != catalog::FURNACE {
            return Vec::new();
        }
        let block_entity = self
            .block_entities
            .entry(position)
            .or_insert_with(BlockEntityState::furnace)
            .clone();
        let Some(window_id) = self.allocate_non_player_window_id(player_id) else {
            return Vec::new();
        };
        self.open_non_player_window(
            player_id,
            OpenInventoryWindow {
                window_id,
                container: InventoryContainer::Furnace,
                state: OpenInventoryWindowState::Furnace(FurnaceWindowState::new_block(
                    position,
                    &block_entity,
                )),
            },
            "Furnace",
        )
    }

    pub(in crate::core) fn close_inventory_window(
        &mut self,
        player_id: PlayerId,
        window_id: u8,
    ) -> Vec<TargetedEvent> {
        if window_id == 0 {
            return Vec::new();
        }
        let Some(active_window_id) = self.online_players.get(&player_id).and_then(|player| {
            player
                .active_container
                .as_ref()
                .map(|window| window.window_id)
        }) else {
            return Vec::new();
        };
        if active_window_id != window_id {
            return Vec::new();
        }
        self.close_player_active_container(player_id, true)
    }

    pub(in crate::core) fn persisted_online_player_snapshot(
        player: &OnlinePlayer,
    ) -> PlayerSnapshot {
        persist_online_window_state(player)
    }

    pub(in crate::core) fn recompute_crafting_result_for_inventory(
        inventory: &mut PlayerInventory,
    ) {
        recompute_player_crafting_result(inventory);
    }

    pub(in crate::core) fn tick_active_containers(&mut self) -> Vec<TargetedEvent> {
        let mut events = Vec::new();
        let player_ids = self.online_players.keys().copied().collect::<Vec<_>>();
        for player_id in player_ids {
            let Some(before_player) = self.online_players.get(&player_id) else {
                continue;
            };
            let Some(before_window) = before_player.active_container.as_ref() else {
                continue;
            };
            if before_window.container != InventoryContainer::Furnace {
                continue;
            }

            let before_contents = before_window.contents(&before_player.snapshot.inventory);
            let before_properties = before_window.property_entries();
            let window_id = before_window.window_id;

            let after = match self.online_players.get_mut(&player_id) {
                Some(player) => match player.active_container.as_mut() {
                    Some(window) if window.container == InventoryContainer::Furnace => {
                        tick_furnace_window(window);
                        Some((
                            window.contents(&player.snapshot.inventory),
                            window.property_entries(),
                        ))
                    }
                    _ => None,
                },
                None => None,
            };

            let Some((after_contents, after_properties)) = after else {
                continue;
            };

            if let Some((position, block_entity)) = self
                .online_players
                .get(&player_id)
                .and_then(|player| player.active_container.as_ref())
                .and_then(OpenInventoryWindow::world_block_entity)
                && matches!(block_entity, BlockEntityState::Furnace { .. })
                && self.block_at(position).key.as_str() == catalog::FURNACE
            {
                self.block_entities.insert(position, block_entity);
            }

            events.extend(inventory_diff_events(
                window_id,
                InventoryContainer::Furnace,
                player_id,
                &before_contents,
                &after_contents,
            ));
            events.extend(property_diff_events(
                window_id,
                player_id,
                &before_properties,
                &after_properties,
            ));
        }
        events
    }

    pub(in crate::core) fn tick_dropped_items(&mut self, now_ms: u64) -> Vec<TargetedEvent> {
        let mut events = Vec::new();
        let mut despawned = Vec::new();
        let item_ids = self.dropped_items.keys().copied().collect::<Vec<_>>();

        for entity_id in item_ids {
            let Some(mut item) = self.dropped_items.remove(&entity_id) else {
                continue;
            };
            self.advance_dropped_item_entity(&mut item, now_ms);
            if now_ms >= item.despawn_at_ms {
                despawned.push(entity_id);
                continue;
            }
            if now_ms < item.pickup_allowed_at_ms {
                self.dropped_items.insert(entity_id, item);
                continue;
            }
            let Some(player_id) =
                nearest_pickup_player(&self.online_players, item.snapshot.position)
            else {
                self.dropped_items.insert(entity_id, item);
                continue;
            };
            let (pickup_events, leftover) = self
                .merge_stack_into_online_player_inventory(player_id, item.snapshot.item.clone());
            events.extend(pickup_events);
            match leftover {
                Some(leftover) => {
                    item.snapshot.item = leftover;
                    self.dropped_items.insert(entity_id, item);
                }
                None => despawned.push(entity_id),
            }
        }

        if !despawned.is_empty() {
            despawned.sort();
            despawned.dedup();
            for entity_id in &despawned {
                self.dropped_items.remove(entity_id);
            }
            events.extend(self.broadcast_entity_despawn(&despawned));
        }

        events
    }

    fn advance_dropped_item_entity(&self, item: &mut DroppedItemEntity, now_ms: u64) {
        let elapsed_ms = now_ms.saturating_sub(item.last_updated_at_ms);
        let step_count = elapsed_ms / DROPPED_ITEM_PHYSICS_STEP_MS;
        if step_count == 0 {
            return;
        }

        for _ in 0..step_count {
            self.advance_dropped_item_step(item);
        }

        item.last_updated_at_ms = item
            .last_updated_at_ms
            .saturating_add(step_count.saturating_mul(DROPPED_ITEM_PHYSICS_STEP_MS));
    }

    fn advance_dropped_item_step(&self, item: &mut DroppedItemEntity) {
        let next_velocity_y =
            (item.snapshot.velocity.y - DROPPED_ITEM_GRAVITY_PER_STEP) * DROPPED_ITEM_DRAG;
        let next_y = item.snapshot.position.y + next_velocity_y;
        if let Some(rest_y) =
            self.dropped_item_rest_y(item.snapshot.position.x, next_y, item.snapshot.position.z)
            && next_y <= rest_y
        {
            item.snapshot.position.y = rest_y;
            item.snapshot.velocity.y = 0.0;
            return;
        }

        item.snapshot.position.y = next_y;
        item.snapshot.velocity.y = next_velocity_y;
    }

    fn dropped_item_rest_y(&self, x: f64, y: f64, z: f64) -> Option<f64> {
        let block_x = x.floor() as i32;
        let block_z = z.floor() as i32;
        let max_block_y = (y.floor() as i32).clamp(0, 255);
        for block_y in (0..=max_block_y).rev() {
            let position = BlockPos::new(block_x, block_y, block_z);
            if self.block_at(position).is_air() {
                continue;
            }
            return Some(f64::from(block_y) + 1.0 + DROPPED_ITEM_REST_HEIGHT);
        }
        None
    }

    pub(in crate::core) fn unregister_world_chest_viewer(
        &mut self,
        position: BlockPos,
        player_id: PlayerId,
    ) {
        let Some(viewers) = self.chest_viewers.get_mut(&position) else {
            return;
        };
        viewers.remove(&player_id);
        if viewers.is_empty() {
            self.chest_viewers.remove(&position);
        }
    }

    fn merge_stack_into_online_player_inventory(
        &mut self,
        player_id: PlayerId,
        stack: ItemStack,
    ) -> (Vec<TargetedEvent>, Option<ItemStack>) {
        let Some(before_player) = self.online_players.get(&player_id) else {
            return (Vec::new(), Some(stack));
        };
        let (window_id, container) = before_player
            .active_container
            .as_ref()
            .map(|window| (window.window_id, window.container))
            .unwrap_or((0, InventoryContainer::Player));
        let before_contents = before_player
            .active_container
            .as_ref()
            .map(|window| window.contents(&before_player.snapshot.inventory))
            .unwrap_or_else(|| {
                InventoryWindowContents::player(before_player.snapshot.inventory.clone())
            });

        let leftover = {
            let player = self
                .online_players
                .get_mut(&player_id)
                .expect("online player should still exist");
            let leftover = merge_stack_into_player_inventory(&mut player.snapshot.inventory, stack);
            self.saved_players
                .insert(player_id, player.snapshot.clone());
            leftover
        };

        let Some(after_player) = self.online_players.get(&player_id) else {
            return (Vec::new(), leftover);
        };
        let after_contents = after_player
            .active_container
            .as_ref()
            .map(|window| window.contents(&after_player.snapshot.inventory))
            .unwrap_or_else(|| {
                InventoryWindowContents::player(after_player.snapshot.inventory.clone())
            });

        (
            inventory_diff_events(
                window_id,
                container,
                player_id,
                &before_contents,
                &after_contents,
            ),
            leftover,
        )
    }

    fn broadcast_entity_despawn(&self, entity_ids: &[EntityId]) -> Vec<TargetedEvent> {
        self.online_players
            .keys()
            .copied()
            .map(|player_id| TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::EntityDespawned {
                    entity_ids: entity_ids.to_vec(),
                },
            })
            .collect()
    }

    pub(in crate::core) fn close_world_container_if_invalid(
        &mut self,
        position: BlockPos,
        block: &BlockState,
    ) -> Vec<TargetedEvent> {
        let mut events = Vec::new();

        let had_chest_block_entity = matches!(
            self.block_entities.get(&position),
            Some(BlockEntityState::Chest { .. })
        );
        if block.key.as_str() != catalog::CHEST
            && (had_chest_block_entity || self.chest_viewers.contains_key(&position))
        {
            self.block_entities.remove(&position);
            events.extend(self.close_world_chest_viewers(position));
        }

        let had_furnace_block_entity = matches!(
            self.block_entities.get(&position),
            Some(BlockEntityState::Furnace { .. })
        );
        if block.key.as_str() != catalog::FURNACE
            && (had_furnace_block_entity || self.has_world_furnace_viewers(position))
        {
            self.block_entities.remove(&position);
            events.extend(self.close_world_furnace_viewers(position));
        }

        events
    }

    pub(super) fn sync_world_chest_viewers(
        &mut self,
        position: BlockPos,
        actor_player_id: PlayerId,
    ) -> Vec<TargetedEvent> {
        let Some(slots) = self
            .online_players
            .get(&actor_player_id)
            .and_then(|player| player.active_container.as_ref())
            .and_then(|window| match &window.state {
                OpenInventoryWindowState::Chest(chest)
                    if chest.world_position() == Some(position) =>
                {
                    Some(chest.slots.clone())
                }
                _ => None,
            })
        else {
            return Vec::new();
        };

        self.block_entities.insert(
            position,
            BlockEntityState::Chest {
                slots: slots.clone(),
            },
        );

        let viewer_ids = self
            .chest_viewers
            .get(&position)
            .map(|viewers| viewers.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut stale_viewers = Vec::new();
        let mut events = Vec::new();
        for viewer_id in viewer_ids {
            if viewer_id == actor_player_id {
                continue;
            }
            let Some(player) = self.online_players.get_mut(&viewer_id) else {
                stale_viewers.push(viewer_id);
                continue;
            };
            let Some(window) = player.active_container.as_mut() else {
                stale_viewers.push(viewer_id);
                continue;
            };
            let Some(chest) = (match &mut window.state {
                OpenInventoryWindowState::Chest(chest)
                    if chest.world_position() == Some(position) =>
                {
                    Some(chest)
                }
                _ => None,
            }) else {
                stale_viewers.push(viewer_id);
                continue;
            };
            let before_contents = InventoryWindowContents::with_container(
                player.snapshot.inventory.clone(),
                chest.slots.clone(),
            );
            chest.slots = slots.clone();
            let after_contents = window.contents(&player.snapshot.inventory);
            events.extend(inventory_diff_events(
                window.window_id,
                InventoryContainer::Chest,
                viewer_id,
                &before_contents,
                &after_contents,
            ));
        }

        for stale_viewer in stale_viewers {
            self.unregister_world_chest_viewer(position, stale_viewer);
        }
        events
    }

    pub(super) fn sync_world_furnace_state(
        &mut self,
        position: BlockPos,
        actor_player_id: PlayerId,
    ) {
        let Some((_, block_entity)) = self
            .online_players
            .get(&actor_player_id)
            .and_then(|player| player.active_container.as_ref())
            .and_then(OpenInventoryWindow::world_block_entity)
            .filter(|(window_position, _)| *window_position == position)
        else {
            return;
        };
        if self.block_at(position).key.as_str() == catalog::FURNACE {
            self.block_entities.insert(position, block_entity);
        }
    }

    fn open_non_player_window(
        &mut self,
        player_id: PlayerId,
        mut window: OpenInventoryWindow,
        title: impl Into<String>,
    ) -> Vec<TargetedEvent> {
        if !self.online_players.contains_key(&player_id) {
            return Vec::new();
        }

        match window.container {
            InventoryContainer::CraftingTable => {
                recompute_crafting_result_for_active_container(&mut window);
            }
            InventoryContainer::Furnace => normalize_furnace_window(&mut window),
            InventoryContainer::Chest | InventoryContainer::Player => {}
        }

        let mut events = self.close_player_active_container(player_id, false);

        let title = title.into();
        let properties = window.property_entries();
        let window_id = window.window_id;
        let container = window.container;
        let world_chest_position = window.world_chest_position();
        let Some(contents) = ({
            let Some(player) = self.online_players.get_mut(&player_id) else {
                return events;
            };
            let contents = window.contents(&player.snapshot.inventory);
            player.active_container = Some(window);
            self.saved_players
                .insert(player_id, player.snapshot.clone());
            Some(contents)
        }) else {
            return events;
        };
        if let Some(position) = world_chest_position {
            self.register_world_chest_viewer(position, player_id, window_id);
        }

        events.extend([
            TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::ContainerOpened {
                    window_id,
                    container,
                    title,
                },
            },
            TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventoryContents {
                    window_id,
                    container,
                    contents,
                },
            },
        ]);
        events.extend(property_events(window_id, player_id, &properties));
        events
    }

    fn allocate_non_player_window_id(&mut self, player_id: PlayerId) -> Option<u8> {
        let player = self.online_players.get_mut(&player_id)?;
        let window_id = player.next_non_player_window_id.max(1);
        player.next_non_player_window_id = if window_id == u8::MAX {
            1
        } else {
            window_id + 1
        };
        Some(window_id)
    }

    fn register_world_chest_viewer(
        &mut self,
        position: BlockPos,
        player_id: PlayerId,
        window_id: u8,
    ) {
        self.chest_viewers
            .entry(position)
            .or_default()
            .insert(player_id, window_id);
    }

    fn close_world_chest_viewers(&mut self, position: BlockPos) -> Vec<TargetedEvent> {
        let viewer_ids = self
            .chest_viewers
            .get(&position)
            .map(|viewers| viewers.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut events = Vec::new();
        for viewer_id in viewer_ids {
            events.extend(self.close_player_active_container(viewer_id, true));
        }
        self.chest_viewers.remove(&position);
        events
    }

    fn has_world_furnace_viewers(&self, position: BlockPos) -> bool {
        self.online_players.values().any(|player| {
            player
                .active_container
                .as_ref()
                .and_then(OpenInventoryWindow::world_furnace_position)
                == Some(position)
        })
    }

    fn close_world_furnace_viewers(&mut self, position: BlockPos) -> Vec<TargetedEvent> {
        let viewer_ids = self
            .online_players
            .iter()
            .filter_map(|(player_id, player)| {
                (player
                    .active_container
                    .as_ref()
                    .and_then(OpenInventoryWindow::world_furnace_position)
                    == Some(position))
                .then_some(*player_id)
            })
            .collect::<Vec<_>>();
        let mut events = Vec::new();
        for viewer_id in viewer_ids {
            events.extend(self.close_player_active_container(viewer_id, true));
        }
        events
    }

    fn close_player_active_container(
        &mut self,
        player_id: PlayerId,
        include_player_contents: bool,
    ) -> Vec<TargetedEvent> {
        let Some((window_id, world_block_entity, world_chest_position, contents)) = ({
            let Some(player) = self.online_players.get_mut(&player_id) else {
                return Vec::new();
            };
            let Some(window) = close_active_container_window(player) else {
                return Vec::new();
            };
            let world_block_entity = window.world_block_entity();
            let contents = include_player_contents
                .then(|| InventoryWindowContents::player(player.snapshot.inventory.clone()));
            self.saved_players
                .insert(player_id, player.snapshot.clone());
            Some((
                window.window_id,
                world_block_entity,
                window.world_chest_position(),
                contents,
            ))
        }) else {
            return Vec::new();
        };

        if let Some((position, block_entity)) = world_block_entity {
            let expected_block_key = match &block_entity {
                BlockEntityState::Chest { .. } => catalog::CHEST,
                BlockEntityState::Furnace { .. } => catalog::FURNACE,
            };
            if self.block_at(position).key.as_str() == expected_block_key {
                self.block_entities.insert(position, block_entity);
            }
        }
        if let Some(position) = world_chest_position {
            self.unregister_world_chest_viewer(position, player_id);
        }

        let mut events = vec![TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::ContainerClosed { window_id },
        }];
        if let Some(contents) = contents {
            events.push(TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::InventoryContents {
                    window_id: 0,
                    container: InventoryContainer::Player,
                    contents,
                },
            });
        }
        events
    }
}

pub(crate) fn world_chest_position(window: &OpenInventoryWindow) -> Option<BlockPos> {
    window.world_chest_position()
}

pub(crate) fn world_block_entity(
    window: &OpenInventoryWindow,
) -> Option<(BlockPos, BlockEntityState)> {
    window.world_block_entity()
}

fn persist_online_window_state(player: &OnlinePlayer) -> PlayerSnapshot {
    let mut persisted = player.snapshot.clone();
    if let Some(window) = player.active_container.as_ref() {
        fold_active_container_items_into_player(&mut persisted.inventory, window);
    }

    let transient_slots = [
        InventorySlot::crafting_result(),
        InventorySlot::crafting_input(0).expect("craft slot should exist"),
        InventorySlot::crafting_input(1).expect("craft slot should exist"),
        InventorySlot::crafting_input(2).expect("craft slot should exist"),
        InventorySlot::crafting_input(3).expect("craft slot should exist"),
        InventorySlot::Auxiliary(5),
        InventorySlot::Auxiliary(6),
        InventorySlot::Auxiliary(7),
        InventorySlot::Auxiliary(8),
    ];
    let mut overflow = transient_slots
        .into_iter()
        .filter_map(|slot| persisted.inventory.get_slot(slot).cloned())
        .collect::<Vec<_>>();
    if let Some(cursor) = player.cursor.clone() {
        overflow.push(cursor);
    }
    for slot in transient_slots {
        let _ = persisted.inventory.set_slot(slot, None);
    }
    for stack in overflow {
        let _ = merge_stack_into_player_inventory(&mut persisted.inventory, stack);
    }
    recompute_player_crafting_result(&mut persisted.inventory);
    persisted
}

fn close_active_container_window(player: &mut OnlinePlayer) -> Option<OpenInventoryWindow> {
    let window = player.active_container.take()?;
    fold_active_container_items_into_player(&mut player.snapshot.inventory, &window);
    Some(window)
}

fn fold_active_container_items_into_player(
    inventory: &mut PlayerInventory,
    window: &OpenInventoryWindow,
) {
    match &window.state {
        OpenInventoryWindowState::CraftingTable { slots } => {
            for stack in slots.iter().skip(1).flatten().cloned() {
                let _ = merge_stack_into_player_inventory(inventory, stack);
            }
        }
        OpenInventoryWindowState::Chest(chest) => {
            if chest.world_position().is_some() {
                return;
            }
            for stack in chest.slots.iter().flatten().cloned() {
                let _ = merge_stack_into_player_inventory(inventory, stack);
            }
        }
        OpenInventoryWindowState::Furnace(furnace) => {
            if furnace.world_position().is_some() {
                return;
            }
            for stack in furnace.local_slots().into_iter().flatten() {
                let _ = merge_stack_into_player_inventory(inventory, stack);
            }
        }
    }
}

fn nearest_pickup_player(
    players: &std::collections::BTreeMap<PlayerId, OnlinePlayer>,
    position: Vec3,
) -> Option<PlayerId> {
    let mut best = None;
    for (player_id, player) in players {
        let distance_squared = distance_squared(player.snapshot.position, position);
        if distance_squared > DROPPED_ITEM_PICKUP_RADIUS_SQUARED {
            continue;
        }
        match best {
            Some((_, best_distance_squared)) if distance_squared >= best_distance_squared => {}
            _ => best = Some((*player_id, distance_squared)),
        }
    }
    best.map(|(player_id, _)| player_id)
}

fn distance_squared(left: Vec3, right: Vec3) -> f64 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    let dz = left.z - right.z;
    dx * dx + dy * dy + dz * dz
}
