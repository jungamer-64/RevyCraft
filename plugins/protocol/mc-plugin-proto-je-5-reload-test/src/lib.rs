#![allow(clippy::multiple_crate_versions)]
use mc_core::{
    CoreCommand, CoreEvent, PlayerId, PlayerSnapshot, ProtocolCapability, ProtocolCapabilitySet,
};
use mc_plugin_api::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_sdk_rust::capabilities::{build_tag_contains, protocol_capabilities};
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use mc_plugin_sdk_rust::protocol::RustProtocolPlugin;
use mc_proto_common::{
    BedrockListenerDescriptor, HandshakeIntent, HandshakeProbe, LoginRequest, PlayEncodingContext,
    PlaySyncAdapter, ProtocolAdapter, ProtocolDescriptor, ProtocolError, ServerListStatus,
    SessionAdapter, StatusRequest, TransportKind, WireCodec,
};
use mc_proto_je_5::{JE_5_ADAPTER_ID, Je5Adapter};

#[derive(Default)]
pub struct Je5ReloadTestProtocolPlugin {
    adapter: Je5Adapter,
}

impl Je5ReloadTestProtocolPlugin {
    fn descriptor_for_build(&self) -> ProtocolDescriptor {
        let mut descriptor = self.adapter.descriptor();
        if option_env!("REVY_PLUGIN_BUILD_TAG")
            .is_some_and(|tag| tag.contains("reload-incompatible"))
        {
            descriptor.protocol_number += 1;
        }
        descriptor
    }
}

impl RustProtocolPlugin for Je5ReloadTestProtocolPlugin {
    fn export_session_state(
        &self,
        session: &ProtocolSessionSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        Ok(session.connection_id.0.to_le_bytes().to_vec())
    }

    fn import_session_state(
        &self,
        session: &ProtocolSessionSnapshot,
        blob: &[u8],
    ) -> Result<(), ProtocolError> {
        if build_tag_contains("reload-fail") {
            return Err(ProtocolError::Plugin(
                "reload test protocol plugin refused session import".to_string(),
            ));
        }
        if blob != session.connection_id.0.to_le_bytes() {
            return Err(ProtocolError::Plugin(
                "reload test protocol plugin received mismatched connection id".to_string(),
            ));
        }
        Ok(())
    }
}

impl HandshakeProbe for Je5ReloadTestProtocolPlugin {
    fn transport_kind(&self) -> TransportKind {
        self.adapter.transport_kind()
    }

    fn adapter_id(&self) -> Option<&'static str> {
        self.adapter.adapter_id()
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        self.adapter.try_route(frame)
    }
}

impl SessionAdapter for Je5ReloadTestProtocolPlugin {
    fn wire_codec(&self) -> &dyn WireCodec {
        self.adapter.wire_codec()
    }

    fn decode_status(&self, frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        self.adapter.decode_status(frame)
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        self.adapter.decode_login(frame)
    }

    fn encode_status_response(&self, status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        self.adapter.encode_status_response(status)
    }

    fn encode_status_pong(&self, payload: i64) -> Result<Vec<u8>, ProtocolError> {
        self.adapter.encode_status_pong(payload)
    }

    fn encode_disconnect(
        &self,
        phase: mc_proto_common::ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.adapter.encode_disconnect(phase, reason)
    }

    fn encode_encryption_request(
        &self,
        server_id: &str,
        public_key_der: &[u8],
        verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        self.adapter
            .encode_encryption_request(server_id, public_key_der, verify_token)
    }

    fn encode_network_settings(
        &self,
        compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.adapter.encode_network_settings(compression_threshold)
    }

    fn encode_login_success(&self, player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
        self.adapter.encode_login_success(player)
    }
}

impl PlaySyncAdapter for Je5ReloadTestProtocolPlugin {
    fn decode_play(
        &self,
        player_id: PlayerId,
        frame: &[u8],
    ) -> Result<Option<CoreCommand>, ProtocolError> {
        self.adapter.decode_play(player_id, frame)
    }

    fn encode_play_event(
        &self,
        event: &CoreEvent,
        context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        self.adapter.encode_play_event(event, context)
    }
}

impl ProtocolAdapter for Je5ReloadTestProtocolPlugin {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.descriptor_for_build()
    }

    fn bedrock_listener_descriptor(&self) -> Option<BedrockListenerDescriptor> {
        self.adapter.bedrock_listener_descriptor()
    }

    fn capability_set(&self) -> ProtocolCapabilitySet {
        protocol_capabilities(&[
            ProtocolCapability::RuntimeReload,
            ProtocolCapability::Je,
            ProtocolCapability::Je5,
        ])
    }
}

const MANIFEST: StaticPluginManifest =
    StaticPluginManifest::protocol(JE_5_ADAPTER_ID, "JE 1.7.10 (Protocol 5) Reload Test Plugin");

export_plugin!(protocol, Je5ReloadTestProtocolPlugin, MANIFEST);
