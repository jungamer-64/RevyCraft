use super::ServerCore;
use super::canonical::{LoginFinalizeDelta, ViewUpdateDelta};
use crate::events::{CoreEvent, EventTarget, TargetedEvent};
use crate::inventory::PlayerInventory;
use crate::player::PlayerSnapshot;
use crate::world::{BlockPos, DimensionId, Vec3};
use crate::{ConnectionId, PlayerId};

impl ServerCore {
    pub(super) fn reject_connection(
        connection_id: ConnectionId,
        reason: &str,
    ) -> Vec<TargetedEvent> {
        vec![TargetedEvent {
            target: EventTarget::Connection(connection_id),
            event: CoreEvent::Disconnect {
                reason: reason.to_string(),
            },
        }]
    }

    pub(super) fn state_update_client_settings(
        &mut self,
        player_id: PlayerId,
        view_distance: u8,
    ) -> Option<ViewUpdateDelta> {
        let capped_view_distance = view_distance.min(self.world.config.view_distance).max(1);
        let Some(position) = self
            .compose_player_snapshot(player_id)
            .map(|player| player.position)
        else {
            return None;
        };
        let Some(session) = self.player_session_mut(player_id) else {
            return None;
        };
        let delta = session
            .view
            .retarget(position.chunk_pos(), capped_view_distance);
        Some(ViewUpdateDelta {
            player_id,
            chunks: delta
                .added
                .into_iter()
                .map(|chunk_pos| self.ensure_chunk(chunk_pos).clone())
                .collect(),
        })
    }

    pub(super) fn finalize_login_delta(
        &mut self,
        connection_id: ConnectionId,
        player_id: PlayerId,
    ) -> Option<LoginFinalizeDelta> {
        let player = self.compose_player_snapshot(player_id)?;
        let (entity_id, session_view_distance) = self
            .player_session(player_id)
            .map(|session| (session.entity_id, session.view.view_distance))?;
        let visible_chunks =
            self.initial_visible_chunks(player.position.chunk_pos(), session_view_distance);
        let existing_players = self
            .sessions
            .player_sessions
            .iter()
            .filter(|(other_id, _)| **other_id != player_id)
            .filter_map(|(other_id, other_session)| {
                self.compose_player_snapshot(*other_id)
                    .map(|snapshot| (other_session.entity_id, snapshot))
            })
            .collect::<Vec<_>>();
        let dropped_items = self
            .entities
            .dropped_items
            .iter()
            .map(|(entity_id, item)| (*entity_id, item.snapshot.clone()))
            .collect::<Vec<_>>();
        Some(LoginFinalizeDelta {
            connection_id,
            player_id,
            entity_id,
            player,
            visible_chunks,
            existing_players,
            dropped_items,
        })
    }
}

pub(super) fn default_player(
    player_id: PlayerId,
    username: String,
    spawn: BlockPos,
) -> PlayerSnapshot {
    PlayerSnapshot {
        id: player_id,
        username,
        position: Vec3::new(
            f64::from(spawn.x) + 0.5,
            f64::from(spawn.y),
            f64::from(spawn.z) + 0.5,
        ),
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        dimension: DimensionId::Overworld,
        health: 20.0,
        food: 20,
        food_saturation: 5.0,
        inventory: PlayerInventory::creative_starter(),
        selected_hotbar_slot: 0,
    }
}
