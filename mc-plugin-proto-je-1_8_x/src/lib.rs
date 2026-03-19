use mc_plugin_sdk_rust::{StaticPluginManifest, export_protocol_plugin};
use mc_proto_common::{
    HandshakeProbe, PlayEncodingContext, PlaySyncAdapter, ProtocolAdapter, ProtocolDescriptor,
    ProtocolError, SessionAdapter, StatusRequest, WireCodec,
};
use mc_proto_je_1_8_x::Je18xAdapter;

#[derive(Default)]
pub struct Je18xProtocolPlugin {
    adapter: Je18xAdapter,
}

impl HandshakeProbe for Je18xProtocolPlugin {
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

impl SessionAdapter for Je18xProtocolPlugin {
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

    fn encode_login_success(
        &self,
        player: &mc_core::PlayerSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.adapter.encode_login_success(player)
    }
}

impl PlaySyncAdapter for Je18xProtocolPlugin {
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

impl ProtocolAdapter for Je18xProtocolPlugin {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.adapter.descriptor()
    }

    fn capability_set(&self) -> mc_core::CapabilitySet {
        let mut capabilities = mc_core::CapabilitySet::new();
        let _ = capabilities.insert("protocol.je");
        let _ = capabilities.insert("protocol.je.1_8_x");
        let _ = capabilities.insert("runtime.reload.protocol");
        if let Some(build_tag) = option_env!("REVY_PLUGIN_BUILD_TAG") {
            let _ = capabilities.insert(format!("build-tag:{build_tag}"));
        }
        capabilities
    }
}

const MANIFEST: StaticPluginManifest =
    StaticPluginManifest::protocol("je-1_8_x", "JE 1.8.x Protocol Plugin");

export_protocol_plugin!(Je18xProtocolPlugin, MANIFEST);
