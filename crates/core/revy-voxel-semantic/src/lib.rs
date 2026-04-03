#![allow(clippy::multiple_crate_versions)]

mod config;
mod events;
mod gameplay;
mod model;
mod player;
mod rules;
mod world;

pub use self::config::CoreConfig;
pub use self::events::{
    CoreCommand, CoreEvent, EventTarget, GameplayCommand, PlayerSummary, RuntimeCommand,
    SessionCommand, TargetedEvent,
};
pub use self::gameplay::{
    GameplayCanEditBlockKey, GameplayEffect, GameplayEffectBatch, GameplayReadSet,
};
pub use self::model::{
    BlockFace, BlockKey, BlockPos, BlockState, ChunkColumn, ChunkDelta, ChunkPos, ChunkSection,
    DimensionId, DroppedItemSnapshot, InteractionHand, InventoryClickButton, InventoryClickTarget,
    InventoryClickValidation, InventorySlot, InventoryTransactionContext, InventoryWindowContents,
    ItemDataMap, ItemDataValue, ItemKey, ItemStack, OpaqueF32, OpaqueF64, PlayerInventory,
    SectionBlockIndex, SectionPos, Vec3, WorldMeta, expand_block_index, flatten_block_index,
    required_chunks, section_local_y,
};
pub use self::player::PlayerSnapshot;
pub use self::rules::{
    BlockDescriptor, BlockEntityKindId, BlockEntityState, ContainerBinding,
    ContainerBlockEntityState, ContainerKindId, ContainerPropertyKey, ContainerSlotRole,
    ContainerSpec, ContentBehavior, ItemDescriptor, MiningToolSpec, OpenContainerState, ToolClass,
};
pub use self::world::WorldSnapshot;
pub use revy_core::{
    AdapterId, AdminSurfaceCapability, AdminSurfaceCapabilitySet, AdminSurfaceProfileId,
    AuthCapability, AuthCapabilitySet, AuthProfileId, CapabilityAnnouncement, CapabilityParseError,
    ClosedCapability, ClosedCapabilitySet, ConnectionId, ConnectionIdSource, EntityId,
    GameplayCapability, GameplayCapabilitySet, GameplayProfileId, PlayerId, PluginBuildTag,
    PluginGenerationId, ProtocolCapability, ProtocolCapabilitySet, RevisionConflict, Revisioned,
    SessionCapabilitySet, SessionRoutes, StorageCapability, StorageCapabilitySet, StorageProfileId,
};
