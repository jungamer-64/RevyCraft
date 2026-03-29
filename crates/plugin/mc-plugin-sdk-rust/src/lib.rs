#![allow(clippy::multiple_crate_versions)]

use mc_plugin_api::abi::{
    CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, OwnedBuffer, PluginAbiVersion, PluginKind,
    Utf8Slice,
};
use mc_plugin_api::codec::auth::{AuthDescriptor, BedrockAuthResult};
use mc_plugin_api::codec::gameplay::{GameplayDescriptor, GameplaySessionSnapshot};
use mc_plugin_api::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_api::codec::storage::StorageDescriptor;
use mc_plugin_api::manifest::PluginManifestV1;
use mc_proto_common::{HandshakeProbe, ProtocolAdapter, ProtocolError, StorageError};
use revy_voxel_model::WorldMeta;
use std::path::Path;

#[doc(hidden)]
pub mod __macro_support;
pub mod admin_surface;
pub mod auth;
pub mod buffers;
pub mod capabilities;
pub mod gameplay;
mod macros;
pub mod manifest;
pub mod protocol;
pub mod storage;
#[cfg(any(test, feature = "in-process-testing"))]
pub mod test_support;
#[cfg(test)]
mod tests;

pub use mc_plugin_api::{
    AdminSurfaceCapability, AdminSurfaceCapabilitySet, AdminSurfaceProfileId, AuthCapability,
    AuthCapabilitySet, AuthProfileId, CapabilityAnnouncement, ClosedCapabilitySet, ConnectionId,
    CoreCommand, CoreConfig, CoreEvent, EntityId, EventTarget, GameplayCapability,
    GameplayCapabilitySet, GameplayCommand, GameplayProfileId, PlayerId, PlayerSnapshot,
    PluginBuildTag, PluginGenerationId, ProtocolCapability, ProtocolCapabilitySet, RuntimeCommand,
    ServerCore, SessionCommand, StorageCapability, StorageCapabilitySet, StorageProfileId,
    TargetedEvent, WorldSnapshot,
};
