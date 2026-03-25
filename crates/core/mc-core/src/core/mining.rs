use super::canonical::{
    BeginMiningDelta, ClearMiningDelta, CompleteMiningDelta, MiningProgressDelta,
};
use super::{ActiveMiningState, ServerCore};
use crate::catalog;
use crate::world::{BlockPos, BlockState, Vec3};
use crate::{EntityId, PlayerId};

impl ServerCore {
    pub(super) fn state_begin_mining(
        &mut self,
        player_id: PlayerId,
        position: BlockPos,
        duration_ms: u64,
        now_ms: u64,
    ) -> Option<BeginMiningDelta> {
        let Some(entity_id) = self.player_entity_id(player_id) else {
            return None;
        };
        if self
            .entities
            .player_active_mining
            .get(&entity_id)
            .is_some_and(|state| state.position == position)
        {
            return None;
        }

        let cleared = self.state_clear_active_mining(player_id);
        let Some(inventory) = self.player_inventory(player_id) else {
            return cleared.map(|cleared| BeginMiningDelta {
                progress: cleared.progress.clone(),
                cleared: Some(cleared),
            });
        };
        let Some(selected_hotbar_slot) = self.player_selected_hotbar(player_id) else {
            return cleared.map(|cleared| BeginMiningDelta {
                progress: cleared.progress.clone(),
                cleared: Some(cleared),
            });
        };
        let tool_context =
            catalog::tool_spec_for_item(inventory.selected_hotbar_stack(selected_hotbar_slot));
        self.entities.player_active_mining.insert(
            entity_id,
            ActiveMiningState {
                position,
                started_at_ms: now_ms,
                duration_ms,
                last_stage: Some(0),
                tool_context,
            },
        );
        Some(BeginMiningDelta {
            cleared,
            progress: MiningProgressDelta {
                breaker_entity_id: entity_id,
                position,
                stage: Some(0),
                duration_ms,
            },
        })
    }

    pub(super) fn state_clear_active_mining(
        &mut self,
        player_id: PlayerId,
    ) -> Option<ClearMiningDelta> {
        let Some(entity_id) = self.player_entity_id(player_id) else {
            return None;
        };
        let Some(state) = self.entities.player_active_mining.remove(&entity_id) else {
            return None;
        };
        Some(ClearMiningDelta {
            progress: MiningProgressDelta {
                breaker_entity_id: entity_id,
                position: state.position,
                stage: None,
                duration_ms: state.duration_ms,
            },
        })
    }

    pub(super) fn state_clear_active_mining_at(
        &mut self,
        position: BlockPos,
    ) -> Vec<ClearMiningDelta> {
        let player_ids = self
            .entities
            .players_by_player_id
            .iter()
            .filter_map(|(player_id, entity_id)| {
                self.entities
                    .player_active_mining
                    .get(entity_id)
                    .is_some_and(|state| state.position == position)
                    .then_some(*player_id)
            })
            .collect::<Vec<_>>();
        player_ids
            .into_iter()
            .filter_map(|player_id| self.state_clear_active_mining(player_id))
            .collect()
    }

    pub(super) fn state_advance_mining_stage(
        &mut self,
        player_id: PlayerId,
        entity_id: EntityId,
        position: BlockPos,
        stage: u8,
        duration_ms: u64,
    ) -> Option<MiningProgressDelta> {
        let current_entity_id = self.player_entity_id(player_id);
        let Some(state) = self.entities.player_active_mining.get_mut(&entity_id) else {
            return None;
        };
        if current_entity_id != Some(entity_id)
            || state.position != position
            || Some(stage) == state.last_stage
        {
            return None;
        }
        state.last_stage = Some(stage);
        Some(MiningProgressDelta {
            breaker_entity_id: entity_id,
            position,
            stage: Some(stage),
            duration_ms,
        })
    }

    pub(super) fn collect_active_mining_ops(&self, now_ms: u64) -> Vec<super::canonical::CoreOp> {
        enum Action {
            Stage {
                player_id: PlayerId,
                entity_id: EntityId,
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
            .entities
            .player_active_mining
            .iter()
            .filter_map(|(entity_id, state)| {
                let player_id = self.entities.player_identity.get(entity_id)?.player_id;
                let elapsed_ms = now_ms.saturating_sub(state.started_at_ms);
                if elapsed_ms >= state.duration_ms {
                    return Some(Action::Complete {
                        player_id,
                        position: state.position,
                    });
                }
                let next_stage = ((elapsed_ms.saturating_mul(10)) / state.duration_ms) as u8;
                if Some(next_stage) != state.last_stage {
                    return Some(Action::Stage {
                        player_id,
                        entity_id: *entity_id,
                        position: state.position,
                        stage: next_stage.min(9),
                        duration_ms: state.duration_ms,
                    });
                }
                None
            })
            .collect::<Vec<_>>();

        actions
            .into_iter()
            .map(|action| match action {
                Action::Stage {
                    player_id,
                    entity_id,
                    position,
                    stage,
                    duration_ms,
                } => super::canonical::CoreOp::AdvanceMiningStage {
                    player_id,
                    entity_id,
                    position,
                    stage,
                    duration_ms,
                },
                Action::Complete {
                    player_id,
                    position,
                } => super::canonical::CoreOp::CompleteMining {
                    player_id,
                    position,
                },
            })
            .collect()
    }

    pub(super) fn state_complete_survival_mining(
        &mut self,
        player_id: PlayerId,
        position: BlockPos,
        now_ms: u64,
    ) -> Option<CompleteMiningDelta> {
        let Some(player) = self.compose_player_snapshot(player_id) else {
            return None;
        };
        let Some(state) = self.player_active_mining(player_id) else {
            return None;
        };
        if state.position != position {
            return None;
        }

        let current = self.block_at(position);
        if !self.can_edit_block_for_snapshot(&player, position)
            || current.is_air()
            || current.key.as_str() == catalog::BEDROCK
            || (matches!(current.key.as_str(), catalog::CHEST | catalog::FURNACE)
                && self
                    .block_entity_at(position)
                    .is_some_and(|entity| entity.has_inventory_contents()))
        {
            return self
                .state_clear_active_mining(player_id)
                .map(CompleteMiningDelta::Cleared);
        }

        let block = self.state_set_block(position, BlockState::air());
        let spawned_item = catalog::survival_drop_for_block(&current).and_then(|item| {
            self.state_spawn_dropped_item(
                None,
                Vec3::new(
                    f64::from(position.x) + 0.5,
                    f64::from(position.y) + 0.5,
                    f64::from(position.z) + 0.5,
                ),
                item,
                now_ms,
            )
        });
        Some(CompleteMiningDelta::Completed {
            block,
            spawned_item,
        })
    }
}
