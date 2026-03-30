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
pub use self::model::*;
pub use self::player::PlayerSnapshot;
pub use self::rules::*;
pub use self::world::WorldSnapshot;
pub use revy_core::{
    AdapterId, AdminSurfaceCapability, AdminSurfaceCapabilitySet, AdminSurfaceProfileId,
    AuthCapability, AuthCapabilitySet, AuthProfileId, CapabilityAnnouncement, CapabilityParseError,
    ClosedCapability, ClosedCapabilitySet, ConnectionId, ConnectionIdSource, EntityId,
    GameplayCapability, GameplayCapabilitySet, GameplayProfileId, PlayerId, PluginBuildTag,
    PluginGenerationId, ProtocolCapability, ProtocolCapabilitySet, RevisionConflict, Revisioned,
    SessionCapabilitySet, SessionRoutes, StorageCapability, StorageCapabilitySet, StorageProfileId,
};
