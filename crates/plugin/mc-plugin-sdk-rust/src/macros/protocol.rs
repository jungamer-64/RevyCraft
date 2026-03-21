#[macro_export]
macro_rules! declare_protocol_plugin {
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
            $crate::capabilities::capability_set($capabilities)
        });

        $crate::export_protocol_plugin!(
            $plugin_ty,
            $crate::manifest::StaticPluginManifest::protocol_with_capabilities(
                $plugin_id,
                $display_name,
                $manifest_capabilities,
            )
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

            fn capability_set(&self) -> mc_core::CapabilitySet {
                $capability_body
            }
        }
    };
}

#[macro_export]
macro_rules! export_protocol_plugin {
    ($plugin_ty:ty, $manifest:expr) => {
        static MC_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> = std::sync::OnceLock::new();
        static MC_PLUGIN_MANIFEST: std::sync::OnceLock<mc_plugin_api::manifest::PluginManifestV1> =
            std::sync::OnceLock::new();
        static MC_PLUGIN_API: std::sync::OnceLock<mc_plugin_api::host_api::ProtocolPluginApiV1> =
            std::sync::OnceLock::new();

        fn mc_plugin_instance() -> &'static $plugin_ty {
            MC_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        unsafe extern "C" fn mc_plugin_invoke(
            request: mc_plugin_api::abi::ByteSlice,
            output: *mut mc_plugin_api::abi::OwnedBuffer,
            error_out: *mut mc_plugin_api::abi::OwnedBuffer,
        ) -> mc_plugin_api::abi::PluginErrorCode {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes =
                    unsafe { $crate::__macro_support::buffers::byte_slice_as_bytes(request) };
                mc_plugin_api::codec::protocol::decode_protocol_request(request_bytes)
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        error.to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::InvalidInput;
                }
                Err(_) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        "protocol plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::__macro_support::handle_protocol_request(mc_plugin_instance(), request.clone())
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::__macro_support::buffers::write_error_buffer(error_out, message);
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        "protocol plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::codec::protocol::encode_protocol_response(&request, &response) {
                Ok(bytes) => {
                    $crate::__macro_support::buffers::write_output_buffer(output, bytes);
                    mc_plugin_api::abi::PluginErrorCode::Ok
                }
                Err(message) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        message.to_string(),
                    );
                    mc_plugin_api::abi::PluginErrorCode::Internal
                }
            }
        }

        unsafe extern "C" fn mc_plugin_free_buffer(buffer: mc_plugin_api::abi::OwnedBuffer) {
            unsafe {
                $crate::__macro_support::buffers::free_owned_buffer(buffer);
            }
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::manifest::PluginManifestV1 {
            std::ptr::from_ref(
                MC_PLUGIN_MANIFEST
                    .get_or_init(|| $crate::manifest::manifest_from_static(&$manifest)),
            )
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_protocol_api_v1() -> *const mc_plugin_api::host_api::ProtocolPluginApiV1 {
            std::ptr::from_ref(
                MC_PLUGIN_API.get_or_init(|| mc_plugin_api::host_api::ProtocolPluginApiV1 {
                    invoke: mc_plugin_invoke,
                    free_buffer: mc_plugin_free_buffer,
                }),
            )
        }

        #[cfg(any(test, feature = "in-process-testing"))]
        #[must_use]
        pub fn in_process_protocol_entrypoints() -> $crate::test_support::InProcessProtocolEntrypoints {
            $crate::test_support::InProcessProtocolEntrypoints {
                manifest: unsafe { &*mc_plugin_manifest_v1() },
                api: unsafe { &*mc_plugin_protocol_api_v1() },
            }
        }
    };
}
