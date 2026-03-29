pub mod capabilities;
pub mod event;
pub mod ids;
pub mod overlay;
pub mod revision;
pub mod routing;

pub use self::capabilities::{
    AdminSurfaceCapability, AdminSurfaceCapabilitySet, AuthCapability, AuthCapabilitySet,
    CapabilityAnnouncement, CapabilityParseError, ClosedCapability, ClosedCapabilitySet,
    GameplayCapability, GameplayCapabilitySet, ProtocolCapability, ProtocolCapabilitySet,
    SessionCapabilitySet, StorageCapability, StorageCapabilitySet,
};
pub use self::event::{EventTarget, RoutedEvent};
pub use self::ids::{
    AdapterId, AdminSurfaceProfileId, AuthProfileId, ConnectionId, EntityId, GameplayProfileId,
    PlayerId, PluginBuildTag, PluginGenerationId, StorageProfileId,
};
pub use self::revision::{RevisionConflict, Revisioned};
pub use self::routing::{ConnectionIdSource, SessionRoutes};

#[cfg(test)]
mod tests;
