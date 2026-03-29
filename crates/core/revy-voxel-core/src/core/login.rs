use super::ServerCore;
use super::canonical::{LoginFinalizeDelta, ViewUpdateDelta};
use super::state_backend::{CoreStateMut, initial_visible_chunks};
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
}

pub(super) fn state_update_client_settings(
    state: &mut impl CoreStateMut,
    player_id: PlayerId,
    view_distance: u8,
) -> Option<ViewUpdateDelta> {
    let capped_view_distance = view_distance.min(state.config().view_distance).max(1);
    let Some(position) = state
        .compose_player_snapshot(player_id)
        .map(|player| player.position)
    else {
        return None;
    };
    let Some(session) = state.player_session_mut(player_id) else {
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
            .map(|chunk_pos| state.ensure_chunk_mut(chunk_pos).clone())
            .collect(),
    })
}

pub(super) fn finalize_login_delta(
    state: &mut impl CoreStateMut,
    connection_id: ConnectionId,
    player_id: PlayerId,
) -> Option<LoginFinalizeDelta> {
    let player = state.compose_player_snapshot(player_id)?;
    let (entity_id, session_view_distance) = state
        .player_session(player_id)
        .map(|session| (session.entity_id, session.view.view_distance))?;
    let visible_chunks =
        initial_visible_chunks(state, player.position.chunk_pos(), session_view_distance);
    let existing_players = state
        .player_ids()
        .into_iter()
        .filter(|other_id| *other_id != player_id)
        .filter_map(|other_id| {
            let session = state.player_session(other_id)?;
            let snapshot = state.compose_player_snapshot(other_id)?;
            Some((session.entity_id, snapshot))
        })
        .collect::<Vec<_>>();
    let dropped_items = state
        .dropped_item_ids()
        .into_iter()
        .filter_map(|entity_id| {
            state
                .dropped_item_by_entity(entity_id)
                .map(|item| (entity_id, item.snapshot))
        })
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

pub(super) fn default_player(
    player_id: PlayerId,
    username: String,
    spawn: BlockPos,
    inventory: PlayerInventory,
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
        inventory,
        selected_hotbar_slot: 0,
    }
}
