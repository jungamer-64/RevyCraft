use super::{
    OpenInventoryWindow, ServerCore,
    inventory::{
        apply_inventory_click_state, close_inventory_window_state,
        close_player_active_container_state, inventory_diff_events, open_non_player_window_state,
        open_world_chest_state, open_world_furnace_state, property_diff_events, property_events,
        tick_active_container_state, tick_dropped_item_state, window_resync_events,
    },
    login::{finalize_login_delta, state_update_client_settings},
    mining::{
        state_advance_mining_stage, state_begin_mining, state_clear_active_mining,
        state_complete_survival_mining,
    },
    mutation::{
        state_inventory_slot, state_player_pose, state_selected_hotbar_slot, state_set_block,
        state_spawn_dropped_item,
    },
    state_backend::{BaseState, CoreStateMut},
    tick::{disconnect_player_state, schedule_keep_alive_state},
};
use crate::events::{
    CoreEvent, EventTarget, InventoryClickButton, InventoryClickTarget, InventoryClickValidation,
    InventoryTransactionContext, TargetedEvent,
};
use crate::inventory::{InventoryContainer, InventorySlot, InventoryWindowContents, ItemStack};
use crate::player::PlayerSnapshot;
use crate::world::{BlockPos, BlockState, ChunkColumn, DroppedItemSnapshot, Vec3};
use crate::{ConnectionId, EntityId, PlayerId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub(super) enum CoreOp {
    PrepareLogin {
        player_id: PlayerId,
        player: PlayerSnapshot,
        expected_entity_id: EntityId,
    },
    FinalizeLogin {
        connection_id: ConnectionId,
        player_id: PlayerId,
    },
    SetPlayerPose {
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    },
    SetSelectedHotbarSlot {
        player_id: PlayerId,
        slot: u8,
    },
    SetInventorySlot {
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    },
    SetViewDistance {
        player_id: PlayerId,
        view_distance: u8,
    },
    InventoryClick {
        player_id: PlayerId,
        transaction: InventoryTransactionContext,
        target: InventoryClickTarget,
        button: InventoryClickButton,
        validation: InventoryClickValidation,
    },
    OpenWindow {
        player_id: PlayerId,
        window: OpenInventoryWindow,
        title: String,
    },
    OpenChest {
        player_id: PlayerId,
        position: BlockPos,
    },
    OpenFurnace {
        player_id: PlayerId,
        position: BlockPos,
    },
    CloseContainer {
        player_id: PlayerId,
        window_id: u8,
        include_player_contents: bool,
    },
    ClearMining {
        player_id: PlayerId,
    },
    BeginMining {
        player_id: PlayerId,
        position: BlockPos,
        duration_ms: u64,
    },
    AdvanceMiningStage {
        player_id: PlayerId,
        entity_id: EntityId,
        position: BlockPos,
        stage: u8,
        duration_ms: u64,
    },
    CompleteMining {
        player_id: PlayerId,
        position: BlockPos,
    },
    TickFurnaceWindow {
        player_id: PlayerId,
    },
    TickDroppedItem {
        entity_id: EntityId,
    },
    SetBlock {
        position: BlockPos,
        block: BlockState,
    },
    SpawnDroppedItem {
        expected_entity_id: Option<EntityId>,
        position: Vec3,
        item: ItemStack,
    },
    KeepAliveRequested {
        player_id: PlayerId,
        keep_alive_id: i32,
    },
    DisconnectPlayer {
        player_id: PlayerId,
    },
    EmitEvent {
        target: EventTarget,
        event: CoreEvent,
    },
}

#[derive(Clone, Debug, Default)]
pub(super) struct ApplyCoreOpsOptions {
    pub(super) hidden_players: BTreeSet<PlayerId>,
}

#[derive(Clone, Debug)]
pub(super) struct ViewUpdateDelta {
    pub(super) player_id: PlayerId,
    pub(super) chunks: Vec<ChunkColumn>,
}

#[derive(Clone, Debug)]
pub(super) struct PlayerPoseDelta {
    pub(super) player_id: PlayerId,
    pub(super) entity_id: EntityId,
    pub(super) player: PlayerSnapshot,
    pub(super) chunks: Vec<ChunkColumn>,
}

#[derive(Clone, Debug)]
pub(super) struct SelectedHotbarDelta {
    pub(super) player_id: PlayerId,
    pub(super) slot: u8,
}

#[derive(Clone, Debug)]
pub(super) struct InventorySlotDelta {
    pub(super) player_id: PlayerId,
    pub(super) slot: InventorySlot,
    pub(super) stack: Option<ItemStack>,
    pub(super) crafting_result: Option<Option<ItemStack>>,
}

#[derive(Clone, Debug)]
pub(super) struct MiningProgressDelta {
    pub(super) breaker_entity_id: EntityId,
    pub(super) position: BlockPos,
    pub(super) stage: Option<u8>,
    pub(super) duration_ms: u64,
}

#[derive(Clone, Debug)]
pub(super) struct ClearMiningDelta {
    pub(super) progress: MiningProgressDelta,
}

#[derive(Clone, Debug)]
pub(super) struct BeginMiningDelta {
    pub(super) cleared: Option<ClearMiningDelta>,
    pub(super) progress: MiningProgressDelta,
}

#[derive(Clone, Debug)]
pub(super) struct CloseContainerDelta {
    pub(super) player_id: PlayerId,
    pub(super) window_id: u8,
    pub(super) contents: Option<InventoryWindowContents>,
}

#[derive(Clone, Debug)]
pub(super) struct OpenContainerDelta {
    pub(super) closed: Vec<CloseContainerDelta>,
    pub(super) player_id: PlayerId,
    pub(super) window_id: u8,
    pub(super) container: InventoryContainer,
    pub(super) title: String,
    pub(super) contents: InventoryWindowContents,
    pub(super) properties: Vec<(u8, i16)>,
}

#[derive(Clone, Debug)]
pub(super) struct WindowDiffDelta {
    pub(super) player_id: PlayerId,
    pub(super) window_id: u8,
    pub(super) container: InventoryContainer,
    pub(super) before_contents: InventoryWindowContents,
    pub(super) after_contents: InventoryWindowContents,
    pub(super) before_properties: Vec<(u8, i16)>,
    pub(super) after_properties: Vec<(u8, i16)>,
}

#[derive(Clone, Debug)]
pub(super) struct BlockDelta {
    pub(super) position: BlockPos,
    pub(super) cleared_mining: Vec<ClearMiningDelta>,
    pub(super) closed_containers: Vec<CloseContainerDelta>,
}

#[derive(Clone, Debug)]
pub(super) struct DroppedItemSpawnDelta {
    pub(super) entity_id: EntityId,
    pub(super) item: DroppedItemSnapshot,
}

#[derive(Clone, Debug)]
pub(super) struct EntityDespawnDelta {
    pub(super) entity_ids: Vec<EntityId>,
}

#[derive(Clone, Debug)]
pub(super) struct InventoryClickDelta {
    pub(super) player_id: PlayerId,
    pub(super) transaction: InventoryTransactionContext,
    pub(super) accepted: bool,
    pub(super) should_resync_on_reject: bool,
    pub(super) container: InventoryContainer,
    pub(super) window_id: u8,
    pub(super) resolved_slot: Option<InventorySlot>,
    pub(super) before_contents: InventoryWindowContents,
    pub(super) after_contents: InventoryWindowContents,
    pub(super) before_properties: Vec<(u8, i16)>,
    pub(super) after_properties: Vec<(u8, i16)>,
    pub(super) before_cursor: Option<ItemStack>,
    pub(super) after_cursor: Option<ItemStack>,
    pub(super) selected_hotbar_before: u8,
    pub(super) selected_hotbar_after: u8,
    pub(super) viewer_syncs: Vec<WindowDiffDelta>,
}

#[derive(Clone, Debug)]
pub(super) struct LoginFinalizeDelta {
    pub(super) connection_id: ConnectionId,
    pub(super) player_id: PlayerId,
    pub(super) entity_id: EntityId,
    pub(super) player: PlayerSnapshot,
    pub(super) visible_chunks: Vec<ChunkColumn>,
    pub(super) existing_players: Vec<(EntityId, PlayerSnapshot)>,
    pub(super) dropped_items: Vec<(EntityId, DroppedItemSnapshot)>,
}

#[derive(Clone, Debug)]
pub(super) enum CompleteMiningDelta {
    Cleared(ClearMiningDelta),
    Completed {
        block: BlockDelta,
        spawned_item: Option<DroppedItemSpawnDelta>,
    },
}

#[derive(Clone, Debug)]
pub(super) struct DroppedItemTickDelta {
    pub(super) inventory_delta: Option<WindowDiffDelta>,
    pub(super) despawn: Option<EntityDespawnDelta>,
}

#[derive(Clone, Debug)]
pub(super) struct KeepAliveDelta {
    pub(super) player_id: PlayerId,
    pub(super) keep_alive_id: i32,
}

#[derive(Clone, Debug)]
pub(super) struct DisconnectDelta {
    pub(super) player_id: PlayerId,
    pub(super) entity_id: EntityId,
    pub(super) cleared_mining: Option<ClearMiningDelta>,
}

#[derive(Clone, Debug)]
pub(super) enum AppliedCoreOp {
    PrepareLogin {
        player_id: PlayerId,
        entity_id: EntityId,
    },
    FinalizeLogin {
        player_id: PlayerId,
        delta: Option<LoginFinalizeDelta>,
    },
    SetPlayerPose(Option<PlayerPoseDelta>),
    SetSelectedHotbarSlot(Option<SelectedHotbarDelta>),
    SetInventorySlot(Option<InventorySlotDelta>),
    SetViewDistance(Option<ViewUpdateDelta>),
    InventoryClick(Option<InventoryClickDelta>),
    OpenContainer(Option<OpenContainerDelta>),
    CloseContainer(Option<CloseContainerDelta>),
    ClearMining(Option<ClearMiningDelta>),
    BeginMining(Option<BeginMiningDelta>),
    AdvanceMiningStage(Option<MiningProgressDelta>),
    CompleteMining(Option<CompleteMiningDelta>),
    WindowDiff(Option<WindowDiffDelta>),
    DroppedItemTick(DroppedItemTickDelta),
    SetBlock(BlockDelta),
    SpawnDroppedItem(Option<DroppedItemSpawnDelta>),
    KeepAliveRequested(Option<KeepAliveDelta>),
    DisconnectPlayer(Option<DisconnectDelta>),
    EmitEvent(TargetedEvent),
}

struct CoreEventBuilder {
    hidden_players: BTreeSet<PlayerId>,
    hidden_player_entities: BTreeMap<PlayerId, EntityId>,
}

pub(super) type CoreEvents = Vec<TargetedEvent>;

pub(super) fn apply_core_ops(
    core: &mut ServerCore,
    ops: Vec<CoreOp>,
    now_ms: u64,
    options: ApplyCoreOpsOptions,
) -> CoreEvents {
    let finalized_players = ops
        .iter()
        .filter_map(|op| match op {
            CoreOp::FinalizeLogin { player_id, .. } => Some(*player_id),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut state = BaseState::new(core);
    let mut builder = CoreEventBuilder::new(options.hidden_players);
    let mut events = Vec::new();

    for op in ops {
        let applied = reduce_core_op(&mut state, op, now_ms, &finalized_players);
        events.extend(builder.build(state.core(), applied));
    }

    events
}

pub(super) fn reduce_core_op(
    state: &mut impl CoreStateMut,
    op: CoreOp,
    now_ms: u64,
    materialized_players: &BTreeSet<PlayerId>,
) -> AppliedCoreOp {
    match op {
        CoreOp::PrepareLogin {
            player_id,
            player,
            expected_entity_id,
        } => {
            if materialized_players.contains(&player_id) {
                let entity_id = state.spawn_online_player(player, now_ms, Some(expected_entity_id));
                debug_assert_eq!(entity_id, expected_entity_id);
            }
            AppliedCoreOp::PrepareLogin {
                player_id,
                entity_id: expected_entity_id,
            }
        }
        CoreOp::FinalizeLogin {
            connection_id,
            player_id,
        } => AppliedCoreOp::FinalizeLogin {
            player_id,
            delta: finalize_login_delta(state, connection_id, player_id),
        },
        CoreOp::SetPlayerPose {
            player_id,
            position,
            yaw,
            pitch,
            on_ground,
        } => AppliedCoreOp::SetPlayerPose(state_player_pose(
            state, player_id, position, yaw, pitch, on_ground,
        )),
        CoreOp::SetSelectedHotbarSlot { player_id, slot } => {
            AppliedCoreOp::SetSelectedHotbarSlot(state_selected_hotbar_slot(state, player_id, slot))
        }
        CoreOp::SetInventorySlot {
            player_id,
            slot,
            stack,
        } => AppliedCoreOp::SetInventorySlot(state_inventory_slot(state, player_id, slot, stack)),
        CoreOp::SetViewDistance {
            player_id,
            view_distance,
        } => AppliedCoreOp::SetViewDistance(state_update_client_settings(
            state,
            player_id,
            view_distance,
        )),
        CoreOp::InventoryClick {
            player_id,
            transaction,
            target,
            button,
            validation,
        } => AppliedCoreOp::InventoryClick(apply_inventory_click_state(
            state,
            player_id,
            transaction,
            target,
            button,
            &validation,
        )),
        CoreOp::OpenWindow {
            player_id,
            window,
            title,
        } => AppliedCoreOp::OpenContainer(open_non_player_window_state(
            state, player_id, window, title,
        )),
        CoreOp::OpenChest {
            player_id,
            position,
        } => AppliedCoreOp::OpenContainer(open_world_chest_state(state, player_id, position)),
        CoreOp::OpenFurnace {
            player_id,
            position,
        } => AppliedCoreOp::OpenContainer(open_world_furnace_state(state, player_id, position)),
        CoreOp::CloseContainer {
            player_id,
            window_id,
            include_player_contents,
        } => {
            let delta = if window_id == 0 {
                close_player_active_container_state(state, player_id, include_player_contents)
            } else if include_player_contents {
                close_inventory_window_state(state, player_id, window_id)
            } else {
                let active_window_id = state.player_session(player_id).and_then(|session| {
                    session
                        .active_container
                        .as_ref()
                        .map(|window| window.window_id)
                });
                (active_window_id == Some(window_id))
                    .then(|| close_player_active_container_state(state, player_id, false))
                    .flatten()
            };
            AppliedCoreOp::CloseContainer(delta)
        }
        CoreOp::ClearMining { player_id } => {
            AppliedCoreOp::ClearMining(state_clear_active_mining(state, player_id))
        }
        CoreOp::BeginMining {
            player_id,
            position,
            duration_ms,
        } => AppliedCoreOp::BeginMining(state_begin_mining(
            state,
            player_id,
            position,
            duration_ms,
            now_ms,
        )),
        CoreOp::AdvanceMiningStage {
            player_id,
            entity_id,
            position,
            stage,
            duration_ms,
        } => AppliedCoreOp::AdvanceMiningStage(state_advance_mining_stage(
            state,
            player_id,
            entity_id,
            position,
            stage,
            duration_ms,
        )),
        CoreOp::CompleteMining {
            player_id,
            position,
        } => AppliedCoreOp::CompleteMining(state_complete_survival_mining(
            state, player_id, position, now_ms,
        )),
        CoreOp::TickFurnaceWindow { player_id } => {
            AppliedCoreOp::WindowDiff(tick_active_container_state(state, player_id))
        }
        CoreOp::TickDroppedItem { entity_id } => {
            AppliedCoreOp::DroppedItemTick(tick_dropped_item_state(state, entity_id, now_ms))
        }
        CoreOp::SetBlock { position, block } => {
            AppliedCoreOp::SetBlock(state_set_block(state, position, block))
        }
        CoreOp::SpawnDroppedItem {
            expected_entity_id,
            position,
            item,
        } => AppliedCoreOp::SpawnDroppedItem(state_spawn_dropped_item(
            state,
            expected_entity_id,
            position,
            item,
            now_ms,
        )),
        CoreOp::KeepAliveRequested {
            player_id,
            keep_alive_id,
        } => AppliedCoreOp::KeepAliveRequested(schedule_keep_alive_state(
            state,
            player_id,
            keep_alive_id,
            now_ms,
        )),
        CoreOp::DisconnectPlayer { player_id } => {
            AppliedCoreOp::DisconnectPlayer(disconnect_player_state(state, player_id))
        }
        CoreOp::EmitEvent { target, event } => {
            AppliedCoreOp::EmitEvent(TargetedEvent { target, event })
        }
    }
}

pub(super) fn build_applied_core_events(
    core: &ServerCore,
    applied_ops: impl IntoIterator<Item = AppliedCoreOp>,
    options: ApplyCoreOpsOptions,
) -> CoreEvents {
    let mut builder = CoreEventBuilder::new(options.hidden_players);
    let mut events = Vec::new();
    for applied in applied_ops {
        events.extend(builder.build(core, applied));
    }
    events
}

impl CoreEventBuilder {
    fn new(hidden_players: BTreeSet<PlayerId>) -> Self {
        Self {
            hidden_players,
            hidden_player_entities: BTreeMap::new(),
        }
    }

    fn build(&mut self, core: &ServerCore, applied: AppliedCoreOp) -> Vec<TargetedEvent> {
        let events = match applied {
            AppliedCoreOp::PrepareLogin {
                player_id,
                entity_id,
            } => {
                self.hidden_players.insert(player_id);
                self.hidden_player_entities.insert(player_id, entity_id);
                Vec::new()
            }
            AppliedCoreOp::FinalizeLogin { player_id, delta } => {
                self.hidden_players.remove(&player_id);
                self.hidden_player_entities.remove(&player_id);
                delta
                    .map(|delta| self.build_finalize_login(core, delta))
                    .unwrap_or_default()
            }
            AppliedCoreOp::SetPlayerPose(delta) => delta
                .map(|delta| self.build_player_pose(delta))
                .unwrap_or_default(),
            AppliedCoreOp::SetSelectedHotbarSlot(delta) => delta
                .map(|delta| self.build_selected_hotbar(delta))
                .unwrap_or_default(),
            AppliedCoreOp::SetInventorySlot(delta) => delta
                .map(|delta| self.build_inventory_slot(delta))
                .unwrap_or_default(),
            AppliedCoreOp::SetViewDistance(delta) => delta
                .map(|delta| self.build_view_update(delta))
                .unwrap_or_default(),
            AppliedCoreOp::InventoryClick(delta) => delta
                .map(|delta| self.build_inventory_click(delta))
                .unwrap_or_default(),
            AppliedCoreOp::OpenContainer(delta) => delta
                .map(|delta| self.build_open_container(delta))
                .unwrap_or_default(),
            AppliedCoreOp::CloseContainer(delta) => delta
                .map(|delta| self.build_close_container(delta))
                .unwrap_or_default(),
            AppliedCoreOp::ClearMining(delta) => delta
                .map(|delta| self.build_clear_mining(core, delta))
                .unwrap_or_default(),
            AppliedCoreOp::BeginMining(delta) => delta
                .map(|delta| self.build_begin_mining(core, delta))
                .unwrap_or_default(),
            AppliedCoreOp::AdvanceMiningStage(delta) => delta
                .map(|delta| self.build_mining_progress(core, delta))
                .unwrap_or_default(),
            AppliedCoreOp::CompleteMining(delta) => delta
                .map(|delta| self.build_complete_mining(core, delta))
                .unwrap_or_default(),
            AppliedCoreOp::WindowDiff(delta) => delta
                .map(|delta| self.build_window_diff(delta))
                .unwrap_or_default(),
            AppliedCoreOp::DroppedItemTick(delta) => self.build_dropped_item_tick(core, delta),
            AppliedCoreOp::SetBlock(delta) => self.build_set_block(core, delta),
            AppliedCoreOp::SpawnDroppedItem(delta) => delta
                .map(|delta| self.build_dropped_item_spawn(core, delta))
                .unwrap_or_default(),
            AppliedCoreOp::KeepAliveRequested(delta) => delta
                .map(|delta| self.build_keep_alive(delta))
                .unwrap_or_default(),
            AppliedCoreOp::DisconnectPlayer(delta) => delta
                .map(|delta| self.build_disconnect(core, delta))
                .unwrap_or_default(),
            AppliedCoreOp::EmitEvent(event) => vec![event],
        };
        self.filter_hidden_events(core, events)
    }

    fn build_finalize_login(
        &self,
        core: &ServerCore,
        delta: LoginFinalizeDelta,
    ) -> Vec<TargetedEvent> {
        let mut events = login_initial_events(
            core,
            delta.connection_id,
            delta.player_id,
            delta.entity_id,
            &delta.player,
            delta.visible_chunks,
        );
        events.extend(existing_player_spawn_events(
            delta.connection_id,
            delta.existing_players,
        ));
        events.extend(
            delta
                .dropped_items
                .into_iter()
                .map(|(entity_id, item)| TargetedEvent {
                    target: EventTarget::Connection(delta.connection_id),
                    event: CoreEvent::DroppedItemSpawned { entity_id, item },
                }),
        );
        events.push(TargetedEvent {
            target: EventTarget::EveryoneExcept(delta.player_id),
            event: CoreEvent::EntitySpawned {
                entity_id: delta.entity_id,
                player: delta.player,
            },
        });
        events
    }

    fn build_player_pose(&self, delta: PlayerPoseDelta) -> Vec<TargetedEvent> {
        let mut events = delta
            .chunks
            .into_iter()
            .map(|chunk| TargetedEvent {
                target: EventTarget::Player(delta.player_id),
                event: CoreEvent::ChunkBatch {
                    chunks: vec![chunk],
                },
            })
            .collect::<Vec<_>>();
        events.push(TargetedEvent {
            target: EventTarget::EveryoneExcept(delta.player_id),
            event: CoreEvent::EntityMoved {
                entity_id: delta.entity_id,
                player: delta.player,
            },
        });
        events
    }

    fn build_selected_hotbar(&self, delta: SelectedHotbarDelta) -> Vec<TargetedEvent> {
        vec![TargetedEvent {
            target: EventTarget::Player(delta.player_id),
            event: CoreEvent::SelectedHotbarSlotChanged { slot: delta.slot },
        }]
    }

    fn build_inventory_slot(&self, delta: InventorySlotDelta) -> Vec<TargetedEvent> {
        let mut events = vec![TargetedEvent {
            target: EventTarget::Player(delta.player_id),
            event: CoreEvent::InventorySlotChanged {
                window_id: 0,
                container: InventoryContainer::Player,
                slot: delta.slot,
                stack: delta.stack,
            },
        }];
        if let Some(crafting_result) = delta.crafting_result {
            events.push(TargetedEvent {
                target: EventTarget::Player(delta.player_id),
                event: CoreEvent::InventorySlotChanged {
                    window_id: 0,
                    container: InventoryContainer::Player,
                    slot: InventorySlot::crafting_result(),
                    stack: crafting_result,
                },
            });
        }
        events
    }

    fn build_view_update(&self, delta: ViewUpdateDelta) -> Vec<TargetedEvent> {
        delta
            .chunks
            .into_iter()
            .map(|chunk| TargetedEvent {
                target: EventTarget::Player(delta.player_id),
                event: CoreEvent::ChunkBatch {
                    chunks: vec![chunk],
                },
            })
            .collect()
    }

    fn build_inventory_click(&self, delta: InventoryClickDelta) -> Vec<TargetedEvent> {
        let mut events = vec![TargetedEvent {
            target: EventTarget::Player(delta.player_id),
            event: CoreEvent::InventoryTransactionProcessed {
                transaction: delta.transaction,
                accepted: delta.accepted,
            },
        }];

        if !delta.accepted {
            if !delta.should_resync_on_reject {
                return events;
            }
            events.extend(window_resync_events(
                delta.player_id,
                delta.window_id,
                delta.container,
                &delta.after_contents,
                delta.selected_hotbar_after,
                delta.after_cursor.as_ref(),
                delta.resolved_slot,
            ));
            events.extend(property_diff_events(
                delta.window_id,
                delta.player_id,
                &delta.before_properties,
                &delta.after_properties,
            ));
            return events;
        }

        events.extend(inventory_diff_events(
            delta.window_id,
            delta.container,
            delta.player_id,
            &delta.before_contents,
            &delta.after_contents,
        ));
        events.extend(property_diff_events(
            delta.window_id,
            delta.player_id,
            &delta.before_properties,
            &delta.after_properties,
        ));
        if delta.before_cursor != delta.after_cursor {
            events.push(TargetedEvent {
                target: EventTarget::Player(delta.player_id),
                event: CoreEvent::CursorChanged {
                    stack: delta.after_cursor,
                },
            });
        }
        if delta.container == InventoryContainer::Player
            && delta.selected_hotbar_before != delta.selected_hotbar_after
        {
            events.push(TargetedEvent {
                target: EventTarget::Player(delta.player_id),
                event: CoreEvent::SelectedHotbarSlotChanged {
                    slot: delta.selected_hotbar_after,
                },
            });
        }
        for viewer_sync in delta.viewer_syncs {
            events.extend(self.build_window_diff(viewer_sync));
        }
        events
    }

    fn build_open_container(&self, delta: OpenContainerDelta) -> Vec<TargetedEvent> {
        let mut events = delta
            .closed
            .into_iter()
            .flat_map(|delta| self.build_close_container(delta))
            .collect::<Vec<_>>();
        events.extend([
            TargetedEvent {
                target: EventTarget::Player(delta.player_id),
                event: CoreEvent::ContainerOpened {
                    window_id: delta.window_id,
                    container: delta.container,
                    title: delta.title,
                },
            },
            TargetedEvent {
                target: EventTarget::Player(delta.player_id),
                event: CoreEvent::InventoryContents {
                    window_id: delta.window_id,
                    container: delta.container,
                    contents: delta.contents,
                },
            },
        ]);
        events.extend(property_events(
            delta.window_id,
            delta.player_id,
            &delta.properties,
        ));
        events
    }

    fn build_close_container(&self, delta: CloseContainerDelta) -> Vec<TargetedEvent> {
        let mut events = vec![TargetedEvent {
            target: EventTarget::Player(delta.player_id),
            event: CoreEvent::ContainerClosed {
                window_id: delta.window_id,
            },
        }];
        if let Some(contents) = delta.contents {
            events.push(TargetedEvent {
                target: EventTarget::Player(delta.player_id),
                event: CoreEvent::InventoryContents {
                    window_id: 0,
                    container: InventoryContainer::Player,
                    contents,
                },
            });
        }
        events
    }

    fn build_clear_mining(&self, core: &ServerCore, delta: ClearMiningDelta) -> Vec<TargetedEvent> {
        self.build_mining_progress(core, delta.progress)
    }

    fn build_begin_mining(&self, core: &ServerCore, delta: BeginMiningDelta) -> Vec<TargetedEvent> {
        let mut events = delta
            .cleared
            .into_iter()
            .flat_map(|delta| self.build_clear_mining(core, delta))
            .collect::<Vec<_>>();
        events.extend(self.build_mining_progress(core, delta.progress));
        events
    }

    fn build_mining_progress(
        &self,
        core: &ServerCore,
        delta: MiningProgressDelta,
    ) -> Vec<TargetedEvent> {
        block_break_progress_events(
            core,
            delta.breaker_entity_id,
            delta.position,
            delta.stage,
            delta.duration_ms,
        )
    }

    fn build_complete_mining(
        &self,
        core: &ServerCore,
        delta: CompleteMiningDelta,
    ) -> Vec<TargetedEvent> {
        match delta {
            CompleteMiningDelta::Cleared(delta) => self.build_clear_mining(core, delta),
            CompleteMiningDelta::Completed {
                block,
                spawned_item,
            } => {
                let mut events = self.build_set_block(core, block);
                if let Some(delta) = spawned_item {
                    events.extend(self.build_dropped_item_spawn(core, delta));
                }
                events
            }
        }
    }

    fn build_window_diff(&self, delta: WindowDiffDelta) -> Vec<TargetedEvent> {
        let mut events = inventory_diff_events(
            delta.window_id,
            delta.container,
            delta.player_id,
            &delta.before_contents,
            &delta.after_contents,
        );
        events.extend(property_diff_events(
            delta.window_id,
            delta.player_id,
            &delta.before_properties,
            &delta.after_properties,
        ));
        events
    }

    fn build_dropped_item_tick(
        &self,
        core: &ServerCore,
        delta: DroppedItemTickDelta,
    ) -> Vec<TargetedEvent> {
        let mut events = delta
            .inventory_delta
            .into_iter()
            .flat_map(|delta| self.build_window_diff(delta))
            .collect::<Vec<_>>();
        if let Some(delta) = delta.despawn {
            events.extend(self.build_entity_despawn(core, delta));
        }
        events
    }

    fn build_set_block(&self, core: &ServerCore, delta: BlockDelta) -> Vec<TargetedEvent> {
        let mut events = delta
            .cleared_mining
            .into_iter()
            .flat_map(|delta| self.build_clear_mining(core, delta))
            .collect::<Vec<_>>();
        events.extend(
            delta
                .closed_containers
                .into_iter()
                .flat_map(|delta| self.build_close_container(delta)),
        );
        events.extend(block_change_events(core, delta.position));
        events
    }

    fn build_dropped_item_spawn(
        &self,
        core: &ServerCore,
        delta: DroppedItemSpawnDelta,
    ) -> Vec<TargetedEvent> {
        dropped_item_spawn_events(core, delta.entity_id, &delta.item)
    }

    fn build_entity_despawn(
        &self,
        core: &ServerCore,
        delta: EntityDespawnDelta,
    ) -> Vec<TargetedEvent> {
        core.sessions
            .player_sessions
            .keys()
            .copied()
            .map(|player_id| TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::EntityDespawned {
                    entity_ids: delta.entity_ids.clone(),
                },
            })
            .collect()
    }

    fn build_keep_alive(&self, delta: KeepAliveDelta) -> Vec<TargetedEvent> {
        vec![TargetedEvent {
            target: EventTarget::Player(delta.player_id),
            event: CoreEvent::KeepAliveRequested {
                keep_alive_id: delta.keep_alive_id,
            },
        }]
    }

    fn build_disconnect(&self, core: &ServerCore, delta: DisconnectDelta) -> Vec<TargetedEvent> {
        let mut events = delta
            .cleared_mining
            .into_iter()
            .flat_map(|delta| self.build_clear_mining(core, delta))
            .collect::<Vec<_>>();
        events.push(TargetedEvent {
            target: EventTarget::EveryoneExcept(delta.player_id),
            event: CoreEvent::EntityDespawned {
                entity_ids: vec![delta.entity_id],
            },
        });
        events
    }

    fn filter_hidden_events(
        &self,
        core: &ServerCore,
        events: Vec<TargetedEvent>,
    ) -> Vec<TargetedEvent> {
        let mut hidden_entities = self
            .hidden_player_entities
            .values()
            .copied()
            .collect::<BTreeSet<_>>();
        hidden_entities.extend(
            self.hidden_players
                .iter()
                .filter_map(|player_id| core.player_entity_id(*player_id)),
        );

        events
            .into_iter()
            .filter_map(|targeted| self.sanitize_hidden_event(&hidden_entities, targeted))
            .collect()
    }

    fn sanitize_hidden_event(
        &self,
        hidden_entities: &BTreeSet<EntityId>,
        targeted: TargetedEvent,
    ) -> Option<TargetedEvent> {
        match targeted.target {
            EventTarget::Player(player_id) if self.hidden_players.contains(&player_id) => {
                return None;
            }
            EventTarget::EveryoneExcept(player_id) if self.hidden_players.contains(&player_id) => {
                return None;
            }
            _ => {}
        }

        match targeted.event {
            CoreEvent::EntityMoved { entity_id, .. } if hidden_entities.contains(&entity_id) => {
                None
            }
            CoreEvent::EntitySpawned { entity_id, .. } if hidden_entities.contains(&entity_id) => {
                None
            }
            CoreEvent::BlockBreakingProgress {
                breaker_entity_id, ..
            } if hidden_entities.contains(&breaker_entity_id) => None,
            CoreEvent::EntityDespawned { entity_ids } => {
                let entity_ids = entity_ids
                    .into_iter()
                    .filter(|entity_id| !hidden_entities.contains(entity_id))
                    .collect::<Vec<_>>();
                (!entity_ids.is_empty()).then_some(TargetedEvent {
                    target: targeted.target,
                    event: CoreEvent::EntityDespawned { entity_ids },
                })
            }
            event => Some(TargetedEvent {
                target: targeted.target,
                event,
            }),
        }
    }
}

fn login_initial_events(
    core: &ServerCore,
    connection_id: ConnectionId,
    player_id: PlayerId,
    entity_id: EntityId,
    player: &PlayerSnapshot,
    visible_chunks: Vec<ChunkColumn>,
) -> Vec<TargetedEvent> {
    vec![
        TargetedEvent {
            target: EventTarget::Connection(connection_id),
            event: CoreEvent::LoginAccepted {
                player_id,
                entity_id,
                player: player.clone(),
            },
        },
        TargetedEvent {
            target: EventTarget::Connection(connection_id),
            event: CoreEvent::PlayBootstrap {
                player: player.clone(),
                entity_id,
                world_meta: core.world.world_meta.clone(),
                view_distance: core.world.config.view_distance,
            },
        },
        TargetedEvent {
            target: EventTarget::Connection(connection_id),
            event: CoreEvent::ChunkBatch {
                chunks: visible_chunks,
            },
        },
        TargetedEvent {
            target: EventTarget::Connection(connection_id),
            event: CoreEvent::InventoryContents {
                window_id: 0,
                container: InventoryContainer::Player,
                contents: InventoryWindowContents::player(player.inventory.clone()),
            },
        },
        TargetedEvent {
            target: EventTarget::Connection(connection_id),
            event: CoreEvent::SelectedHotbarSlotChanged {
                slot: player.selected_hotbar_slot,
            },
        },
    ]
}

fn existing_player_spawn_events(
    connection_id: ConnectionId,
    existing_players: Vec<(EntityId, PlayerSnapshot)>,
) -> Vec<TargetedEvent> {
    existing_players
        .into_iter()
        .map(|(entity_id, player)| TargetedEvent {
            target: EventTarget::Connection(connection_id),
            event: CoreEvent::EntitySpawned { entity_id, player },
        })
        .collect()
}

fn block_change_events(core: &ServerCore, position: BlockPos) -> Vec<TargetedEvent> {
    let block = core.block_at(position);
    core.sessions
        .player_sessions
        .iter()
        .filter(|(_, session)| session.view.loaded_chunks.contains(&position.chunk_pos()))
        .map(|(player_id, _)| TargetedEvent {
            target: EventTarget::Player(*player_id),
            event: CoreEvent::BlockChanged {
                position,
                block: block.clone(),
            },
        })
        .collect()
}

fn dropped_item_spawn_events(
    core: &ServerCore,
    entity_id: EntityId,
    item: &DroppedItemSnapshot,
) -> Vec<TargetedEvent> {
    core.sessions
        .player_sessions
        .keys()
        .copied()
        .map(|player_id| TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::DroppedItemSpawned {
                entity_id,
                item: item.clone(),
            },
        })
        .collect()
}

fn block_break_progress_events(
    core: &ServerCore,
    breaker_entity_id: EntityId,
    position: BlockPos,
    stage: Option<u8>,
    duration_ms: u64,
) -> Vec<TargetedEvent> {
    core.sessions
        .player_sessions
        .iter()
        .filter(|(_, session)| session.view.loaded_chunks.contains(&position.chunk_pos()))
        .map(|(player_id, _)| TargetedEvent {
            target: EventTarget::Player(*player_id),
            event: CoreEvent::BlockBreakingProgress {
                breaker_entity_id,
                position,
                stage,
                duration_ms,
            },
        })
        .collect()
}
