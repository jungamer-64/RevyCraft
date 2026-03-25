use super::{
    ServerCore,
    canonical::{ApplyCoreOpsOptions, CoreOp, DisconnectDelta, KeepAliveDelta, apply_core_ops},
    inventory::{world_block_entity, world_chest_position},
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

    pub(super) fn state_schedule_keep_alive(
        &mut self,
        player_id: PlayerId,
        keep_alive_id: i32,
        now_ms: u64,
    ) -> Option<KeepAliveDelta> {
        let keepalive_interval_ms = self.sessions.keepalive_interval_ms;
        let session = self.sessions.player_sessions.get_mut(&player_id)?;
        session.pending_keep_alive_id = Some(keep_alive_id);
        session.last_keep_alive_sent_at = Some(now_ms);
        session.next_keep_alive_at = now_ms.saturating_add(keepalive_interval_ms);
        Some(KeepAliveDelta {
            player_id,
            keep_alive_id,
        })
    }

    pub(super) fn state_disconnect_player(
        &mut self,
        player_id: PlayerId,
    ) -> Option<DisconnectDelta> {
        let cleared_mining = self.state_clear_active_mining(player_id);
        let Some(persisted_snapshot) = self.persisted_online_player_snapshot(player_id) else {
            return None;
        };
        let Some(session) = self.remove_online_player(player_id) else {
            return None;
        };
        if let Some(window) = session.active_container.as_ref() {
            if let Some((position, block_entity)) = world_block_entity(window) {
                let expected_block_key = match &block_entity {
                    BlockEntityState::Chest { .. } => crate::catalog::CHEST,
                    BlockEntityState::Furnace { .. } => crate::catalog::FURNACE,
                };
                if self.block_at(position).key.as_str() == expected_block_key {
                    self.world.block_entities.insert(position, block_entity);
                }
            }
            if let Some(position) = world_chest_position(window) {
                self.unregister_world_chest_viewer(position, player_id);
            }
        }
        self.world
            .saved_players
            .insert(player_id, persisted_snapshot);
        Some(DisconnectDelta {
            player_id,
            entity_id: session.entity_id,
            cleared_mining,
        })
    }
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
