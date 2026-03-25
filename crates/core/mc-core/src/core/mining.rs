use super::{ActiveMiningState, ServerCore};
use crate::catalog;
use crate::events::{CoreEvent, EventTarget, TargetedEvent};
use crate::world::{BlockPos, BlockState, Vec3};
use crate::{EntityId, PlayerId};

impl ServerCore {
    pub(super) fn apply_begin_mining_mutation(
        &mut self,
        player_id: PlayerId,
        position: BlockPos,
        duration_ms: u64,
        now_ms: u64,
    ) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get(&player_id) else {
            return Vec::new();
        };
        if player
            .active_mining
            .as_ref()
            .is_some_and(|state| state.position == position)
        {
            return Vec::new();
        }

        let mut events = self.clear_active_mining(player_id);
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return events;
        };
        let entity_id = player.entity_id;
        let tool_context = catalog::tool_spec_for_item(
            player
                .snapshot
                .inventory
                .selected_hotbar_stack(player.snapshot.selected_hotbar_slot),
        );
        player.active_mining = Some(ActiveMiningState {
            position,
            started_at_ms: now_ms,
            duration_ms,
            last_stage: Some(0),
            tool_context,
        });
        let _ = player;
        events.extend(self.block_break_progress_events(entity_id, position, Some(0), duration_ms));
        events
    }

    pub(super) fn clear_active_mining(&mut self, player_id: PlayerId) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return Vec::new();
        };
        let entity_id = player.entity_id;
        let Some(state) = player.active_mining.take() else {
            return Vec::new();
        };
        let position = state.position;
        let duration_ms = state.duration_ms;
        let _ = player;
        self.block_break_progress_events(entity_id, position, None, duration_ms)
    }

    pub(super) fn clear_active_mining_at(&mut self, position: BlockPos) -> Vec<TargetedEvent> {
        let player_ids = self
            .online_players
            .iter()
            .filter_map(|(player_id, player)| {
                player
                    .active_mining
                    .as_ref()
                    .is_some_and(|state| state.position == position)
                    .then_some(*player_id)
            })
            .collect::<Vec<_>>();
        let mut events = Vec::new();
        for player_id in player_ids {
            events.extend(self.clear_active_mining(player_id));
        }
        events
    }

    pub(super) fn tick_active_mining(&mut self, now_ms: u64) -> Vec<TargetedEvent> {
        enum Action {
            Stage {
                player_id: PlayerId,
                position: BlockPos,
                stage: u8,
                duration_ms: u64,
            },
            Complete {
                player_id: PlayerId,
                position: BlockPos,
            },
        }

        let actions = self
            .online_players
            .iter()
            .filter_map(|(player_id, player)| {
                let state = player.active_mining.as_ref()?;
                let elapsed_ms = now_ms.saturating_sub(state.started_at_ms);
                if elapsed_ms >= state.duration_ms {
                    return Some(Action::Complete {
                        player_id: *player_id,
                        position: state.position,
                    });
                }
                let next_stage = ((elapsed_ms.saturating_mul(10)) / state.duration_ms) as u8;
                if Some(next_stage) != state.last_stage {
                    return Some(Action::Stage {
                        player_id: *player_id,
                        position: state.position,
                        stage: next_stage.min(9),
                        duration_ms: state.duration_ms,
                    });
                }
                None
            })
            .collect::<Vec<_>>();

        let mut events = Vec::new();
        for action in actions {
            match action {
                Action::Stage {
                    player_id,
                    position,
                    stage,
                    duration_ms,
                } => {
                    let Some(player) = self.online_players.get_mut(&player_id) else {
                        continue;
                    };
                    let entity_id = player.entity_id;
                    let Some(state) = player.active_mining.as_mut() else {
                        continue;
                    };
                    if state.position != position || Some(stage) == state.last_stage {
                        continue;
                    }
                    state.last_stage = Some(stage);
                    let _ = state;
                    let _ = player;
                    events.extend(self.block_break_progress_events(
                        entity_id,
                        position,
                        Some(stage),
                        duration_ms,
                    ));
                }
                Action::Complete {
                    player_id,
                    position,
                } => {
                    events.extend(self.complete_survival_mining(player_id, position, now_ms));
                }
            }
        }
        events
    }

    fn complete_survival_mining(
        &mut self,
        player_id: PlayerId,
        position: BlockPos,
        now_ms: u64,
    ) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get(&player_id) else {
            return Vec::new();
        };
        let Some(state) = player.active_mining.as_ref() else {
            return Vec::new();
        };
        if state.position != position {
            return Vec::new();
        }

        let current = self.block_at(position);
        if !self.can_edit_block_for_snapshot(&player.snapshot, position)
            || current.is_air()
            || current.key.as_str() == catalog::BEDROCK
            || (matches!(current.key.as_str(), catalog::CHEST | catalog::FURNACE)
                && self
                    .block_entity_at(position)
                    .is_some_and(|entity| entity.has_inventory_contents()))
        {
            return self.clear_active_mining(player_id);
        }

        let mut events = self.apply_block_mutation(position, BlockState::air());
        if let Some(item) = catalog::survival_drop_for_block(&current) {
            events.extend(self.apply_dropped_item_mutation(
                Vec3::new(
                    f64::from(position.x) + 0.5,
                    f64::from(position.y) + 0.5,
                    f64::from(position.z) + 0.5,
                ),
                item,
                now_ms,
            ));
        }
        events
    }

    fn block_break_progress_events(
        &self,
        breaker_entity_id: EntityId,
        position: BlockPos,
        stage: Option<u8>,
        duration_ms: u64,
    ) -> Vec<TargetedEvent> {
        self.online_players
            .iter()
            .filter(|(_, player)| player.view.loaded_chunks.contains(&position.chunk_pos()))
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
}
