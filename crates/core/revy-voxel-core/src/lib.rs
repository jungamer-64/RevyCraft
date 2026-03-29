#![allow(clippy::multiple_crate_versions)]

pub(crate) mod inventory;

pub(crate) mod core;
pub(crate) mod events;
pub(crate) mod player;
#[cfg(test)]
mod tests;
pub(crate) mod world;

pub use self::core::transaction::{
    GameplayJournal, GameplayJournalApplyResult, GameplayTransaction,
};
pub use self::core::{
    ActiveMiningState, ClientView, CoreConfig, CoreRuntimeStateBlob, DroppedItemState,
    OnlinePlayerRuntimeState, OpenInventoryWindow, PlayerSessionState, ServerCore,
    WorldContainerViewers,
};
pub use self::events::{
    CoreCommand, CoreEvent, EventTarget, GameplayCommand, PlayerSummary, RuntimeCommand,
    SessionCommand, TargetedEvent,
};
pub use self::player::PlayerSnapshot;
pub use self::world::WorldSnapshot;
pub use revy_core::{
    AdapterId, AdminSurfaceCapability, AdminSurfaceCapabilitySet, AdminSurfaceProfileId,
    AuthCapability, AuthCapabilitySet, AuthProfileId, CapabilityAnnouncement, CapabilityParseError,
    ClosedCapability, ClosedCapabilitySet, ConnectionId, ConnectionIdSource, EntityId,
    GameplayCapability, GameplayCapabilitySet, GameplayProfileId, PlayerId, PluginBuildTag,
    PluginGenerationId, ProtocolCapability, ProtocolCapabilitySet, RevisionConflict, Revisioned,
    SessionCapabilitySet, SessionRoutes, StorageCapability, StorageCapabilitySet, StorageProfileId,
};

#[allow(unused_imports)]
pub(crate) use revy_voxel_model::{
    BlockFace, BlockKey, BlockPos, BlockState, ChunkColumn, ChunkDelta, ChunkPos, ChunkSection,
    DimensionId, DroppedItemSnapshot, InteractionHand, InventoryClickButton, InventoryClickTarget,
    InventoryClickValidation, InventorySlot, InventoryTransactionContext, InventoryWindowContents,
    ItemKey, ItemStack, PlayerInventory, SectionBlockIndex, SectionPos, Vec3, WorldMeta,
    expand_block_index,
};
#[allow(unused_imports)]
pub(crate) use revy_voxel_rules::{
    BlockDescriptor, BlockEntityKindId, BlockEntityState, ContainerBinding,
    ContainerBlockEntityState, ContainerKindId, ContainerPropertyKey, ContainerSlotRole,
    ContainerSpec, ContentBehavior, ItemDescriptor, MiningToolSpec, OpenContainerState, ToolClass,
};

#[cfg(test)]
pub(crate) use self::world::flatten_block_index;

const CHUNK_WIDTH: i32 = 16;
const DEFAULT_KEEPALIVE_INTERVAL_MS: u64 = 10_000;
const DEFAULT_KEEPALIVE_TIMEOUT_MS: u64 = 30_000;
const HOTBAR_SLOT_COUNT: u8 = 9;
const PLAYER_WIDTH: f64 = 0.6;
const PLAYER_HEIGHT: f64 = 1.8;
const BLOCK_EDIT_REACH: f64 = 6.0;
