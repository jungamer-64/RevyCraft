use super::{ClientView, OnlinePlayer, ServerCore};
use crate::events::{CoreEvent, EventTarget, TargetedEvent};
use crate::gameplay::{GameplayJoinEffect, GameplayPolicyResolver};
use crate::player::{InventoryContainer, PlayerInventory, PlayerSnapshot};
use crate::world::{BlockPos, ChunkColumn, DimensionId, Vec3};
use crate::{ConnectionId, EntityId, PlayerId, SessionCapabilitySet};

impl ServerCore {
    pub(super) fn login_player_with_policy<R: GameplayPolicyResolver>(
        &mut self,
        connection_id: ConnectionId,
        username: String,
        player_id: PlayerId,
        now_ms: u64,
        session: &SessionCapabilitySet,
        resolver: &R,
    ) -> Result<Vec<TargetedEvent>, String> {
        if username.is_empty() || username.len() > 16 {
            return Ok(Self::reject_connection(connection_id, "Invalid username"));
        }
        if self.online_players.len() >= usize::from(self.config.max_players) {
            return Ok(Self::reject_connection(connection_id, "Server is full"));
        }
        if self.online_players.contains_key(&player_id) {
            return Ok(Self::reject_connection(
                connection_id,
                "Player is already online",
            ));
        }

        let mut player = self
            .saved_players
            .get(&player_id)
            .cloned()
            .unwrap_or_else(|| default_player(player_id, username.clone(), self.config.spawn));
        player.username = username;
        let join_effect = resolver.handle_player_join(self, session, &player)?;
        let join_events = Self::apply_gameplay_join_effect(&mut player, join_effect);

        let entity_id = EntityId(self.next_entity_id);
        self.next_entity_id = self.next_entity_id.saturating_add(1);

        let existing_players = self
            .online_players
            .values()
            .map(|online| (online.entity_id, online.snapshot.clone()))
            .collect::<Vec<_>>();

        let visible_chunks =
            self.initial_visible_chunks(player.position.chunk_pos(), self.config.view_distance);
        let view = ClientView::new(player.position.chunk_pos(), self.config.view_distance);

        self.online_players.insert(
            player_id,
            OnlinePlayer {
                entity_id,
                snapshot: player.clone(),
                view,
                pending_keep_alive_id: None,
                last_keep_alive_sent_at: None,
                next_keep_alive_at: now_ms.saturating_add(self.keepalive_interval_ms),
            },
        );

        let mut events =
            self.login_initial_events(connection_id, player_id, entity_id, &player, visible_chunks);
        events.extend(Self::existing_player_spawn_events(
            connection_id,
            existing_players,
        ));
        events.extend(join_events);

        events.push(TargetedEvent {
            target: EventTarget::EveryoneExcept(player_id),
            event: CoreEvent::EntitySpawned { entity_id, player },
        });
        Ok(events)
    }

    pub(super) fn apply_gameplay_join_effect(
        player: &mut PlayerSnapshot,
        effect: GameplayJoinEffect,
    ) -> Vec<TargetedEvent> {
        if let Some(inventory) = effect.inventory {
            player.inventory = inventory;
        }
        if let Some(selected_hotbar_slot) = effect.selected_hotbar_slot {
            player.selected_hotbar_slot = selected_hotbar_slot;
        }
        effect.emitted_events
    }

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

    pub(super) fn login_initial_events(
        &self,
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
                    world_meta: self.world_meta.clone(),
                    view_distance: self.config.view_distance,
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
                    container: InventoryContainer::Player,
                    inventory: player.inventory.clone(),
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

    pub(super) fn existing_player_spawn_events(
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

    pub(super) fn update_client_settings(
        &mut self,
        player_id: PlayerId,
        view_distance: u8,
    ) -> Vec<TargetedEvent> {
        let Some(player) = self.online_players.get_mut(&player_id) else {
            return Vec::new();
        };
        let capped_view_distance = view_distance.min(self.config.view_distance).max(1);
        let delta = player
            .view
            .retarget(player.snapshot.position.chunk_pos(), capped_view_distance);
        delta
            .added
            .into_iter()
            .map(|chunk_pos| TargetedEvent {
                target: EventTarget::Player(player_id),
                event: CoreEvent::ChunkBatch {
                    chunks: vec![self.ensure_chunk(chunk_pos).clone()],
                },
            })
            .collect()
    }
}

fn default_player(player_id: PlayerId, username: String, spawn: BlockPos) -> PlayerSnapshot {
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
