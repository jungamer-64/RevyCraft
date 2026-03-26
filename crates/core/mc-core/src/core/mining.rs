use super::ActiveMiningState;
use super::canonical::{
    BeginMiningDelta, ClearMiningDelta, CompleteMiningDelta, MiningProgressDelta,
};
use super::state_backend::{CoreStateMut, CoreStateRead};
use crate::catalog;
use crate::world::{BlockPos, BlockState, Vec3};
use crate::{EntityId, PlayerId};

pub(super) fn state_begin_mining(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    position: BlockPos,
    duration_ms: u64,
    now_ms: u64,
) -> Option<BeginMiningDelta> {
    let Some(entity_id) = state.player_entity_id(player_id) else {
        return None;
    };
    if state
        .player_active_mining_by_entity(entity_id)
        .is_some_and(|state| state.position == position)
    {
        return None;
    }

    let cleared = state_clear_active_mining(state, player_id);
    let Some(inventory) = state.player_inventory(player_id) else {
        return cleared.map(|cleared| BeginMiningDelta {
            progress: cleared.progress.clone(),
            cleared: Some(cleared),
        });
    };
    let Some(selected_hotbar_slot) = state.player_selected_hotbar(player_id) else {
        return cleared.map(|cleared| BeginMiningDelta {
            progress: cleared.progress.clone(),
            cleared: Some(cleared),
        });
    };
    let tool_context =
        catalog::tool_spec_for_item(inventory.selected_hotbar_stack(selected_hotbar_slot));
    state.set_player_active_mining(
        entity_id,
        Some(ActiveMiningState {
            position,
            started_at_ms: now_ms,
            duration_ms,
            last_stage: Some(0),
            tool_context,
        }),
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
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
) -> Option<ClearMiningDelta> {
    let Some(entity_id) = state.player_entity_id(player_id) else {
        return None;
    };
    let Some(mining_state) = state.remove_player_active_mining(entity_id) else {
        return None;
    };
    Some(ClearMiningDelta {
        progress: MiningProgressDelta {
            breaker_entity_id: entity_id,
            position: mining_state.position,
            stage: None,
            duration_ms: mining_state.duration_ms,
        },
    })
}

pub(super) fn state_clear_active_mining_at(
    state: &mut impl CoreStateMut,
    position: BlockPos,
) -> Vec<ClearMiningDelta> {
    let player_ids = state
        .player_ids()
        .into_iter()
        .filter(|player_id| {
            state
                .player_active_mining(*player_id)
                .is_some_and(|mining_state| mining_state.position == position)
        })
        .collect::<Vec<_>>();
    player_ids
        .into_iter()
        .filter_map(|player_id| state_clear_active_mining(state, player_id))
        .collect()
}

pub(super) fn state_advance_mining_stage(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    entity_id: EntityId,
    position: BlockPos,
    stage: u8,
    duration_ms: u64,
) -> Option<MiningProgressDelta> {
    let current_entity_id = state.player_entity_id(player_id);
    let Some(mining_state) = state.player_active_mining_mut(entity_id) else {
        return None;
    };
    if current_entity_id != Some(entity_id)
        || mining_state.position != position
        || Some(stage) == mining_state.last_stage
    {
        return None;
    }
    mining_state.last_stage = Some(stage);
    Some(MiningProgressDelta {
        breaker_entity_id: entity_id,
        position,
        stage: Some(stage),
        duration_ms,
    })
}

pub(super) fn collect_active_mining_ops(
    state: &impl CoreStateRead,
    now_ms: u64,
) -> Vec<super::canonical::CoreOp> {
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

    let actions = state
        .player_entity_ids()
        .into_iter()
        .filter_map(|entity_id| {
            let mining_state = state.player_active_mining_by_entity(entity_id)?;
            let player_id = state.player_identity_by_entity(entity_id)?.player_id;
            let elapsed_ms = now_ms.saturating_sub(mining_state.started_at_ms);
            if elapsed_ms >= mining_state.duration_ms {
                return Some(Action::Complete {
                    player_id,
                    position: mining_state.position,
                });
            }
            let next_stage = ((elapsed_ms.saturating_mul(10)) / mining_state.duration_ms) as u8;
            if Some(next_stage) != mining_state.last_stage {
                return Some(Action::Stage {
                    player_id,
                    entity_id,
                    position: mining_state.position,
                    stage: next_stage.min(9),
                    duration_ms: mining_state.duration_ms,
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
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    position: BlockPos,
    now_ms: u64,
) -> Option<CompleteMiningDelta> {
    let Some(player) = state.compose_player_snapshot(player_id) else {
        return None;
    };
    let Some(mining_state) = state.player_active_mining(player_id) else {
        return None;
    };
    if mining_state.position != position {
        return None;
    }

    let current = state.block_state(position);
    if !state.can_edit_block_for_snapshot(&player, position)
        || current.is_air()
        || current.key.as_str() == catalog::BEDROCK
        || (matches!(current.key.as_str(), catalog::CHEST | catalog::FURNACE)
            && state
                .block_entity(position)
                .is_some_and(|entity| entity.has_inventory_contents()))
    {
        return state_clear_active_mining(state, player_id).map(CompleteMiningDelta::Cleared);
    }

    let block = super::mutation::state_set_block(state, position, BlockState::air());
    let spawned_item = catalog::survival_drop_for_block(&current).and_then(|item| {
        super::mutation::state_spawn_dropped_item(
            state,
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
