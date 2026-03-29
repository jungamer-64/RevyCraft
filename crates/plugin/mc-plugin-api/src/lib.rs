#![allow(clippy::multiple_crate_versions)]
pub mod abi;
pub mod codec;
pub mod host_api;
pub mod manifest;

pub mod semantic {
    pub use revy_voxel_core::{
        AdapterId, AdminSurfaceCapability, AdminSurfaceCapabilitySet, AdminSurfaceProfileId,
        AuthCapability, AuthCapabilitySet, AuthProfileId, CapabilityAnnouncement,
        ClosedCapabilitySet, ConnectionId, CoreCommand, CoreConfig, CoreEvent, EntityId,
        EventTarget, GameplayCapability, GameplayCapabilitySet, GameplayCommand, GameplayJournal,
        GameplayJournalApplyResult, GameplayProfileId, GameplayTransaction, PlayerId,
        PlayerSnapshot, PluginBuildTag, PluginGenerationId, ProtocolCapability,
        ProtocolCapabilitySet, RuntimeCommand, ServerCore, SessionCapabilitySet, SessionCommand,
        StorageCapability, StorageCapabilitySet, StorageProfileId, TargetedEvent, WorldSnapshot,
    };
}

pub use self::semantic::{
    AdapterId, AdminSurfaceCapability, AdminSurfaceCapabilitySet, AdminSurfaceProfileId,
    AuthCapability, AuthCapabilitySet, AuthProfileId, CapabilityAnnouncement, ClosedCapabilitySet,
    ConnectionId, CoreCommand, CoreConfig, CoreEvent, EntityId, EventTarget, GameplayCapability,
    GameplayCapabilitySet, GameplayCommand, GameplayJournal, GameplayJournalApplyResult,
    GameplayProfileId, GameplayTransaction, PlayerId, PlayerSnapshot, PluginBuildTag,
    PluginGenerationId, ProtocolCapability, ProtocolCapabilitySet, RuntimeCommand, ServerCore,
    SessionCapabilitySet, SessionCommand, StorageCapability, StorageCapabilitySet,
    StorageProfileId, TargetedEvent, WorldSnapshot,
};
