use crate::errors::{ProtocolError, StorageError};
use crate::types::{
    BedrockListenerDescriptor, ConnectionPhase, HandshakeIntent, LoginRequest, PlayEncodingContext,
    ProtocolDescriptor, ServerListStatus, StatusRequest, TransportKind,
};
use bytes::BytesMut;
use mc_core::{
    CapabilitySet, CoreCommand, CoreEvent, PlayerId, PlayerSnapshot, PluginGenerationId,
    WorldSnapshot,
};
use std::path::Path;

pub trait WireCodec: Send + Sync {
    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the payload cannot be framed for the
    /// target wire format.
    fn encode_frame(&self, payload: &[u8]) -> Result<Vec<u8>, ProtocolError>;

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the buffered bytes are malformed for the
    /// wire format. Returns `Ok(None)` when a full frame is not available yet.
    fn try_decode_frame(&self, buffer: &mut BytesMut) -> Result<Option<Vec<u8>>, ProtocolError>;
}

pub trait StorageAdapter: Send + Sync {
    /// # Errors
    ///
    /// Returns [`StorageError`] when the snapshot backend cannot be read or
    /// when persisted data is invalid.
    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError>;

    /// # Errors
    ///
    /// Returns [`StorageError`] when the snapshot cannot be serialized or
    /// written to the backing store.
    fn save_snapshot(&self, world_dir: &Path, snapshot: &WorldSnapshot)
    -> Result<(), StorageError>;
}

pub trait HandshakeProbe: Send + Sync {
    fn transport_kind(&self) -> TransportKind;

    #[must_use]
    fn adapter_id(&self) -> Option<&'static str> {
        None
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the payload matches the probe's protocol
    /// family but is malformed. Returns `Ok(None)` when the payload does not
    /// belong to this probe.
    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError>;
}

pub trait SessionAdapter: Send + Sync {
    fn wire_codec(&self) -> &dyn WireCodec;

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the frame is malformed or unsupported for
    /// the adapter's status phase.
    fn decode_status(&self, frame: &[u8]) -> Result<StatusRequest, ProtocolError>;

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the frame is malformed or unsupported for
    /// the adapter's login phase.
    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError>;

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the status response cannot be encoded for
    /// the adapter's protocol version.
    fn encode_status_response(&self, status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError>;

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the pong packet cannot be encoded for the
    /// adapter's protocol version.
    fn encode_status_pong(&self, payload: i64) -> Result<Vec<u8>, ProtocolError>;

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the disconnect payload cannot be encoded
    /// for the given connection phase.
    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError>;

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the encryption request payload cannot be
    /// encoded for the adapter's protocol version.
    fn encode_encryption_request(
        &self,
        server_id: &str,
        public_key_der: &[u8],
        verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError>;

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the network settings payload cannot be
    /// encoded for the adapter's protocol version.
    fn encode_network_settings(&self, compression_threshold: u16)
    -> Result<Vec<u8>, ProtocolError>;

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the login success payload cannot be
    /// encoded for the adapter's protocol version.
    fn encode_login_success(&self, player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError>;
}

pub trait PlaySyncAdapter: Send + Sync {
    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the frame is malformed or unsupported for
    /// the adapter's play phase.
    fn decode_play(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError>;

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the core event cannot be represented in
    /// the target protocol for the provided play session context.
    fn encode_play_event(
        &self,
        event: &CoreEvent,
        context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError>;
}

pub trait ProtocolAdapter: SessionAdapter + PlaySyncAdapter + Send + Sync {
    fn descriptor(&self) -> ProtocolDescriptor;

    #[must_use]
    fn bedrock_listener_descriptor(&self) -> Option<BedrockListenerDescriptor> {
        None
    }

    #[must_use]
    fn capability_set(&self) -> CapabilitySet {
        CapabilitySet::default()
    }

    #[must_use]
    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        None
    }
}
