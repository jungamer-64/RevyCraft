use crate::inventory::{InventoryContainer, InventorySlot, InventoryWindowContents, ItemStack};
use crate::player::{InteractionHand, PlayerSnapshot};
use crate::world::{
    BlockFace, BlockPos, BlockState, ChunkColumn, DroppedItemSnapshot, Vec3, WorldMeta,
};
use crate::{ConnectionId, EntityId, PlayerId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryTransactionContext {
    pub window_id: u8,
    pub action_number: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InventoryClickButton {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InventoryClickTarget {
    Slot(InventorySlot),
    Outside,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum InventoryClickValidation {
    StrictSlotEcho { clicked_item: Option<ItemStack> },
    Authoritative,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CoreCommand {
    LoginStart {
        connection_id: ConnectionId,
        username: String,
        player_id: PlayerId,
    },
    UpdateClientView {
        player_id: PlayerId,
        view_distance: u8,
    },
    ClientStatus {
        player_id: PlayerId,
        action_id: i8,
    },
    MoveIntent {
        player_id: PlayerId,
        position: Option<Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    },
    KeepAliveResponse {
        player_id: PlayerId,
        keep_alive_id: i32,
    },
    SetHeldSlot {
        player_id: PlayerId,
        slot: i16,
    },
    CreativeInventorySet {
        player_id: PlayerId,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    },
    InventoryClick {
        player_id: PlayerId,
        transaction: InventoryTransactionContext,
        target: InventoryClickTarget,
        button: InventoryClickButton,
        validation: InventoryClickValidation,
    },
    InventoryTransactionAck {
        player_id: PlayerId,
        transaction: InventoryTransactionContext,
        accepted: bool,
    },
    CloseContainer {
        player_id: PlayerId,
        window_id: u8,
    },
    DigBlock {
        player_id: PlayerId,
        position: BlockPos,
        status: u8,
        face: Option<BlockFace>,
    },
    PlaceBlock {
        player_id: PlayerId,
        hand: InteractionHand,
        position: BlockPos,
        face: Option<BlockFace>,
        held_item: Option<ItemStack>,
    },
    UseBlock {
        player_id: PlayerId,
        hand: InteractionHand,
        position: BlockPos,
        face: Option<BlockFace>,
        held_item: Option<ItemStack>,
    },
    Disconnect {
        player_id: PlayerId,
    },
}

impl CoreCommand {
    #[must_use]
    pub const fn player_id(&self) -> Option<PlayerId> {
        match self {
            Self::LoginStart { player_id, .. }
            | Self::UpdateClientView { player_id, .. }
            | Self::ClientStatus { player_id, .. }
            | Self::MoveIntent { player_id, .. }
            | Self::KeepAliveResponse { player_id, .. }
            | Self::SetHeldSlot { player_id, .. }
            | Self::CreativeInventorySet { player_id, .. }
            | Self::InventoryClick { player_id, .. }
            | Self::InventoryTransactionAck { player_id, .. }
            | Self::CloseContainer { player_id, .. }
            | Self::DigBlock { player_id, .. }
            | Self::PlaceBlock { player_id, .. }
            | Self::UseBlock { player_id, .. }
            | Self::Disconnect { player_id, .. } => Some(*player_id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CoreEvent {
    LoginAccepted {
        player_id: PlayerId,
        entity_id: EntityId,
        player: PlayerSnapshot,
    },
    PlayBootstrap {
        player: PlayerSnapshot,
        entity_id: EntityId,
        world_meta: WorldMeta,
        view_distance: u8,
    },
    ChunkBatch {
        chunks: Vec<ChunkColumn>,
    },
    EntitySpawned {
        entity_id: EntityId,
        player: PlayerSnapshot,
    },
    EntityMoved {
        entity_id: EntityId,
        player: PlayerSnapshot,
    },
    DroppedItemSpawned {
        entity_id: EntityId,
        item: DroppedItemSnapshot,
    },
    BlockBreakingProgress {
        breaker_entity_id: EntityId,
        position: BlockPos,
        stage: Option<u8>,
        duration_ms: u64,
    },
    EntityDespawned {
        entity_ids: Vec<EntityId>,
    },
    InventoryContents {
        window_id: u8,
        container: InventoryContainer,
        contents: InventoryWindowContents,
    },
    ContainerOpened {
        window_id: u8,
        container: InventoryContainer,
        title: String,
    },
    ContainerClosed {
        window_id: u8,
    },
    ContainerPropertyChanged {
        window_id: u8,
        property_id: u8,
        value: i16,
    },
    InventorySlotChanged {
        window_id: u8,
        container: InventoryContainer,
        slot: InventorySlot,
        stack: Option<ItemStack>,
    },
    InventoryTransactionProcessed {
        transaction: InventoryTransactionContext,
        accepted: bool,
    },
    CursorChanged {
        stack: Option<ItemStack>,
    },
    SelectedHotbarSlotChanged {
        slot: u8,
    },
    BlockChanged {
        position: BlockPos,
        block: BlockState,
    },
    KeepAliveRequested {
        keep_alive_id: i32,
    },
    Disconnect {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventTarget {
    Connection(ConnectionId),
    Player(PlayerId),
    EveryoneExcept(PlayerId),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetedEvent {
    pub target: EventTarget,
    pub event: CoreEvent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerSummary {
    pub online_players: usize,
    pub max_players: u8,
}
