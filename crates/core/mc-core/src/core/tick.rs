use super::{
    ServerCore,
    inventory::{OpenInventoryWindowState, world_chest_position},
};
use crate::events::{CoreEvent, EventTarget, TargetedEvent};
use crate::gameplay::GameplayPolicyResolver;
use crate::world::BlockEntityState;
use crate::{PlayerId, SessionCapabilitySet};

impl ServerCore {
    pub fn tick(&mut self, now_ms: u64) -> Vec<TargetedEvent> {
        let mut events = self.tick_active_containers();
        let player_ids = self.online_players.keys().copied().collect::<Vec<_>>();
        for player_id in player_ids {
            let Some(player) = self.online_players.get_mut(&player_id) else {
                continue;
            };
            if let Some(sent_at) = player.last_keep_alive_sent_at
                && now_ms.saturating_sub(sent_at) > self.keepalive_timeout_ms
            {
                events.extend(self.disconnect_player(player_id));
                continue;
            }
            if player.pending_keep_alive_id.is_none() && now_ms >= player.next_keep_alive_at {
                let keep_alive_id = self.next_keep_alive_id;
                self.next_keep_alive_id = self.next_keep_alive_id.saturating_add(1);
                player.pending_keep_alive_id = Some(keep_alive_id);
                player.last_keep_alive_sent_at = Some(now_ms);
                player.next_keep_alive_at = now_ms.saturating_add(self.keepalive_interval_ms);
                events.push(TargetedEvent {
                    target: EventTarget::Player(player_id),
                    event: CoreEvent::KeepAliveRequested { keep_alive_id },
                });
            }
        }
        events
    }

    /// Applies a tick for a single player using the provided gameplay policy resolver.
    ///
    /// # Errors
    ///
    /// Returns an error when the gameplay policy resolver rejects the tick.
    pub fn tick_player_with_policy<R: GameplayPolicyResolver + ?Sized>(
        &mut self,
        player_id: PlayerId,
        now_ms: u64,
        session: &SessionCapabilitySet,
        resolver: &R,
    ) -> Result<Vec<TargetedEvent>, String> {
        let effect = resolver.handle_tick(self, session, player_id, now_ms)?;
        Ok(self.apply_gameplay_effect(effect))
    }

    pub(super) fn accept_keep_alive(&mut self, player_id: PlayerId, keep_alive_id: i32) {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return;
        };
        if player.pending_keep_alive_id == Some(keep_alive_id) {
            player.pending_keep_alive_id = None;
            player.last_keep_alive_sent_at = None;
        }
    }

    pub(super) fn disconnect_player(&mut self, player_id: PlayerId) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.remove(&player_id) else {
            return Vec::new();
        };
        if let Some(window) = player.active_container.as_ref() {
            if let Some(position) = world_chest_position(window) {
                if let OpenInventoryWindowState::Chest(chest) = &window.state {
                    self.block_entities.insert(
                        position,
                        BlockEntityState::Chest {
                            slots: chest.slots.clone(),
                        },
                    );
                }
                self.unregister_world_chest_viewer(position, player_id);
            }
        }
        self.saved_players
            .insert(player_id, Self::persisted_online_player_snapshot(&player));
        vec![TargetedEvent {
            target: EventTarget::EveryoneExcept(player_id),
            event: CoreEvent::EntityDespawned {
                entity_ids: vec![player.entity_id],
            },
        }]
    }
}
