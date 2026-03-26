use super::{
    ServerCore,
    canonical::{ApplyCoreOpsOptions, CoreOp, DisconnectDelta, KeepAliveDelta, apply_core_ops},
    inventory::{persisted_online_player_snapshot_state, world_block_entity, world_chest_position},
    mining::collect_active_mining_ops,
    mining::state_clear_active_mining,
    state_backend::{BaseStateRef, CoreStateMut, CoreStateRead},
};
use crate::PlayerId;
use crate::events::TargetedEvent;
use crate::world::BlockEntityState;

impl ServerCore {
    pub fn tick(&mut self, now_ms: u64) -> Vec<TargetedEvent> {
        let state = BaseStateRef::new(self);
        let ops = self
            .scheduler
            .tick(&state, now_ms, self.sessions.keepalive_timeout_ms);
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
    now_ms: u64,
) -> Option<KeepAliveDelta> {
    state.player_session(player_id)?;
    let keep_alive_id = state.allocate_keep_alive_id();
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
    fn tick(
        self,
        core: &impl CoreStateRead,
        now_ms: u64,
        keepalive_timeout_ms: u64,
    ) -> Vec<CoreOp> {
        let mut ops = core
            .player_ids()
            .into_iter()
            .filter_map(|player_id| {
                core.player_session(player_id)
                    .is_some_and(|session| {
                        session.active_container.as_ref().is_some_and(|window| {
                            window.container == crate::inventory::InventoryContainer::Furnace
                        })
                    })
                    .then_some(CoreOp::TickFurnaceWindow { player_id })
            })
            .collect::<Vec<_>>();
        ops.extend(
            core.dropped_item_ids()
                .into_iter()
                .map(|entity_id| CoreOp::TickDroppedItem { entity_id }),
        );
        ops.extend(collect_active_mining_ops(core, now_ms));
        ops.extend(collect_keepalive_ops(core, now_ms, keepalive_timeout_ms));
        ops
    }
}

fn collect_keepalive_ops(
    state: &impl CoreStateRead,
    now_ms: u64,
    keepalive_timeout_ms: u64,
) -> Vec<CoreOp> {
    state
        .player_ids()
        .into_iter()
        .filter_map(|player_id| {
            let session = state.player_session(player_id)?;
            let disconnect_due_to_timeout = session
                .last_keep_alive_sent_at
                .is_some_and(|sent_at| now_ms.saturating_sub(sent_at) > keepalive_timeout_ms);
            if disconnect_due_to_timeout {
                return Some(CoreOp::DisconnectPlayer { player_id });
            }
            (!session.pending_keep_alive_id.is_some() && now_ms >= session.next_keep_alive_at)
                .then_some(CoreOp::RequestKeepAlive { player_id })
        })
        .collect()
}
