use super::{
    ServerCore,
    canonical::{ApplyCoreOpsOptions, CoreOp, DisconnectDelta, KeepAliveDelta, apply_core_ops},
    inventory::{persisted_online_player_snapshot_state, world_block_entity, world_chest_position},
    mining::state_clear_active_mining,
    state_backend::CoreStateMut,
};
use crate::PlayerId;
use crate::events::TargetedEvent;
use crate::world::BlockEntityState;

impl ServerCore {
    pub fn tick(&mut self, now_ms: u64) -> Vec<TargetedEvent> {
        let mut ops = self.scheduler.tick(self, now_ms);
        let player_ids = self
            .sessions
            .player_sessions
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let keepalive_timeout_ms = self.sessions.keepalive_timeout_ms;
        for player_id in player_ids {
            let (disconnect_due_to_timeout, should_schedule_keepalive) = if let Some(session) =
                self.sessions.player_sessions.get(&player_id)
            {
                let disconnect_due_to_timeout = session
                    .last_keep_alive_sent_at
                    .is_some_and(|sent_at| now_ms.saturating_sub(sent_at) > keepalive_timeout_ms);
                (
                    disconnect_due_to_timeout,
                    !disconnect_due_to_timeout
                        && session.pending_keep_alive_id.is_none()
                        && now_ms >= session.next_keep_alive_at,
                )
            } else {
                continue;
            };
            if disconnect_due_to_timeout {
                ops.push(CoreOp::DisconnectPlayer { player_id });
                continue;
            }
            if should_schedule_keepalive {
                let keep_alive_id = self.sessions.next_keep_alive_id;
                self.sessions.next_keep_alive_id =
                    self.sessions.next_keep_alive_id.saturating_add(1);
                ops.push(CoreOp::KeepAliveRequested {
                    player_id,
                    keep_alive_id,
                });
            }
        }
        apply_core_ops(self, ops, now_ms, ApplyCoreOpsOptions::default())
    }

    pub(super) fn accept_keep_alive(&mut self, player_id: PlayerId, keep_alive_id: i32) {
        let Some(session) = self.sessions.player_sessions.get_mut(&player_id) else {
            return;
        };
        if session.pending_keep_alive_id == Some(keep_alive_id) {
            session.pending_keep_alive_id = None;
            session.last_keep_alive_sent_at = None;
        }
    }
}

pub(super) fn schedule_keep_alive_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    keep_alive_id: i32,
    now_ms: u64,
) -> Option<KeepAliveDelta> {
    let keepalive_interval_ms = state.keepalive_interval_ms();
    let session = state.player_session_mut(player_id)?;
    session.pending_keep_alive_id = Some(keep_alive_id);
    session.last_keep_alive_sent_at = Some(now_ms);
    session.next_keep_alive_at = now_ms.saturating_add(keepalive_interval_ms);
    Some(KeepAliveDelta {
        player_id,
        keep_alive_id,
    })
}

pub(super) fn disconnect_player_state(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
) -> Option<DisconnectDelta> {
    let cleared_mining = state_clear_active_mining(state, player_id);
    let persisted_snapshot = persisted_online_player_snapshot_state(state, player_id)?;
    let session = state.remove_online_player(player_id)?;
    if let Some(window) = session.active_container.as_ref() {
        if let Some((position, block_entity)) = world_block_entity(window) {
            let expected_block_key = match &block_entity {
                BlockEntityState::Chest { .. } => crate::catalog::CHEST,
                BlockEntityState::Furnace { .. } => crate::catalog::FURNACE,
            };
            if state.block_state(position).key.as_str() == expected_block_key {
                state.set_block_entity(position, Some(block_entity));
            }
        }
        if let Some(position) = world_chest_position(window) {
            let mut viewers = state.chest_viewers(position).unwrap_or_default();
            viewers.remove(&player_id);
            if viewers.is_empty() {
                state.set_chest_viewers(position, None);
            } else {
                state.set_chest_viewers(position, Some(viewers));
            }
        }
    }
    state.set_saved_player(player_id, Some(persisted_snapshot));
    Some(DisconnectDelta {
        player_id,
        entity_id: session.entity_id,
        cleared_mining,
    })
}

impl super::SystemScheduler {
    fn tick(self, core: &ServerCore, now_ms: u64) -> Vec<CoreOp> {
        let mut ops = core
            .sessions
            .player_sessions
            .iter()
            .filter_map(|(player_id, session)| {
                session
                    .active_container
                    .as_ref()
                    .filter(|window| {
                        window.container == crate::inventory::InventoryContainer::Furnace
                    })
                    .map(|_| CoreOp::TickFurnaceWindow {
                        player_id: *player_id,
                    })
            })
            .collect::<Vec<_>>();
        ops.extend(
            core.entities
                .dropped_items
                .keys()
                .copied()
                .map(|entity_id| CoreOp::TickDroppedItem { entity_id }),
        );
        ops.extend(core.collect_active_mining_ops(now_ms));
        ops
    }
}
