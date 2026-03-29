#![allow(clippy::multiple_crate_versions)]

mod errors;
mod packet;
#[cfg(test)]
mod tests;
mod traits;
mod types;
mod wire;

pub mod semantic {
    pub use revy_voxel_core::{
        ConnectionId, CoreCommand, CoreConfig, CoreEvent, EntityId, PlayerId, PlayerSnapshot,
        PluginGenerationId, ProtocolCapability, ProtocolCapabilitySet, RuntimeCommand,
        ServerCore, SessionCommand, WorldSnapshot,
    };
}

pub use self::errors::{ProtocolError, StorageError};
pub use self::packet::{PacketReader, PacketWriter};
pub use self::semantic::{
    ConnectionId, CoreCommand, CoreConfig, CoreEvent, EntityId, PlayerId, PlayerSnapshot,
    PluginGenerationId, ProtocolCapability, ProtocolCapabilitySet, RuntimeCommand, ServerCore,
    SessionCommand, WorldSnapshot,
};
pub use self::traits::{
    HandshakeProbe, PlaySyncAdapter, ProtocolAdapter, SessionAdapter, StorageAdapter, WireCodec,
};
pub use self::types::{
    BedrockListenerDescriptor, ConnectionPhase, Edition, HandshakeIntent, HandshakeNextState,
    LoginRequest, PlayEncodingContext, ProtocolDescriptor, ProtocolSessionSnapshot,
    ServerListStatus, StatusRequest, TransportKind, WireFormatKind,
};
pub use self::wire::{MinecraftWireCodec, RawPacketStreamWireCodec};
