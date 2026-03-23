#[macro_export]
macro_rules! declare_protocol_plugin {
    (
        $plugin_ty:ident,
        $adapter_ty:ty,
        $plugin_id:expr,
        $display_name:expr,
        $capabilities:expr $(,)?
    ) => {
        $crate::declare_protocol_plugin!(
            $plugin_ty,
            $adapter_ty,
            $plugin_id,
            $display_name,
            $capabilities,
            &[]
        );
    };
    (
        $plugin_ty:ident,
        $adapter_ty:ty,
        $plugin_id:expr,
        $display_name:expr,
        $capabilities:expr,
        $manifest_capabilities:expr $(,)?
    ) => {
        #[derive(Default)]
        pub struct $plugin_ty {
            adapter: $adapter_ty,
        }

        $crate::delegate_protocol_adapter!($plugin_ty, adapter, {
            $crate::capabilities::protocol_capabilities($capabilities)
        });

        $crate::export_plugin!(
            protocol,
            $plugin_ty,
            $crate::manifest::StaticPluginManifest::protocol($plugin_id, $display_name)
        );
    };
}

#[macro_export]
macro_rules! delegate_protocol_adapter {
    ($plugin_ty:ty, $field:ident, $capability_body:block $(,)?) => {
        impl $crate::protocol::RustProtocolPlugin for $plugin_ty {}

        impl mc_proto_common::HandshakeProbe for $plugin_ty {
            fn transport_kind(&self) -> mc_proto_common::TransportKind {
                self.$field.transport_kind()
            }

            fn adapter_id(&self) -> Option<&'static str> {
                self.$field.adapter_id()
            }

            fn try_route(
                &self,
                frame: &[u8],
            ) -> Result<Option<mc_proto_common::HandshakeIntent>, mc_proto_common::ProtocolError>
            {
                self.$field.try_route(frame)
            }
        }

        impl mc_proto_common::SessionAdapter for $plugin_ty {
            fn wire_codec(&self) -> &dyn mc_proto_common::WireCodec {
                self.$field.wire_codec()
            }

            fn decode_status(
                &self,
                frame: &[u8],
            ) -> Result<mc_proto_common::StatusRequest, mc_proto_common::ProtocolError> {
                self.$field.decode_status(frame)
            }

            fn decode_login(
                &self,
                frame: &[u8],
            ) -> Result<mc_proto_common::LoginRequest, mc_proto_common::ProtocolError> {
                self.$field.decode_login(frame)
            }

            fn encode_status_response(
                &self,
                status: &mc_proto_common::ServerListStatus,
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field.encode_status_response(status)
            }

            fn encode_status_pong(
                &self,
                payload: i64,
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field.encode_status_pong(payload)
            }

            fn encode_disconnect(
                &self,
                phase: mc_proto_common::ConnectionPhase,
                reason: &str,
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field.encode_disconnect(phase, reason)
            }

            fn encode_encryption_request(
                &self,
                server_id: &str,
                public_key_der: &[u8],
                verify_token: &[u8],
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field
                    .encode_encryption_request(server_id, public_key_der, verify_token)
            }

            fn encode_network_settings(
                &self,
                compression_threshold: u16,
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field.encode_network_settings(compression_threshold)
            }

            fn encode_login_success(
                &self,
                player: &mc_core::PlayerSnapshot,
            ) -> Result<Vec<u8>, mc_proto_common::ProtocolError> {
                self.$field.encode_login_success(player)
            }
        }

        impl mc_proto_common::PlaySyncAdapter for $plugin_ty {
            fn decode_play(
                &self,
                player_id: mc_core::PlayerId,
                frame: &[u8],
            ) -> Result<Option<mc_core::CoreCommand>, mc_proto_common::ProtocolError> {
                self.$field.decode_play(player_id, frame)
            }

            fn encode_play_event(
                &self,
                event: &mc_core::CoreEvent,
                context: &mc_proto_common::PlayEncodingContext,
            ) -> Result<Vec<Vec<u8>>, mc_proto_common::ProtocolError> {
                self.$field.encode_play_event(event, context)
            }
        }

        impl mc_proto_common::ProtocolAdapter for $plugin_ty {
            fn descriptor(&self) -> mc_proto_common::ProtocolDescriptor {
                self.$field.descriptor()
            }

            fn bedrock_listener_descriptor(
                &self,
            ) -> Option<mc_proto_common::BedrockListenerDescriptor> {
                self.$field.bedrock_listener_descriptor()
            }

            fn capability_set(&self) -> mc_core::ProtocolCapabilitySet {
                $capability_body
            }
        }
    };
}
