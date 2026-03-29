use super::{
    ActiveMiningState, ClientView, CoreConfig, DroppedItemState, EntityKind, PlayerIdentity,
    PlayerSessionState, PlayerTransform, PlayerVitals, ServerCore, WorldContainerViewers,
};
use crate::inventory::PlayerInventory;
use crate::player::PlayerSnapshot;
use crate::world::{
    BlockEntityState, BlockPos, BlockState, ChunkColumn, ChunkPos, WorldMeta, required_chunks,
};
use crate::{BLOCK_EDIT_REACH, EntityId, PLAYER_HEIGHT, PLAYER_WIDTH, PlayerId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct TxOverlay {
    pub(super) chunks: BTreeMap<ChunkPos, ChunkColumn>,
    pub(super) block_entities: BTreeMap<BlockPos, Option<BlockEntityState>>,
    pub(super) container_viewers: BTreeMap<BlockPos, Option<WorldContainerViewers>>,
    pub(super) saved_players: BTreeMap<PlayerId, Option<PlayerSnapshot>>,
    pub(super) players_by_player_id: BTreeMap<PlayerId, Option<EntityId>>,
    pub(super) entity_kinds: BTreeMap<EntityId, Option<EntityKind>>,
    pub(super) player_identity: BTreeMap<EntityId, Option<PlayerIdentity>>,
    pub(super) player_transform: BTreeMap<EntityId, Option<PlayerTransform>>,
    pub(super) player_vitals: BTreeMap<EntityId, Option<PlayerVitals>>,
    pub(super) player_inventory: BTreeMap<EntityId, Option<PlayerInventory>>,
    pub(super) player_selected_hotbar: BTreeMap<EntityId, Option<u8>>,
    pub(super) player_active_mining: BTreeMap<EntityId, Option<ActiveMiningState>>,
    pub(super) dropped_items: BTreeMap<EntityId, Option<DroppedItemState>>,
    pub(super) player_sessions: BTreeMap<PlayerId, Option<PlayerSessionState>>,
    pub(super) next_entity_id: Option<i32>,
    pub(super) next_keep_alive_id: Option<i32>,
}

pub(super) trait CoreStateRead {
    fn content_behavior(&self) -> &dyn super::ContentBehavior;
    fn content_behavior_arc(&self) -> Arc<dyn super::ContentBehavior>;
    fn config(&self) -> &CoreConfig;
    fn world_meta_ref(&self) -> &WorldMeta;
    fn block_state(&self, position: BlockPos) -> Option<BlockState>;
    fn block_entity(&self, position: BlockPos) -> Option<BlockEntityState>;
    fn saved_player(&self, player_id: PlayerId) -> Option<PlayerSnapshot>;
    fn player_entity_id(&self, player_id: PlayerId) -> Option<EntityId>;
    fn player_session(&self, player_id: PlayerId) -> Option<PlayerSessionState>;
    fn player_identity_by_entity(&self, entity_id: EntityId) -> Option<PlayerIdentity>;
    fn player_transform_by_entity(&self, entity_id: EntityId) -> Option<PlayerTransform>;
    fn player_vitals_by_entity(&self, entity_id: EntityId) -> Option<PlayerVitals>;
    fn player_inventory_by_entity(&self, entity_id: EntityId) -> Option<PlayerInventory>;
    fn player_selected_hotbar_by_entity(&self, entity_id: EntityId) -> Option<u8>;
    fn player_active_mining_by_entity(&self, entity_id: EntityId) -> Option<ActiveMiningState>;
    fn dropped_item_by_entity(&self, entity_id: EntityId) -> Option<DroppedItemState>;
    fn player_ids(&self) -> Vec<PlayerId>;
    fn player_entity_ids(&self) -> BTreeSet<EntityId>;
    fn dropped_item_ids(&self) -> Vec<EntityId>;
    fn container_viewers(&self, position: BlockPos) -> Option<WorldContainerViewers>;
    fn keepalive_interval_ms(&self) -> u64;

    fn world_meta(&self) -> WorldMeta {
        self.world_meta_ref().clone()
    }

    fn compose_player_snapshot_by_entity(&self, entity_id: EntityId) -> Option<PlayerSnapshot> {
        let identity = self.player_identity_by_entity(entity_id)?;
        let transform = self.player_transform_by_entity(entity_id)?;
        let vitals = self.player_vitals_by_entity(entity_id)?;
        let inventory = self.player_inventory_by_entity(entity_id)?;
        let selected_hotbar_slot = self.player_selected_hotbar_by_entity(entity_id)?;
        Some(PlayerSnapshot {
            id: identity.player_id,
            username: identity.username,
            position: transform.position,
            yaw: transform.yaw,
            pitch: transform.pitch,
            on_ground: transform.on_ground,
            dimension: transform.dimension,
            health: vitals.health,
            food: vitals.food,
            food_saturation: vitals.food_saturation,
            inventory,
            selected_hotbar_slot,
        })
    }

    fn compose_player_snapshot(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        let entity_id = self.player_entity_id(player_id)?;
        self.compose_player_snapshot_by_entity(entity_id)
    }

    fn player_inventory(&self, player_id: PlayerId) -> Option<PlayerInventory> {
        let entity_id = self.player_entity_id(player_id)?;
        self.player_inventory_by_entity(entity_id)
    }

    fn player_selected_hotbar(&self, player_id: PlayerId) -> Option<u8> {
        let entity_id = self.player_entity_id(player_id)?;
        self.player_selected_hotbar_by_entity(entity_id)
    }

    fn player_transform(&self, player_id: PlayerId) -> Option<PlayerTransform> {
        let entity_id = self.player_entity_id(player_id)?;
        self.player_transform_by_entity(entity_id)
    }

    fn player_active_mining(&self, player_id: PlayerId) -> Option<ActiveMiningState> {
        let entity_id = self.player_entity_id(player_id)?;
        self.player_active_mining_by_entity(entity_id)
    }

    fn online_player_count(&self) -> usize {
        self.player_ids().len()
    }

    fn can_edit_block_for_snapshot(&self, actor: &PlayerSnapshot, position: BlockPos) -> bool {
        if !(0..=255).contains(&position.y) {
            return false;
        }
        if distance_squared_to_block_center(actor.position, position) > BLOCK_EDIT_REACH.powi(2) {
            return false;
        }
        !self.player_entity_ids().into_iter().any(|entity_id| {
            self.player_transform_by_entity(entity_id)
                .is_some_and(|transform| block_intersects_player(position, &transform))
        })
    }
}

pub(super) trait CoreStateMut: CoreStateRead {
    fn ensure_chunk_mut(&mut self, chunk_pos: ChunkPos) -> &mut ChunkColumn;
    fn set_block_state(&mut self, position: BlockPos, block: Option<BlockState>);
    fn set_block_entity(&mut self, position: BlockPos, block_entity: Option<BlockEntityState>);
    fn player_session_mut(&mut self, player_id: PlayerId) -> Option<&mut PlayerSessionState>;
    fn player_session_inventory_mut(
        &mut self,
        player_id: PlayerId,
    ) -> Option<(&mut PlayerSessionState, &mut PlayerInventory)>;
    fn player_transform_mut(&mut self, entity_id: EntityId) -> Option<&mut PlayerTransform>;
    fn player_inventory_mut(&mut self, entity_id: EntityId) -> Option<&mut PlayerInventory>;
    fn player_selected_hotbar_mut(&mut self, entity_id: EntityId) -> Option<&mut u8>;
    fn player_active_mining_mut(&mut self, entity_id: EntityId) -> Option<&mut ActiveMiningState>;
    fn remove_player_active_mining(&mut self, entity_id: EntityId) -> Option<ActiveMiningState>;
    fn set_player_active_mining(&mut self, entity_id: EntityId, state: Option<ActiveMiningState>);
    fn allocate_entity_id(&mut self) -> EntityId;
    fn allocate_keep_alive_id(&mut self) -> i32;
    fn set_entity_kind(&mut self, entity_id: EntityId, kind: Option<EntityKind>);
    fn set_dropped_item(&mut self, entity_id: EntityId, item: Option<DroppedItemState>);
    fn take_dropped_item(&mut self, entity_id: EntityId) -> Option<DroppedItemState>;
    fn set_saved_player(&mut self, player_id: PlayerId, snapshot: Option<PlayerSnapshot>);
    fn set_container_viewers(&mut self, position: BlockPos, viewers: Option<WorldContainerViewers>);
    fn spawn_online_player(
        &mut self,
        player: PlayerSnapshot,
        now_ms: u64,
        expected_entity_id: Option<EntityId>,
    ) -> EntityId;
    fn remove_online_player(&mut self, player_id: PlayerId) -> Option<PlayerSessionState>;
}

pub(super) struct BaseState<'a> {
    core: &'a mut ServerCore,
}

pub(super) struct BaseStateRef<'a> {
    core: &'a ServerCore,
}

pub(super) struct OverlayState<'a> {
    base: &'a mut ServerCore,
    overlay: &'a mut TxOverlay,
}

pub(super) struct OverlayStateRef<'a> {
    base: &'a ServerCore,
    overlay: &'a TxOverlay,
}

impl TxOverlay {
    #[allow(dead_code)]
    pub(super) fn materialize_into(
        self,
        base: &mut ServerCore,
        prepared_players: &BTreeMap<PlayerId, EntityId>,
        finalized_players: &BTreeSet<PlayerId>,
    ) {
        let skipped_players = prepared_players
            .keys()
            .filter(|player_id| !finalized_players.contains(player_id))
            .copied()
            .collect::<BTreeSet<_>>();
        let skipped_entities = prepared_players
            .iter()
            .filter(|(player_id, _)| !finalized_players.contains(player_id))
            .map(|(_, entity_id)| *entity_id)
            .collect::<BTreeSet<_>>();

        for (chunk_pos, chunk) in self.chunks {
            base.world.chunks.insert(chunk_pos, chunk);
        }
        for (position, block_entity) in self.block_entities {
            apply_optional_entry(&mut base.world.block_entities, position, block_entity);
        }
        for (position, viewers) in self.container_viewers {
            apply_optional_entry(&mut base.world.container_viewers, position, viewers);
        }
        for (player_id, snapshot) in self.saved_players {
            apply_optional_entry(&mut base.world.saved_players, player_id, snapshot);
        }
        for (player_id, session) in self.player_sessions {
            if skipped_players.contains(&player_id) {
                continue;
            }
            apply_optional_entry(&mut base.sessions.player_sessions, player_id, session);
        }
        for (player_id, entity_id) in self.players_by_player_id {
            if skipped_players.contains(&player_id) {
                continue;
            }
            apply_optional_entry(
                &mut base.entities.players_by_player_id,
                player_id,
                entity_id,
            );
        }
        for (entity_id, kind) in self.entity_kinds {
            if skipped_entities.contains(&entity_id) {
                continue;
            }
            apply_optional_entry(&mut base.entities.entity_kinds, entity_id, kind);
        }
        for (entity_id, identity) in self.player_identity {
            if skipped_entities.contains(&entity_id) {
                continue;
            }
            apply_optional_entry(&mut base.entities.player_identity, entity_id, identity);
        }
        for (entity_id, transform) in self.player_transform {
            if skipped_entities.contains(&entity_id) {
                continue;
            }
            apply_optional_entry(&mut base.entities.player_transform, entity_id, transform);
        }
        for (entity_id, vitals) in self.player_vitals {
            if skipped_entities.contains(&entity_id) {
                continue;
            }
            apply_optional_entry(&mut base.entities.player_vitals, entity_id, vitals);
        }
        for (entity_id, inventory) in self.player_inventory {
            if skipped_entities.contains(&entity_id) {
                continue;
            }
            apply_optional_entry(&mut base.entities.player_inventory, entity_id, inventory);
        }
        for (entity_id, selected_hotbar) in self.player_selected_hotbar {
            if skipped_entities.contains(&entity_id) {
                continue;
            }
            apply_optional_entry(
                &mut base.entities.player_selected_hotbar,
                entity_id,
                selected_hotbar,
            );
        }
        for (entity_id, mining_state) in self.player_active_mining {
            if skipped_entities.contains(&entity_id) {
                continue;
            }
            apply_optional_entry(
                &mut base.entities.player_active_mining,
                entity_id,
                mining_state,
            );
        }
        for (entity_id, dropped_item) in self.dropped_items {
            apply_optional_entry(&mut base.entities.dropped_items, entity_id, dropped_item);
        }
        if let Some(next_entity_id) = self.next_entity_id {
            base.entities.next_entity_id = next_entity_id;
        }
        if let Some(next_keep_alive_id) = self.next_keep_alive_id {
            base.sessions.next_keep_alive_id = next_keep_alive_id;
        }
    }
}

impl<'a> BaseState<'a> {
    pub(super) fn new(core: &'a mut ServerCore) -> Self {
        Self { core }
    }

    pub(super) fn core(&self) -> &ServerCore {
        self.core
    }

    fn view(&self) -> BaseStateRef<'_> {
        BaseStateRef { core: self.core }
    }
}

impl<'a> BaseStateRef<'a> {
    pub(super) fn new(core: &'a ServerCore) -> Self {
        Self { core }
    }
}

impl<'a> OverlayState<'a> {
    pub(super) fn new(base: &'a mut ServerCore, overlay: &'a mut TxOverlay) -> Self {
        Self { base, overlay }
    }

    fn view(&self) -> OverlayStateRef<'_> {
        OverlayStateRef {
            base: self.base,
            overlay: self.overlay,
        }
    }
}

impl<'a> OverlayStateRef<'a> {
    pub(super) fn new(base: &'a ServerCore, overlay: &'a TxOverlay) -> Self {
        Self { base, overlay }
    }
}

impl CoreStateRead for BaseState<'_> {
    fn content_behavior(&self) -> &dyn super::ContentBehavior {
        self.core.content_behavior.as_ref()
    }

    fn content_behavior_arc(&self) -> Arc<dyn super::ContentBehavior> {
        self.view().content_behavior_arc()
    }

    fn config(&self) -> &CoreConfig {
        &self.core.world.config
    }

    fn world_meta_ref(&self) -> &WorldMeta {
        &self.core.world.world_meta
    }

    fn block_state(&self, position: BlockPos) -> Option<BlockState> {
        self.view().block_state(position)
    }

    fn block_entity(&self, position: BlockPos) -> Option<BlockEntityState> {
        self.view().block_entity(position)
    }

    fn saved_player(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        self.view().saved_player(player_id)
    }

    fn player_entity_id(&self, player_id: PlayerId) -> Option<EntityId> {
        self.view().player_entity_id(player_id)
    }

    fn player_session(&self, player_id: PlayerId) -> Option<PlayerSessionState> {
        self.view().player_session(player_id)
    }

    fn player_identity_by_entity(&self, entity_id: EntityId) -> Option<PlayerIdentity> {
        self.view().player_identity_by_entity(entity_id)
    }

    fn player_transform_by_entity(&self, entity_id: EntityId) -> Option<PlayerTransform> {
        self.view().player_transform_by_entity(entity_id)
    }

    fn player_vitals_by_entity(&self, entity_id: EntityId) -> Option<PlayerVitals> {
        self.view().player_vitals_by_entity(entity_id)
    }

    fn player_inventory_by_entity(&self, entity_id: EntityId) -> Option<PlayerInventory> {
        self.view().player_inventory_by_entity(entity_id)
    }

    fn player_selected_hotbar_by_entity(&self, entity_id: EntityId) -> Option<u8> {
        self.view().player_selected_hotbar_by_entity(entity_id)
    }

    fn player_active_mining_by_entity(&self, entity_id: EntityId) -> Option<ActiveMiningState> {
        self.view().player_active_mining_by_entity(entity_id)
    }

    fn dropped_item_by_entity(&self, entity_id: EntityId) -> Option<DroppedItemState> {
        self.view().dropped_item_by_entity(entity_id)
    }

    fn player_ids(&self) -> Vec<PlayerId> {
        self.view().player_ids()
    }

    fn player_entity_ids(&self) -> BTreeSet<EntityId> {
        self.view().player_entity_ids()
    }

    fn dropped_item_ids(&self) -> Vec<EntityId> {
        self.view().dropped_item_ids()
    }

    fn container_viewers(&self, position: BlockPos) -> Option<WorldContainerViewers> {
        self.view().container_viewers(position)
    }

    fn keepalive_interval_ms(&self) -> u64 {
        self.view().keepalive_interval_ms()
    }
}

impl CoreStateRead for BaseStateRef<'_> {
    fn content_behavior(&self) -> &dyn super::ContentBehavior {
        self.core.content_behavior.as_ref()
    }

    fn content_behavior_arc(&self) -> Arc<dyn super::ContentBehavior> {
        self.core.content_behavior.clone()
    }

    fn config(&self) -> &CoreConfig {
        &self.core.world.config
    }

    fn world_meta_ref(&self) -> &WorldMeta {
        &self.core.world.world_meta
    }

    fn block_state(&self, position: BlockPos) -> Option<BlockState> {
        self.core.block_at(position)
    }

    fn block_entity(&self, position: BlockPos) -> Option<BlockEntityState> {
        self.core.block_entity_at(position)
    }

    fn saved_player(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        self.core.world.saved_players.get(&player_id).cloned()
    }

    fn player_entity_id(&self, player_id: PlayerId) -> Option<EntityId> {
        self.core.player_entity_id(player_id)
    }

    fn player_session(&self, player_id: PlayerId) -> Option<PlayerSessionState> {
        self.core.player_session(player_id).cloned()
    }

    fn player_identity_by_entity(&self, entity_id: EntityId) -> Option<PlayerIdentity> {
        self.core.entities.player_identity.get(&entity_id).cloned()
    }

    fn player_transform_by_entity(&self, entity_id: EntityId) -> Option<PlayerTransform> {
        self.core.entities.player_transform.get(&entity_id).copied()
    }

    fn player_vitals_by_entity(&self, entity_id: EntityId) -> Option<PlayerVitals> {
        self.core.entities.player_vitals.get(&entity_id).copied()
    }

    fn player_inventory_by_entity(&self, entity_id: EntityId) -> Option<PlayerInventory> {
        self.core.entities.player_inventory.get(&entity_id).cloned()
    }

    fn player_selected_hotbar_by_entity(&self, entity_id: EntityId) -> Option<u8> {
        self.core
            .entities
            .player_selected_hotbar
            .get(&entity_id)
            .copied()
    }

    fn player_active_mining_by_entity(&self, entity_id: EntityId) -> Option<ActiveMiningState> {
        self.core
            .entities
            .player_active_mining
            .get(&entity_id)
            .cloned()
    }

    fn dropped_item_by_entity(&self, entity_id: EntityId) -> Option<DroppedItemState> {
        self.core.entities.dropped_items.get(&entity_id).cloned()
    }

    fn player_ids(&self) -> Vec<PlayerId> {
        self.core
            .sessions
            .player_sessions
            .keys()
            .copied()
            .collect::<Vec<_>>()
    }

    fn player_entity_ids(&self) -> BTreeSet<EntityId> {
        self.core
            .entities
            .players_by_player_id
            .values()
            .copied()
            .collect::<BTreeSet<_>>()
    }

    fn dropped_item_ids(&self) -> Vec<EntityId> {
        self.core
            .entities
            .dropped_items
            .keys()
            .copied()
            .collect::<Vec<_>>()
    }

    fn container_viewers(&self, position: BlockPos) -> Option<WorldContainerViewers> {
        self.core.world.container_viewers.get(&position).cloned()
    }

    fn keepalive_interval_ms(&self) -> u64 {
        self.core.sessions.keepalive_interval_ms
    }
}

impl CoreStateMut for BaseState<'_> {
    fn ensure_chunk_mut(&mut self, chunk_pos: ChunkPos) -> &mut ChunkColumn {
        self.core.world.chunks.entry(chunk_pos).or_insert_with(|| {
            self.core
                .content_behavior
                .generate_chunk(&self.core.world.world_meta, chunk_pos)
        })
    }

    fn set_block_state(&mut self, position: BlockPos, block: Option<BlockState>) {
        self.core.set_block_at(position, block);
    }

    fn set_block_entity(&mut self, position: BlockPos, block_entity: Option<BlockEntityState>) {
        match block_entity {
            Some(block_entity) => {
                self.core
                    .world
                    .block_entities
                    .insert(position, block_entity);
            }
            None => {
                self.core.world.block_entities.remove(&position);
            }
        }
    }

    fn player_session_mut(&mut self, player_id: PlayerId) -> Option<&mut PlayerSessionState> {
        self.core.player_session_mut(player_id)
    }

    fn player_session_inventory_mut(
        &mut self,
        player_id: PlayerId,
    ) -> Option<(&mut PlayerSessionState, &mut PlayerInventory)> {
        let entity_id = self.core.player_entity_id(player_id)?;
        let session = self.core.sessions.player_sessions.get_mut(&player_id)?;
        let inventory = self.core.entities.player_inventory.get_mut(&entity_id)?;
        Some((session, inventory))
    }

    fn player_transform_mut(&mut self, entity_id: EntityId) -> Option<&mut PlayerTransform> {
        self.core.entities.player_transform.get_mut(&entity_id)
    }

    fn player_inventory_mut(&mut self, entity_id: EntityId) -> Option<&mut PlayerInventory> {
        self.core.entities.player_inventory.get_mut(&entity_id)
    }

    fn player_selected_hotbar_mut(&mut self, entity_id: EntityId) -> Option<&mut u8> {
        self.core
            .entities
            .player_selected_hotbar
            .get_mut(&entity_id)
    }

    fn player_active_mining_mut(&mut self, entity_id: EntityId) -> Option<&mut ActiveMiningState> {
        self.core.entities.player_active_mining.get_mut(&entity_id)
    }

    fn remove_player_active_mining(&mut self, entity_id: EntityId) -> Option<ActiveMiningState> {
        self.core.entities.player_active_mining.remove(&entity_id)
    }

    fn set_player_active_mining(&mut self, entity_id: EntityId, state: Option<ActiveMiningState>) {
        match state {
            Some(state) => {
                self.core
                    .entities
                    .player_active_mining
                    .insert(entity_id, state);
            }
            None => {
                self.core.entities.player_active_mining.remove(&entity_id);
            }
        }
    }

    fn allocate_entity_id(&mut self) -> EntityId {
        let entity_id = EntityId(self.core.entities.next_entity_id);
        self.core.entities.next_entity_id = self.core.entities.next_entity_id.saturating_add(1);
        entity_id
    }

    fn allocate_keep_alive_id(&mut self) -> i32 {
        let keep_alive_id = self.core.sessions.next_keep_alive_id;
        self.core.sessions.next_keep_alive_id =
            self.core.sessions.next_keep_alive_id.saturating_add(1);
        keep_alive_id
    }

    fn set_entity_kind(&mut self, entity_id: EntityId, kind: Option<EntityKind>) {
        match kind {
            Some(kind) => {
                self.core.entities.entity_kinds.insert(entity_id, kind);
            }
            None => {
                self.core.entities.entity_kinds.remove(&entity_id);
            }
        }
    }

    fn set_dropped_item(&mut self, entity_id: EntityId, item: Option<DroppedItemState>) {
        match item {
            Some(item) => {
                self.core.entities.dropped_items.insert(entity_id, item);
            }
            None => {
                self.core.entities.dropped_items.remove(&entity_id);
            }
        }
    }

    fn take_dropped_item(&mut self, entity_id: EntityId) -> Option<DroppedItemState> {
        self.core.entities.dropped_items.remove(&entity_id)
    }

    fn set_saved_player(&mut self, player_id: PlayerId, snapshot: Option<PlayerSnapshot>) {
        match snapshot {
            Some(snapshot) => {
                self.core.world.saved_players.insert(player_id, snapshot);
            }
            None => {
                self.core.world.saved_players.remove(&player_id);
            }
        }
    }

    fn set_container_viewers(
        &mut self,
        position: BlockPos,
        viewers: Option<WorldContainerViewers>,
    ) {
        match viewers {
            Some(viewers) => {
                self.core.world.container_viewers.insert(position, viewers);
            }
            None => {
                self.core.world.container_viewers.remove(&position);
            }
        }
    }

    fn spawn_online_player(
        &mut self,
        player: PlayerSnapshot,
        now_ms: u64,
        expected_entity_id: Option<EntityId>,
    ) -> EntityId {
        let player_id = player.id;
        let entity_id = expected_entity_id.unwrap_or_else(|| {
            let entity_id = EntityId(self.core.entities.next_entity_id);
            self.core.entities.next_entity_id = self.core.entities.next_entity_id.saturating_add(1);
            entity_id
        });
        if self.core.entities.next_entity_id <= entity_id.0 {
            self.core.entities.next_entity_id = entity_id.0.saturating_add(1);
        }
        let view = ClientView::new(
            player.position.chunk_pos(),
            self.core.world.config.view_distance,
        );
        self.core
            .entities
            .entity_kinds
            .insert(entity_id, EntityKind::Player);
        self.core
            .entities
            .players_by_player_id
            .insert(player_id, entity_id);
        self.core.entities.player_identity.insert(
            entity_id,
            PlayerIdentity {
                player_id,
                username: player.username.clone(),
            },
        );
        self.core.entities.player_transform.insert(
            entity_id,
            PlayerTransform {
                position: player.position,
                yaw: player.yaw,
                pitch: player.pitch,
                on_ground: player.on_ground,
                dimension: player.dimension,
            },
        );
        self.core.entities.player_vitals.insert(
            entity_id,
            PlayerVitals {
                health: player.health,
                food: player.food,
                food_saturation: player.food_saturation,
            },
        );
        self.core
            .entities
            .player_inventory
            .insert(entity_id, player.inventory);
        self.core
            .entities
            .player_selected_hotbar
            .insert(entity_id, player.selected_hotbar_slot);
        self.core.entities.player_active_mining.remove(&entity_id);
        self.core.sessions.player_sessions.insert(
            player_id,
            PlayerSessionState {
                entity_id,
                cursor: None,
                active_container: None,
                next_non_player_window_id: 1,
                view,
                pending_keep_alive_id: None,
                last_keep_alive_sent_at: None,
                next_keep_alive_at: now_ms.saturating_add(self.core.sessions.keepalive_interval_ms),
            },
        );
        entity_id
    }

    fn remove_online_player(&mut self, player_id: PlayerId) -> Option<PlayerSessionState> {
        self.core.remove_online_player(player_id)
    }
}

impl CoreStateRead for OverlayState<'_> {
    fn content_behavior(&self) -> &dyn super::ContentBehavior {
        self.base.content_behavior.as_ref()
    }

    fn content_behavior_arc(&self) -> Arc<dyn super::ContentBehavior> {
        self.view().content_behavior_arc()
    }

    fn config(&self) -> &CoreConfig {
        &self.base.world.config
    }

    fn world_meta_ref(&self) -> &WorldMeta {
        &self.base.world.world_meta
    }

    fn block_state(&self, position: BlockPos) -> Option<BlockState> {
        self.view().block_state(position)
    }

    fn block_entity(&self, position: BlockPos) -> Option<BlockEntityState> {
        self.view().block_entity(position)
    }

    fn saved_player(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        self.view().saved_player(player_id)
    }

    fn player_entity_id(&self, player_id: PlayerId) -> Option<EntityId> {
        self.view().player_entity_id(player_id)
    }

    fn player_session(&self, player_id: PlayerId) -> Option<PlayerSessionState> {
        self.view().player_session(player_id)
    }

    fn player_identity_by_entity(&self, entity_id: EntityId) -> Option<PlayerIdentity> {
        self.view().player_identity_by_entity(entity_id)
    }

    fn player_transform_by_entity(&self, entity_id: EntityId) -> Option<PlayerTransform> {
        self.view().player_transform_by_entity(entity_id)
    }

    fn player_vitals_by_entity(&self, entity_id: EntityId) -> Option<PlayerVitals> {
        self.view().player_vitals_by_entity(entity_id)
    }

    fn player_inventory_by_entity(&self, entity_id: EntityId) -> Option<PlayerInventory> {
        self.view().player_inventory_by_entity(entity_id)
    }

    fn player_selected_hotbar_by_entity(&self, entity_id: EntityId) -> Option<u8> {
        self.view().player_selected_hotbar_by_entity(entity_id)
    }

    fn player_active_mining_by_entity(&self, entity_id: EntityId) -> Option<ActiveMiningState> {
        self.view().player_active_mining_by_entity(entity_id)
    }

    fn dropped_item_by_entity(&self, entity_id: EntityId) -> Option<DroppedItemState> {
        self.view().dropped_item_by_entity(entity_id)
    }

    fn player_ids(&self) -> Vec<PlayerId> {
        self.view().player_ids()
    }

    fn player_entity_ids(&self) -> BTreeSet<EntityId> {
        self.view().player_entity_ids()
    }

    fn dropped_item_ids(&self) -> Vec<EntityId> {
        self.view().dropped_item_ids()
    }

    fn container_viewers(&self, position: BlockPos) -> Option<WorldContainerViewers> {
        self.view().container_viewers(position)
    }

    fn keepalive_interval_ms(&self) -> u64 {
        self.view().keepalive_interval_ms()
    }
}

impl CoreStateRead for OverlayStateRef<'_> {
    fn content_behavior(&self) -> &dyn super::ContentBehavior {
        self.base.content_behavior.as_ref()
    }

    fn content_behavior_arc(&self) -> Arc<dyn super::ContentBehavior> {
        self.base.content_behavior.clone()
    }

    fn config(&self) -> &CoreConfig {
        &self.base.world.config
    }

    fn world_meta_ref(&self) -> &WorldMeta {
        &self.base.world.world_meta
    }

    fn block_state(&self, position: BlockPos) -> Option<BlockState> {
        let chunk_pos = position.chunk_pos();
        if let Some(chunk) = self.overlay.chunks.get(&chunk_pos) {
            return chunk.get_block(local_block_x(position), position.y, local_block_z(position));
        }
        self.base.block_at(position)
    }

    fn block_entity(&self, position: BlockPos) -> Option<BlockEntityState> {
        if let Some(entry) = self.overlay.block_entities.get(&position) {
            return entry.clone();
        }
        self.base.block_entity_at(position)
    }

    fn saved_player(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        if let Some(entry) = self.overlay.saved_players.get(&player_id) {
            return entry.clone();
        }
        self.base.world.saved_players.get(&player_id).cloned()
    }

    fn player_entity_id(&self, player_id: PlayerId) -> Option<EntityId> {
        if let Some(entry) = self.overlay.players_by_player_id.get(&player_id) {
            return *entry;
        }
        self.base.player_entity_id(player_id)
    }

    fn player_session(&self, player_id: PlayerId) -> Option<PlayerSessionState> {
        if let Some(entry) = self.overlay.player_sessions.get(&player_id) {
            return entry.clone();
        }
        self.base.player_session(player_id).cloned()
    }

    fn player_identity_by_entity(&self, entity_id: EntityId) -> Option<PlayerIdentity> {
        if let Some(entry) = self.overlay.player_identity.get(&entity_id) {
            return entry.clone();
        }
        self.base.entities.player_identity.get(&entity_id).cloned()
    }

    fn player_transform_by_entity(&self, entity_id: EntityId) -> Option<PlayerTransform> {
        if let Some(entry) = self.overlay.player_transform.get(&entity_id) {
            return *entry;
        }
        self.base.entities.player_transform.get(&entity_id).copied()
    }

    fn player_vitals_by_entity(&self, entity_id: EntityId) -> Option<PlayerVitals> {
        if let Some(entry) = self.overlay.player_vitals.get(&entity_id) {
            return *entry;
        }
        self.base.entities.player_vitals.get(&entity_id).copied()
    }

    fn player_inventory_by_entity(&self, entity_id: EntityId) -> Option<PlayerInventory> {
        if let Some(entry) = self.overlay.player_inventory.get(&entity_id) {
            return entry.clone();
        }
        self.base.entities.player_inventory.get(&entity_id).cloned()
    }

    fn player_selected_hotbar_by_entity(&self, entity_id: EntityId) -> Option<u8> {
        if let Some(entry) = self.overlay.player_selected_hotbar.get(&entity_id) {
            return *entry;
        }
        self.base
            .entities
            .player_selected_hotbar
            .get(&entity_id)
            .copied()
    }

    fn player_active_mining_by_entity(&self, entity_id: EntityId) -> Option<ActiveMiningState> {
        if let Some(entry) = self.overlay.player_active_mining.get(&entity_id) {
            return entry.clone();
        }
        self.base
            .entities
            .player_active_mining
            .get(&entity_id)
            .cloned()
    }

    fn dropped_item_by_entity(&self, entity_id: EntityId) -> Option<DroppedItemState> {
        if let Some(entry) = self.overlay.dropped_items.get(&entity_id) {
            return entry.clone();
        }
        self.base.entities.dropped_items.get(&entity_id).cloned()
    }

    fn player_ids(&self) -> Vec<PlayerId> {
        let mut player_ids = self
            .base
            .sessions
            .player_sessions
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        for (player_id, entry) in &self.overlay.player_sessions {
            match entry {
                Some(_) => {
                    player_ids.insert(*player_id);
                }
                None => {
                    player_ids.remove(player_id);
                }
            }
        }
        player_ids.into_iter().collect()
    }

    fn player_entity_ids(&self) -> BTreeSet<EntityId> {
        let mut entity_ids = self
            .base
            .entities
            .players_by_player_id
            .values()
            .copied()
            .collect::<BTreeSet<_>>();
        for (player_id, entry) in &self.overlay.players_by_player_id {
            match entry {
                Some(entity_id) => {
                    entity_ids.insert(*entity_id);
                }
                None => {
                    if let Some(entity_id) = self.base.entities.players_by_player_id.get(player_id)
                    {
                        entity_ids.remove(entity_id);
                    }
                }
            }
        }
        entity_ids
    }

    fn dropped_item_ids(&self) -> Vec<EntityId> {
        let mut entity_ids = self
            .base
            .entities
            .dropped_items
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        for (entity_id, entry) in &self.overlay.dropped_items {
            match entry {
                Some(_) => {
                    entity_ids.insert(*entity_id);
                }
                None => {
                    entity_ids.remove(entity_id);
                }
            }
        }
        entity_ids.into_iter().collect()
    }

    fn container_viewers(&self, position: BlockPos) -> Option<WorldContainerViewers> {
        if let Some(entry) = self.overlay.container_viewers.get(&position) {
            return entry.clone();
        }
        self.base.world.container_viewers.get(&position).cloned()
    }

    fn keepalive_interval_ms(&self) -> u64 {
        self.base.sessions.keepalive_interval_ms
    }
}

impl CoreStateMut for OverlayState<'_> {
    fn ensure_chunk_mut(&mut self, chunk_pos: ChunkPos) -> &mut ChunkColumn {
        self.overlay.chunks.entry(chunk_pos).or_insert_with(|| {
            self.base
                .world
                .chunks
                .get(&chunk_pos)
                .cloned()
                .unwrap_or_else(|| {
                    self.base
                        .content_behavior
                        .generate_chunk(&self.base.world.world_meta, chunk_pos)
                })
        })
    }

    fn set_block_state(&mut self, position: BlockPos, block: Option<BlockState>) {
        self.ensure_chunk_mut(position.chunk_pos()).set_block(
            local_block_x(position),
            position.y,
            local_block_z(position),
            block,
        );
    }

    fn set_block_entity(&mut self, position: BlockPos, block_entity: Option<BlockEntityState>) {
        self.overlay.block_entities.insert(position, block_entity);
    }

    fn player_session_mut(&mut self, player_id: PlayerId) -> Option<&mut PlayerSessionState> {
        if !self.overlay.player_sessions.contains_key(&player_id) {
            let session = self.base.player_session(player_id)?.clone();
            self.overlay
                .player_sessions
                .insert(player_id, Some(session));
        }
        self.overlay
            .player_sessions
            .get_mut(&player_id)
            .and_then(Option::as_mut)
    }

    fn player_session_inventory_mut(
        &mut self,
        player_id: PlayerId,
    ) -> Option<(&mut PlayerSessionState, &mut PlayerInventory)> {
        let entity_id = self.player_entity_id(player_id)?;
        if !self.overlay.player_sessions.contains_key(&player_id) {
            let session = self.base.player_session(player_id)?.clone();
            self.overlay
                .player_sessions
                .insert(player_id, Some(session));
        }
        if !self.overlay.player_inventory.contains_key(&entity_id) {
            let inventory = self.player_inventory_by_entity(entity_id)?;
            self.overlay
                .player_inventory
                .insert(entity_id, Some(inventory));
        }
        let overlay = &mut self.overlay;
        let session = overlay
            .player_sessions
            .get_mut(&player_id)
            .and_then(Option::as_mut)?;
        let inventory = overlay
            .player_inventory
            .get_mut(&entity_id)
            .and_then(Option::as_mut)?;
        Some((session, inventory))
    }

    fn player_transform_mut(&mut self, entity_id: EntityId) -> Option<&mut PlayerTransform> {
        if !self.overlay.player_transform.contains_key(&entity_id) {
            let transform = self
                .base
                .entities
                .player_transform
                .get(&entity_id)
                .copied()?;
            self.overlay
                .player_transform
                .insert(entity_id, Some(transform));
        }
        self.overlay
            .player_transform
            .get_mut(&entity_id)
            .and_then(Option::as_mut)
    }

    fn player_inventory_mut(&mut self, entity_id: EntityId) -> Option<&mut PlayerInventory> {
        if !self.overlay.player_inventory.contains_key(&entity_id) {
            let inventory = self.base.entities.player_inventory.get(&entity_id)?.clone();
            self.overlay
                .player_inventory
                .insert(entity_id, Some(inventory));
        }
        self.overlay
            .player_inventory
            .get_mut(&entity_id)
            .and_then(Option::as_mut)
    }

    fn player_selected_hotbar_mut(&mut self, entity_id: EntityId) -> Option<&mut u8> {
        if !self.overlay.player_selected_hotbar.contains_key(&entity_id) {
            let slot = *self.base.entities.player_selected_hotbar.get(&entity_id)?;
            self.overlay
                .player_selected_hotbar
                .insert(entity_id, Some(slot));
        }
        self.overlay
            .player_selected_hotbar
            .get_mut(&entity_id)
            .and_then(Option::as_mut)
    }

    fn player_active_mining_mut(&mut self, entity_id: EntityId) -> Option<&mut ActiveMiningState> {
        if !self.overlay.player_active_mining.contains_key(&entity_id) {
            let state = self
                .base
                .entities
                .player_active_mining
                .get(&entity_id)?
                .clone();
            self.overlay
                .player_active_mining
                .insert(entity_id, Some(state));
        }
        self.overlay
            .player_active_mining
            .get_mut(&entity_id)
            .and_then(Option::as_mut)
    }

    fn remove_player_active_mining(&mut self, entity_id: EntityId) -> Option<ActiveMiningState> {
        let current = self.player_active_mining_by_entity(entity_id)?;
        self.overlay.player_active_mining.insert(entity_id, None);
        Some(current)
    }

    fn set_player_active_mining(&mut self, entity_id: EntityId, state: Option<ActiveMiningState>) {
        self.overlay.player_active_mining.insert(entity_id, state);
    }

    fn allocate_entity_id(&mut self) -> EntityId {
        let next_entity_id = self
            .overlay
            .next_entity_id
            .get_or_insert(self.base.entities.next_entity_id);
        let entity_id = EntityId(*next_entity_id);
        *next_entity_id = next_entity_id.saturating_add(1);
        entity_id
    }

    fn allocate_keep_alive_id(&mut self) -> i32 {
        let next_keep_alive_id = self
            .overlay
            .next_keep_alive_id
            .get_or_insert(self.base.sessions.next_keep_alive_id);
        let keep_alive_id = *next_keep_alive_id;
        *next_keep_alive_id = next_keep_alive_id.saturating_add(1);
        keep_alive_id
    }

    fn set_entity_kind(&mut self, entity_id: EntityId, kind: Option<EntityKind>) {
        self.overlay.entity_kinds.insert(entity_id, kind);
    }

    fn set_dropped_item(&mut self, entity_id: EntityId, item: Option<DroppedItemState>) {
        self.overlay.dropped_items.insert(entity_id, item);
    }

    fn take_dropped_item(&mut self, entity_id: EntityId) -> Option<DroppedItemState> {
        let item = self.dropped_item_by_entity(entity_id)?;
        self.overlay.dropped_items.insert(entity_id, None);
        Some(item)
    }

    fn set_saved_player(&mut self, player_id: PlayerId, snapshot: Option<PlayerSnapshot>) {
        self.overlay.saved_players.insert(player_id, snapshot);
    }

    fn set_container_viewers(
        &mut self,
        position: BlockPos,
        viewers: Option<WorldContainerViewers>,
    ) {
        self.overlay.container_viewers.insert(position, viewers);
    }

    fn spawn_online_player(
        &mut self,
        player: PlayerSnapshot,
        now_ms: u64,
        expected_entity_id: Option<EntityId>,
    ) -> EntityId {
        let player_id = player.id;
        let entity_id = expected_entity_id.unwrap_or_else(|| self.allocate_entity_id());
        if let Some(next_entity_id) = &mut self.overlay.next_entity_id {
            *next_entity_id = (*next_entity_id).max(entity_id.0.saturating_add(1));
        } else if self.base.entities.next_entity_id <= entity_id.0 {
            self.overlay.next_entity_id = Some(entity_id.0.saturating_add(1));
        }
        let view = ClientView::new(
            player.position.chunk_pos(),
            self.base.world.config.view_distance,
        );
        self.overlay
            .entity_kinds
            .insert(entity_id, Some(EntityKind::Player));
        self.overlay
            .players_by_player_id
            .insert(player_id, Some(entity_id));
        self.overlay.player_identity.insert(
            entity_id,
            Some(PlayerIdentity {
                player_id,
                username: player.username.clone(),
            }),
        );
        self.overlay.player_transform.insert(
            entity_id,
            Some(PlayerTransform {
                position: player.position,
                yaw: player.yaw,
                pitch: player.pitch,
                on_ground: player.on_ground,
                dimension: player.dimension,
            }),
        );
        self.overlay.player_vitals.insert(
            entity_id,
            Some(PlayerVitals {
                health: player.health,
                food: player.food,
                food_saturation: player.food_saturation,
            }),
        );
        self.overlay
            .player_inventory
            .insert(entity_id, Some(player.inventory));
        self.overlay
            .player_selected_hotbar
            .insert(entity_id, Some(player.selected_hotbar_slot));
        self.overlay.player_active_mining.insert(entity_id, None);
        self.overlay.player_sessions.insert(
            player_id,
            Some(PlayerSessionState {
                entity_id,
                cursor: None,
                active_container: None,
                next_non_player_window_id: 1,
                view,
                pending_keep_alive_id: None,
                last_keep_alive_sent_at: None,
                next_keep_alive_at: now_ms.saturating_add(self.base.sessions.keepalive_interval_ms),
            }),
        );
        entity_id
    }

    fn remove_online_player(&mut self, player_id: PlayerId) -> Option<PlayerSessionState> {
        let session = self.player_session(player_id)?;
        let entity_id = session.entity_id;
        self.overlay.player_sessions.insert(player_id, None);
        self.overlay.players_by_player_id.insert(player_id, None);
        self.overlay.entity_kinds.insert(entity_id, None);
        self.overlay.player_identity.insert(entity_id, None);
        self.overlay.player_transform.insert(entity_id, None);
        self.overlay.player_vitals.insert(entity_id, None);
        self.overlay.player_inventory.insert(entity_id, None);
        self.overlay.player_selected_hotbar.insert(entity_id, None);
        self.overlay.player_active_mining.insert(entity_id, None);
        Some(session)
    }
}

#[allow(dead_code)]
fn apply_optional_entry<K, V>(map: &mut BTreeMap<K, V>, key: K, value: Option<V>)
where
    K: Ord + Clone,
{
    match value {
        Some(value) => {
            map.insert(key, value);
        }
        None => {
            map.remove(&key);
        }
    }
}

pub(super) fn initial_visible_chunks(
    state: &mut impl CoreStateMut,
    center: ChunkPos,
    view_distance: u8,
) -> Vec<ChunkColumn> {
    required_chunks(center, view_distance)
        .into_iter()
        .map(|chunk_pos| state.ensure_chunk_mut(chunk_pos).clone())
        .collect()
}

fn local_block_x(position: BlockPos) -> u8 {
    u8::try_from(position.x.rem_euclid(crate::CHUNK_WIDTH)).expect("local x should fit into u8")
}

fn local_block_z(position: BlockPos) -> u8 {
    u8::try_from(position.z.rem_euclid(crate::CHUNK_WIDTH)).expect("local z should fit into u8")
}

fn distance_squared_to_block_center(position: crate::Vec3, block: BlockPos) -> f64 {
    let eye_x = position.x;
    let eye_y = position.y + 1.62;
    let eye_z = position.z;
    let center_x = f64::from(block.x) + 0.5;
    let center_y = f64::from(block.y) + 0.5;
    let center_z = f64::from(block.z) + 0.5;
    let dx = eye_x - center_x;
    let dy = eye_y - center_y;
    let dz = eye_z - center_z;
    dx * dx + dy * dy + dz * dz
}

fn block_intersects_player(block: BlockPos, player: &PlayerTransform) -> bool {
    let half_width = PLAYER_WIDTH / 2.0;
    let player_min_x = player.position.x - half_width;
    let player_max_x = player.position.x + half_width;
    let player_min_y = player.position.y;
    let player_max_y = player.position.y + PLAYER_HEIGHT;
    let player_min_z = player.position.z - half_width;
    let player_max_z = player.position.z + half_width;

    let block_min_x = f64::from(block.x);
    let block_max_x = block_min_x + 1.0;
    let block_min_y = f64::from(block.y);
    let block_max_y = block_min_y + 1.0;
    let block_min_z = f64::from(block.z);
    let block_max_z = block_min_z + 1.0;

    player_min_x < block_max_x
        && player_max_x > block_min_x
        && player_min_y < block_max_y
        && player_max_y > block_min_y
        && player_min_z < block_max_z
        && player_max_z > block_min_z
}
