use mc_plugin_sdk_rust::{StaticPluginManifest, export_protocol_plugin};
use mc_proto_be_placeholder::BePlaceholderAdapter;
use mc_proto_common::{
    BedrockListenerDescriptor, HandshakeProbe, PlayEncodingContext, PlaySyncAdapter,
    ProtocolAdapter, ProtocolDescriptor, ProtocolError, SessionAdapter, StatusRequest, WireCodec,
};

#[derive(Default)]
pub struct BePlaceholderProtocolPlugin {
    adapter: BePlaceholderAdapter,
}

impl HandshakeProbe for BePlaceholderProtocolPlugin {
    fn transport_kind(&self) -> mc_proto_common::TransportKind {
        self.adapter.transport_kind()
    }

    fn adapter_id(&self) -> Option<&'static str> {
        self.adapter.adapter_id()
    }

    fn try_route(
        &self,
        frame: &[u8],
    ) -> Result<Option<mc_proto_common::HandshakeIntent>, ProtocolError> {
        self.adapter.try_route(frame)
    }
}

impl SessionAdapter for BePlaceholderProtocolPlugin {
    fn wire_codec(&self) -> &dyn WireCodec {
        self.adapter.wire_codec()
    }

    fn decode_status(&self, frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        self.adapter.decode_status(frame)
    }

    fn decode_login(&self, frame: &[u8]) -> Result<mc_proto_common::LoginRequest, ProtocolError> {
        self.adapter.decode_login(frame)
    }

    fn encode_status_response(
        &self,
        status: &mc_proto_common::ServerListStatus,
    ) -> Result<Vec<u8>, ProtocolError> {
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

    fn encode_login_success(
        &self,
        player: &mc_core::PlayerSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.adapter.encode_login_success(player)
    }
}

impl PlaySyncAdapter for BePlaceholderProtocolPlugin {
    fn decode_play(
        &self,
        player_id: mc_core::PlayerId,
        frame: &[u8],
    ) -> Result<Option<mc_core::CoreCommand>, ProtocolError> {
        self.adapter.decode_play(player_id, frame)
    }

    fn encode_play_event(
        &self,
        event: &mc_core::CoreEvent,
        context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        self.adapter.encode_play_event(event, context)
    }
}

impl ProtocolAdapter for BePlaceholderProtocolPlugin {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.adapter.descriptor()
    }

    fn bedrock_listener_descriptor(&self) -> Option<BedrockListenerDescriptor> {
        self.adapter.bedrock_listener_descriptor()
    }

    fn capability_set(&self) -> mc_core::CapabilitySet {
        let mut capabilities = mc_core::CapabilitySet::new();
        let _ = capabilities.insert("protocol.be");
        let _ = capabilities.insert("protocol.be.placeholder");
        let _ = capabilities.insert("runtime.reload.protocol");
        if let Some(build_tag) = option_env!("REVY_PLUGIN_BUILD_TAG") {
            let _ = capabilities.insert(format!("build-tag:{build_tag}"));
        }
        capabilities
    }
}

const MANIFEST: StaticPluginManifest =
    StaticPluginManifest::protocol("be-placeholder", "Bedrock Placeholder Protocol Plugin");

export_protocol_plugin!(BePlaceholderProtocolPlugin, MANIFEST);
