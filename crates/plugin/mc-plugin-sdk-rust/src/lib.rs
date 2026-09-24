#![allow(clippy::multiple_crate_versions)]

use mc_plugin_abi::manifest::PluginManifestV9;
use mc_plugin_abi::raw::{CapabilityDescriptorV9, OwnedBuffer, Utf8Slice};
use mc_plugin_abi::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
use mc_plugin_contract::codec::auth::{AuthDescriptor, BedrockAuthResult};
use mc_plugin_contract::codec::gameplay::{GameplayDescriptor, GameplaySessionSnapshot};
use mc_plugin_contract::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_contract::codec::storage::StorageDescriptor;
use mc_plugin_contract::plugin::PluginKind;
use mc_proto_common::{HandshakeProbe, ProtocolAdapter, ProtocolError};
use mc_storage_common::StorageError;
use std::path::Path;

#[doc(hidden)]
pub mod __macro_support;
pub mod admin_surface;
pub mod auth;
pub mod buffers;
pub mod capabilities;
pub mod gameplay;
pub mod instance;
mod macros;
pub mod manifest;
pub mod protocol;
pub mod storage;

pub use revy_voxel_semantic::{
    AdapterId, AdminSurfaceCapability, AdminSurfaceCapabilitySet, AdminSurfaceProfileId,
    AuthCapability, AuthCapabilitySet, AuthProfileId, BlockDescriptor, BlockEntityKindId,
    BlockEntityState, BlockFace, BlockKey, BlockPos, BlockState, CapabilityAnnouncement,
    CapabilityParseError, ChunkColumn, ChunkDelta, ChunkPos, ChunkSection, ClosedCapability,
    ClosedCapabilitySet, ConnectionId, ConnectionIdSource, ContainerBinding,
    ContainerBlockEntityState, ContainerKindId, ContainerPropertyKey, ContainerSlotRole,
    ContainerSpec, ContentBehavior, CoreCommand, CoreConfig, CoreEvent, DimensionId,
    DroppedItemSnapshot, EntityId, EventTarget, GameplayCanEditBlockKey, GameplayCapability,
    GameplayCapabilitySet, GameplayCommand, GameplayEffect, GameplayEffectBatch, GameplayProfileId,
    GameplayReadSet, InteractionHand, InventoryClickButton, InventoryClickTarget,
    InventoryClickValidation, InventorySlot, InventoryTransactionContext, InventoryWindowContents,
    ItemDataMap, ItemDataValue, ItemDescriptor, ItemKey, ItemStack, MiningToolSpec, OpaqueF32,
    OpaqueF64, OpenContainerState, PlayerId, PlayerInventory, PlayerSnapshot, PlayerSummary,
    PluginBuildTag, PluginGenerationId, ProtocolCapability, ProtocolCapabilitySet,
    RevisionConflict, Revisioned, RuntimeCommand, SectionBlockIndex, SectionPos,
    SessionCapabilitySet, SessionCommand, StorageCapability, StorageCapabilitySet,
    StorageProfileId, TargetedEvent, ToolClass, Vec3, WorldMeta, WorldSnapshot, expand_block_index,
    flatten_block_index, required_chunks, section_local_y,
};
