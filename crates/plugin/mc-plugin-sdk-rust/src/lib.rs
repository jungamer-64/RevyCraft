#![allow(clippy::multiple_crate_versions)]
use bytes::BytesMut;
use mc_core::{
    CapabilitySet, CoreCommand, GameplayEffect, GameplayJoinEffect, PlayerId, PlayerSnapshot,
    WorldMeta, WorldSnapshot,
};
use mc_plugin_api::abi::{
    ByteSlice, CURRENT_PLUGIN_ABI, CapabilityDescriptorV1, OwnedBuffer, PluginAbiVersion,
    PluginErrorCode, PluginKind, Utf8Slice,
};
use mc_plugin_api::codec::auth::{AuthDescriptor, AuthRequest, AuthResponse, BedrockAuthResult};
use mc_plugin_api::codec::gameplay::{
    GameplayDescriptor, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
    decode_host_block_state_blob, decode_host_player_snapshot_blob, decode_host_world_meta_blob,
    encode_host_block_pos_blob, encode_host_can_edit_block_key, encode_host_player_id_blob,
};
use mc_plugin_api::codec::protocol::{
    ProtocolRequest, ProtocolResponse, ProtocolSessionSnapshot, WireFrameDecodeResult,
};
use mc_plugin_api::codec::storage::{StorageDescriptor, StorageRequest, StorageResponse};
use mc_plugin_api::host_api::{
    AuthPluginApiV1, GameplayPluginApiV1, HostApiTableV1, ProtocolPluginApiV1, StoragePluginApiV1,
};
use mc_plugin_api::manifest::PluginManifestV1;
use mc_proto_common::{HandshakeProbe, ProtocolAdapter, ProtocolError, StorageError};
use std::path::Path;
pub mod auth;
pub mod buffers;
pub mod entrypoints;
pub mod gameplay;
pub mod manifest;
pub mod protocol;
pub mod storage;
pub mod test_support;

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
                let request_bytes = unsafe { $crate::buffers::byte_slice_as_bytes(request) };
                mc_plugin_api::codec::protocol::decode_protocol_request(request_bytes)
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::buffers::write_error_buffer(error_out, error.to_string());
                    return mc_plugin_api::abi::PluginErrorCode::InvalidInput;
                }
                Err(_) => {
                    $crate::buffers::write_error_buffer(
                        error_out,
                        "protocol plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::protocol::handle_protocol_request(mc_plugin_instance(), request.clone())
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::buffers::write_error_buffer(error_out, message);
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::buffers::write_error_buffer(
                        error_out,
                        "protocol plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::codec::protocol::encode_protocol_response(&request, &response) {
                Ok(bytes) => {
                    $crate::buffers::write_output_buffer(output, bytes);
                    mc_plugin_api::abi::PluginErrorCode::Ok
                }
                Err(message) => {
                    $crate::buffers::write_error_buffer(error_out, message.to_string());
                    mc_plugin_api::abi::PluginErrorCode::Internal
                }
            }
        }

        unsafe extern "C" fn mc_plugin_free_buffer(buffer: mc_plugin_api::abi::OwnedBuffer) {
            unsafe {
                $crate::buffers::free_owned_buffer(buffer);
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

        #[must_use]
        pub fn in_process_protocol_entrypoints() -> $crate::test_support::InProcessProtocolEntrypoints {
            $crate::test_support::InProcessProtocolEntrypoints {
                manifest: unsafe { &*mc_plugin_manifest_v1() },
                api: unsafe { &*mc_plugin_protocol_api_v1() },
            }
        }
    };
}

#[macro_export]
macro_rules! export_gameplay_plugin {
    ($plugin_ty:ty, $manifest:expr) => {
        static MC_GAMEPLAY_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> =
            std::sync::OnceLock::new();
        static MC_GAMEPLAY_PLUGIN_MANIFEST: std::sync::OnceLock<mc_plugin_api::manifest::PluginManifestV1> =
            std::sync::OnceLock::new();
        static MC_GAMEPLAY_PLUGIN_API: std::sync::OnceLock<mc_plugin_api::host_api::GameplayPluginApiV1> =
            std::sync::OnceLock::new();
        static MC_GAMEPLAY_HOST_API_SLOT: std::sync::OnceLock<
            std::sync::Mutex<Option<mc_plugin_api::host_api::HostApiTableV1>>,
        > = std::sync::OnceLock::new();

        fn mc_gameplay_plugin_instance() -> &'static $plugin_ty {
            MC_GAMEPLAY_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        fn mc_gameplay_host_api_slot()
        -> &'static std::sync::Mutex<Option<mc_plugin_api::host_api::HostApiTableV1>> {
            MC_GAMEPLAY_HOST_API_SLOT.get_or_init(|| std::sync::Mutex::new(None))
        }

        unsafe extern "C" fn mc_gameplay_plugin_set_host_api(
            host_api: *const mc_plugin_api::host_api::HostApiTableV1,
        ) -> mc_plugin_api::abi::PluginErrorCode {
            let Some(host_api) = (unsafe { host_api.as_ref() }) else {
                return mc_plugin_api::abi::PluginErrorCode::InvalidInput;
            };
            let mut guard = mc_gameplay_host_api_slot()
                .lock()
                .expect("gameplay host api mutex should not be poisoned");
            *guard = Some(*host_api);
            mc_plugin_api::abi::PluginErrorCode::Ok
        }

        unsafe extern "C" fn mc_gameplay_plugin_invoke(
            request: mc_plugin_api::abi::ByteSlice,
            output: *mut mc_plugin_api::abi::OwnedBuffer,
            error_out: *mut mc_plugin_api::abi::OwnedBuffer,
        ) -> mc_plugin_api::abi::PluginErrorCode {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes = unsafe { $crate::buffers::byte_slice_as_bytes(request) };
                mc_plugin_api::codec::gameplay::decode_gameplay_request(request_bytes)
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::buffers::write_error_buffer(error_out, error.to_string());
                    return mc_plugin_api::abi::PluginErrorCode::InvalidInput;
                }
                Err(_) => {
                    $crate::buffers::write_error_buffer(
                        error_out,
                        "gameplay plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let host_api = {
                    let guard = mc_gameplay_host_api_slot()
                        .lock()
                        .expect("gameplay host api mutex should not be poisoned");
                    *guard
                };
                $crate::gameplay::handle_gameplay_request_with_host_api(
                    mc_gameplay_plugin_instance(),
                    request.clone(),
                    host_api,
                )
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::buffers::write_error_buffer(error_out, message);
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::buffers::write_error_buffer(
                        error_out,
                        "gameplay plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::codec::gameplay::encode_gameplay_response(&request, &response) {
                Ok(bytes) => {
                    $crate::buffers::write_output_buffer(output, bytes);
                    mc_plugin_api::abi::PluginErrorCode::Ok
                }
                Err(message) => {
                    $crate::buffers::write_error_buffer(error_out, message.to_string());
                    mc_plugin_api::abi::PluginErrorCode::Internal
                }
            }
        }

        unsafe extern "C" fn mc_gameplay_plugin_free_buffer(buffer: mc_plugin_api::abi::OwnedBuffer) {
            unsafe {
                $crate::buffers::free_owned_buffer(buffer);
            }
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::manifest::PluginManifestV1 {
            std::ptr::from_ref(
                MC_GAMEPLAY_PLUGIN_MANIFEST
                    .get_or_init(|| $crate::manifest::manifest_from_static(&$manifest)),
            )
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_gameplay_api_v1() -> *const mc_plugin_api::host_api::GameplayPluginApiV1 {
            std::ptr::from_ref(MC_GAMEPLAY_PLUGIN_API.get_or_init(|| {
                mc_plugin_api::host_api::GameplayPluginApiV1 {
                    set_host_api: mc_gameplay_plugin_set_host_api,
                    invoke: mc_gameplay_plugin_invoke,
                    free_buffer: mc_gameplay_plugin_free_buffer,
                }
            }))
        }

        #[must_use]
        pub fn in_process_gameplay_entrypoints() -> $crate::test_support::InProcessGameplayEntrypoints {
            $crate::test_support::InProcessGameplayEntrypoints {
                manifest: unsafe { &*mc_plugin_manifest_v1() },
                api: unsafe { &*mc_plugin_gameplay_api_v1() },
            }
        }
    };
}

#[macro_export]
macro_rules! export_storage_plugin {
    ($plugin_ty:ty, $manifest:expr) => {
        static MC_STORAGE_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> =
            std::sync::OnceLock::new();
        static MC_STORAGE_PLUGIN_MANIFEST: std::sync::OnceLock<mc_plugin_api::manifest::PluginManifestV1> =
            std::sync::OnceLock::new();
        static MC_STORAGE_PLUGIN_API: std::sync::OnceLock<mc_plugin_api::host_api::StoragePluginApiV1> =
            std::sync::OnceLock::new();

        fn mc_storage_plugin_instance() -> &'static $plugin_ty {
            MC_STORAGE_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        unsafe extern "C" fn mc_storage_plugin_invoke(
            request: mc_plugin_api::abi::ByteSlice,
            output: *mut mc_plugin_api::abi::OwnedBuffer,
            error_out: *mut mc_plugin_api::abi::OwnedBuffer,
        ) -> mc_plugin_api::abi::PluginErrorCode {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes = unsafe { $crate::buffers::byte_slice_as_bytes(request) };
                mc_plugin_api::codec::storage::decode_storage_request(request_bytes)
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::buffers::write_error_buffer(error_out, error.to_string());
                    return mc_plugin_api::abi::PluginErrorCode::InvalidInput;
                }
                Err(_) => {
                    $crate::buffers::write_error_buffer(
                        error_out,
                        "storage plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::storage::handle_storage_request(mc_storage_plugin_instance(), request.clone())
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::buffers::write_error_buffer(error_out, message);
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::buffers::write_error_buffer(
                        error_out,
                        "storage plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::codec::storage::encode_storage_response(&request, &response) {
                Ok(bytes) => {
                    $crate::buffers::write_output_buffer(output, bytes);
                    mc_plugin_api::abi::PluginErrorCode::Ok
                }
                Err(message) => {
                    $crate::buffers::write_error_buffer(error_out, message.to_string());
                    mc_plugin_api::abi::PluginErrorCode::Internal
                }
            }
        }

        unsafe extern "C" fn mc_storage_plugin_free_buffer(buffer: mc_plugin_api::abi::OwnedBuffer) {
            unsafe {
                $crate::buffers::free_owned_buffer(buffer);
            }
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::manifest::PluginManifestV1 {
            std::ptr::from_ref(
                MC_STORAGE_PLUGIN_MANIFEST
                    .get_or_init(|| $crate::manifest::manifest_from_static(&$manifest)),
            )
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_storage_api_v1() -> *const mc_plugin_api::host_api::StoragePluginApiV1 {
            std::ptr::from_ref(MC_STORAGE_PLUGIN_API.get_or_init(|| {
                mc_plugin_api::host_api::StoragePluginApiV1 {
                    invoke: mc_storage_plugin_invoke,
                    free_buffer: mc_storage_plugin_free_buffer,
                }
            }))
        }

        #[must_use]
        pub fn in_process_storage_entrypoints() -> $crate::test_support::InProcessStorageEntrypoints {
            $crate::test_support::InProcessStorageEntrypoints {
                manifest: unsafe { &*mc_plugin_manifest_v1() },
                api: unsafe { &*mc_plugin_storage_api_v1() },
            }
        }
    };
}

#[macro_export]
macro_rules! export_auth_plugin {
    ($plugin_ty:ty, $manifest:expr) => {
        static MC_AUTH_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> =
            std::sync::OnceLock::new();
        static MC_AUTH_PLUGIN_MANIFEST: std::sync::OnceLock<mc_plugin_api::manifest::PluginManifestV1> =
            std::sync::OnceLock::new();
        static MC_AUTH_PLUGIN_API: std::sync::OnceLock<mc_plugin_api::host_api::AuthPluginApiV1> =
            std::sync::OnceLock::new();

        fn mc_auth_plugin_instance() -> &'static $plugin_ty {
            MC_AUTH_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        unsafe extern "C" fn mc_auth_plugin_invoke(
            request: mc_plugin_api::abi::ByteSlice,
            output: *mut mc_plugin_api::abi::OwnedBuffer,
            error_out: *mut mc_plugin_api::abi::OwnedBuffer,
        ) -> mc_plugin_api::abi::PluginErrorCode {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes = unsafe { $crate::buffers::byte_slice_as_bytes(request) };
                mc_plugin_api::codec::auth::decode_auth_request(request_bytes)
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::buffers::write_error_buffer(error_out, error.to_string());
                    return mc_plugin_api::abi::PluginErrorCode::InvalidInput;
                }
                Err(_) => {
                    $crate::buffers::write_error_buffer(
                        error_out,
                        "auth plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::auth::handle_auth_request(mc_auth_plugin_instance(), request.clone())
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::buffers::write_error_buffer(error_out, message);
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::buffers::write_error_buffer(
                        error_out,
                        "auth plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::codec::auth::encode_auth_response(&request, &response) {
                Ok(bytes) => {
                    $crate::buffers::write_output_buffer(output, bytes);
                    mc_plugin_api::abi::PluginErrorCode::Ok
                }
                Err(message) => {
                    $crate::buffers::write_error_buffer(error_out, message.to_string());
                    mc_plugin_api::abi::PluginErrorCode::Internal
                }
            }
        }

        unsafe extern "C" fn mc_auth_plugin_free_buffer(buffer: mc_plugin_api::abi::OwnedBuffer) {
            unsafe {
                $crate::buffers::free_owned_buffer(buffer);
            }
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::manifest::PluginManifestV1 {
            std::ptr::from_ref(
                MC_AUTH_PLUGIN_MANIFEST
                    .get_or_init(|| $crate::manifest::manifest_from_static(&$manifest)),
            )
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_auth_api_v1() -> *const mc_plugin_api::host_api::AuthPluginApiV1 {
            std::ptr::from_ref(
                MC_AUTH_PLUGIN_API.get_or_init(|| mc_plugin_api::host_api::AuthPluginApiV1 {
                    invoke: mc_auth_plugin_invoke,
                    free_buffer: mc_auth_plugin_free_buffer,
                }),
            )
        }

        #[must_use]
        pub fn in_process_auth_entrypoints() -> $crate::test_support::InProcessAuthEntrypoints {
            $crate::test_support::InProcessAuthEntrypoints {
                manifest: unsafe { &*mc_plugin_manifest_v1() },
                api: unsafe { &*mc_plugin_auth_api_v1() },
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{
        CapabilitySet, GameplayRequest, GameplayResponse, GameplaySessionSnapshot, HostApiTableV1,
        OwnedBuffer, gameplay, manifest, protocol,
    };
    use bytes::BytesMut;
    use mc_core::{CoreCommand, CoreEvent, PlayerId, PlayerSnapshot};
    use mc_core::{GameplayEffect, GameplayProfileId, WorldMeta};
    use mc_plugin_api::abi::{ByteSlice, CURRENT_PLUGIN_ABI, PluginErrorCode};
    use mc_plugin_api::codec::gameplay::{
        GameplayDescriptor, decode_gameplay_response, encode_gameplay_request,
        encode_host_world_meta_blob,
    };
    use mc_plugin_api::codec::protocol::{
        ProtocolRequest, ProtocolResponse, WireFrameDecodeResult,
    };
    use mc_proto_common::{
        ConnectionPhase, Edition, HandshakeIntent, HandshakeProbe, LoginRequest,
        PlayEncodingContext, ProtocolAdapter, ProtocolDescriptor, ProtocolError, ServerListStatus,
        SessionAdapter, StatusRequest, TransportKind, WireCodec, WireFormatKind,
    };
    use std::ffi::c_void;
    use std::sync::{Mutex, OnceLock};

    #[repr(C)]
    struct TestHostContext {
        level_name: &'static str,
    }

    fn write_test_buffer(output: *mut OwnedBuffer, mut bytes: Vec<u8>) {
        if output.is_null() {
            return;
        }
        unsafe {
            *output = OwnedBuffer {
                ptr: bytes.as_mut_ptr(),
                len: bytes.len(),
                cap: bytes.capacity(),
            };
        }
        std::mem::forget(bytes);
    }

    unsafe extern "C" fn host_read_world_meta(
        context: *mut c_void,
        output: *mut OwnedBuffer,
        error_out: *mut OwnedBuffer,
    ) -> PluginErrorCode {
        let Some(context) = (unsafe { (context as *const TestHostContext).as_ref() }) else {
            write_test_buffer(error_out, b"missing host context".to_vec());
            return PluginErrorCode::InvalidInput;
        };
        let bytes = encode_host_world_meta_blob(&WorldMeta {
            level_name: context.level_name.to_string(),
            seed: 0,
            spawn: mc_core::BlockPos::new(0, 64, 0),
            dimension: mc_core::DimensionId::Overworld,
            age: 0,
            time: 0,
            level_type: "FLAT".to_string(),
            game_mode: 0,
            difficulty: 1,
            max_players: 20,
        })
        .expect("test world meta should encode");
        write_test_buffer(output, bytes);
        PluginErrorCode::Ok
    }

    fn host_api_for(context: &TestHostContext) -> HostApiTableV1 {
        HostApiTableV1 {
            abi: CURRENT_PLUGIN_ABI,
            context: std::ptr::from_ref(context).cast_mut().cast(),
            log: None,
            read_player_snapshot: None,
            read_world_meta: Some(host_read_world_meta),
            read_block_state: None,
            can_edit_block: None,
        }
    }

    #[derive(Default)]
    struct DirectProbePlugin;

    impl gameplay::RustGameplayPlugin for DirectProbePlugin {
        fn descriptor(&self) -> GameplayDescriptor {
            GameplayDescriptor {
                profile: GameplayProfileId::new("probe"),
            }
        }

        fn handle_tick(
            &self,
            host: &dyn gameplay::GameplayHost,
            _session: &GameplaySessionSnapshot,
            _now_ms: u64,
        ) -> Result<GameplayEffect, String> {
            let world_meta = host.read_world_meta()?;
            if world_meta.level_name.is_empty() {
                return Err("world meta should not be empty".to_string());
            }
            Ok(GameplayEffect::default())
        }
    }

    struct TestProtocolWireCodec;

    impl WireCodec for TestProtocolWireCodec {
        fn encode_frame(&self, payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
            let length = u8::try_from(payload.len())
                .map_err(|_| ProtocolError::InvalidPacket("test frame too large"))?;
            let mut frame = vec![length];
            frame.extend_from_slice(payload);
            Ok(frame)
        }

        fn try_decode_frame(
            &self,
            buffer: &mut BytesMut,
        ) -> Result<Option<Vec<u8>>, ProtocolError> {
            let Some(length) = buffer.first().copied() else {
                return Ok(None);
            };
            let frame_len = 1 + usize::from(length);
            if buffer.len() < frame_len {
                return Ok(None);
            }
            let frame = buffer[1..frame_len].to_vec();
            let _ = buffer.split_to(frame_len);
            Ok(Some(frame))
        }
    }

    #[derive(Default)]
    struct DirectProtocolPlugin;

    impl HandshakeProbe for DirectProtocolPlugin {
        fn transport_kind(&self) -> TransportKind {
            TransportKind::Tcp
        }

        fn try_route(&self, _frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
            Ok(None)
        }
    }

    impl SessionAdapter for DirectProtocolPlugin {
        fn wire_codec(&self) -> &dyn WireCodec {
            static CODEC: TestProtocolWireCodec = TestProtocolWireCodec;
            &CODEC
        }

        fn decode_status(&self, _frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
            Err(ProtocolError::InvalidPacket("unused test protocol method"))
        }

        fn decode_login(&self, _frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
            Err(ProtocolError::InvalidPacket("unused test protocol method"))
        }

        fn encode_status_response(
            &self,
            _status: &ServerListStatus,
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket("unused test protocol method"))
        }

        fn encode_status_pong(&self, _payload: i64) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket("unused test protocol method"))
        }

        fn encode_disconnect(
            &self,
            _phase: ConnectionPhase,
            _reason: &str,
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket("unused test protocol method"))
        }

        fn encode_encryption_request(
            &self,
            _server_id: &str,
            _public_key_der: &[u8],
            _verify_token: &[u8],
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket("unused test protocol method"))
        }

        fn encode_network_settings(
            &self,
            _compression_threshold: u16,
        ) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket("unused test protocol method"))
        }

        fn encode_login_success(&self, _player: &PlayerSnapshot) -> Result<Vec<u8>, ProtocolError> {
            Err(ProtocolError::InvalidPacket("unused test protocol method"))
        }
    }

    impl mc_proto_common::PlaySyncAdapter for DirectProtocolPlugin {
        fn decode_play(
            &self,
            _player_id: PlayerId,
            _frame: &[u8],
        ) -> Result<Option<CoreCommand>, ProtocolError> {
            Err(ProtocolError::InvalidPacket("unused test protocol method"))
        }

        fn encode_play_event(
            &self,
            _event: &CoreEvent,
            _context: &PlayEncodingContext,
        ) -> Result<Vec<Vec<u8>>, ProtocolError> {
            Err(ProtocolError::InvalidPacket("unused test protocol method"))
        }
    }

    impl ProtocolAdapter for DirectProtocolPlugin {
        fn descriptor(&self) -> ProtocolDescriptor {
            ProtocolDescriptor {
                adapter_id: "direct-probe".to_string(),
                transport: TransportKind::Tcp,
                wire_format: WireFormatKind::MinecraftFramed,
                edition: Edition::Je,
                version_name: "test".to_string(),
                protocol_number: 0,
            }
        }
    }

    impl protocol::RustProtocolPlugin for DirectProtocolPlugin {}

    #[test]
    fn direct_protocol_requests_route_wire_codec_ops_through_plugin_codec() {
        assert_eq!(
            protocol::handle_protocol_request(
                &DirectProtocolPlugin,
                ProtocolRequest::EncodeWireFrame {
                    payload: vec![0xaa, 0xbb, 0xcc],
                },
            )
            .expect("wire frame should encode"),
            ProtocolResponse::Frame(vec![3, 0xaa, 0xbb, 0xcc])
        );

        assert_eq!(
            protocol::handle_protocol_request(
                &DirectProtocolPlugin,
                ProtocolRequest::TryDecodeWireFrame {
                    buffer: vec![3, 0xaa, 0xbb, 0xcc, 0xff],
                },
            )
            .expect("wire frame should decode"),
            ProtocolResponse::WireFrameDecodeResult(Some(WireFrameDecodeResult {
                frame: vec![0xaa, 0xbb, 0xcc],
                bytes_consumed: 4,
            }))
        );

        assert_eq!(
            protocol::handle_protocol_request(
                &DirectProtocolPlugin,
                ProtocolRequest::TryDecodeWireFrame {
                    buffer: vec![3, 0xaa],
                },
            )
            .expect("incomplete frame should stay buffered"),
            ProtocolResponse::WireFrameDecodeResult(None)
        );
    }

    #[test]
    fn direct_gameplay_requests_require_host_api_for_host_callbacks() {
        let request = GameplayRequest::HandleTick {
            session: GameplaySessionSnapshot {
                phase: ConnectionPhase::Play,
                player_id: None,
                entity_id: None,
                gameplay_profile: GameplayProfileId::new("probe"),
            },
            now_ms: 0,
        };
        let error =
            gameplay::handle_gameplay_request_with_host_api(&DirectProbePlugin, request, None)
                .expect_err("host callbacks should require configured host api");
        assert!(error.contains("gameplay host api is not configured"));
    }

    #[allow(unexpected_cfgs)]
    mod plugin_a {
        use super::*;

        #[derive(Default)]
        pub struct PluginA;

        fn recorded_slot() -> &'static Mutex<Option<String>> {
            static RECORDED: OnceLock<Mutex<Option<String>>> = OnceLock::new();
            RECORDED.get_or_init(|| Mutex::new(None))
        }

        pub fn take_recorded_level_name() -> Option<String> {
            recorded_slot()
                .lock()
                .expect("recorded level name mutex should not be poisoned")
                .take()
        }

        impl gameplay::RustGameplayPlugin for PluginA {
            fn descriptor(&self) -> GameplayDescriptor {
                GameplayDescriptor {
                    profile: GameplayProfileId::new("plugin-a"),
                }
            }

            fn capability_set(&self) -> CapabilitySet {
                let mut capabilities = CapabilitySet::new();
                let _ = capabilities.insert("runtime.reload.gameplay");
                capabilities
            }

            fn handle_tick(
                &self,
                host: &dyn gameplay::GameplayHost,
                _session: &GameplaySessionSnapshot,
                _now_ms: u64,
            ) -> Result<GameplayEffect, String> {
                *recorded_slot()
                    .lock()
                    .expect("recorded level name mutex should not be poisoned") =
                    Some(host.read_world_meta()?.level_name);
                Ok(GameplayEffect::default())
            }
        }

        const MANIFEST: manifest::StaticPluginManifest = manifest::StaticPluginManifest::gameplay(
            "plugin-a",
            "Plugin A",
            &["runtime.reload.gameplay"],
        );

        export_gameplay_plugin!(PluginA, MANIFEST);
    }

    #[allow(unexpected_cfgs)]
    mod plugin_b {
        use super::*;

        #[derive(Default)]
        pub struct PluginB;

        fn recorded_slot() -> &'static Mutex<Option<String>> {
            static RECORDED: OnceLock<Mutex<Option<String>>> = OnceLock::new();
            RECORDED.get_or_init(|| Mutex::new(None))
        }

        pub fn take_recorded_level_name() -> Option<String> {
            recorded_slot()
                .lock()
                .expect("recorded level name mutex should not be poisoned")
                .take()
        }

        impl gameplay::RustGameplayPlugin for PluginB {
            fn descriptor(&self) -> GameplayDescriptor {
                GameplayDescriptor {
                    profile: GameplayProfileId::new("plugin-b"),
                }
            }

            fn capability_set(&self) -> CapabilitySet {
                let mut capabilities = CapabilitySet::new();
                let _ = capabilities.insert("runtime.reload.gameplay");
                capabilities
            }

            fn handle_tick(
                &self,
                host: &dyn gameplay::GameplayHost,
                _session: &GameplaySessionSnapshot,
                _now_ms: u64,
            ) -> Result<GameplayEffect, String> {
                *recorded_slot()
                    .lock()
                    .expect("recorded level name mutex should not be poisoned") =
                    Some(host.read_world_meta()?.level_name);
                Ok(GameplayEffect::default())
            }
        }

        const MANIFEST: manifest::StaticPluginManifest = manifest::StaticPluginManifest::gameplay(
            "plugin-b",
            "Plugin B",
            &["runtime.reload.gameplay"],
        );

        export_gameplay_plugin!(PluginB, MANIFEST);
    }

    unsafe fn invoke_gameplay(
        api: &mc_plugin_api::host_api::GameplayPluginApiV1,
        request: &GameplayRequest,
    ) -> GameplayResponse {
        let payload = encode_gameplay_request(request).expect("gameplay request should encode");
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            (api.invoke)(
                ByteSlice {
                    ptr: payload.as_ptr(),
                    len: payload.len(),
                },
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            let message = if error.ptr.is_null() {
                format!("invoke failed with status {status:?}")
            } else {
                let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
                unsafe {
                    (api.free_buffer)(error);
                }
                String::from_utf8(bytes).expect("plugin error should be utf-8")
            };
            panic!("{message}");
        }
        let bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
        unsafe {
            (api.free_buffer)(output);
        }
        decode_gameplay_response(request, &bytes).expect("gameplay response should decode")
    }

    #[test]
    fn exported_gameplay_plugins_keep_host_api_slots_isolated() {
        let context_a = TestHostContext {
            level_name: "host-a",
        };
        let context_b = TestHostContext {
            level_name: "host-b",
        };
        let host_api_a = host_api_for(&context_a);
        let host_api_b = host_api_for(&context_b);

        let entrypoints_a = plugin_a::in_process_gameplay_entrypoints();
        let entrypoints_b = plugin_b::in_process_gameplay_entrypoints();

        assert_eq!(
            unsafe { (entrypoints_a.api.set_host_api)(&raw const host_api_a) },
            PluginErrorCode::Ok
        );
        assert_eq!(
            unsafe { (entrypoints_b.api.set_host_api)(&raw const host_api_b) },
            PluginErrorCode::Ok
        );

        let request_a = GameplayRequest::HandleTick {
            session: GameplaySessionSnapshot {
                phase: ConnectionPhase::Play,
                player_id: None,
                entity_id: None,
                gameplay_profile: GameplayProfileId::new("plugin-a"),
            },
            now_ms: 1,
        };
        let request_b = GameplayRequest::HandleTick {
            session: GameplaySessionSnapshot {
                phase: ConnectionPhase::Play,
                player_id: None,
                entity_id: None,
                gameplay_profile: GameplayProfileId::new("plugin-b"),
            },
            now_ms: 2,
        };

        assert_eq!(
            unsafe { invoke_gameplay(entrypoints_a.api, &request_a) },
            GameplayResponse::Effect(GameplayEffect::default())
        );
        assert_eq!(
            unsafe { invoke_gameplay(entrypoints_b.api, &request_b) },
            GameplayResponse::Effect(GameplayEffect::default())
        );
        assert_eq!(
            plugin_a::take_recorded_level_name().as_deref(),
            Some("host-a")
        );
        assert_eq!(
            plugin_b::take_recorded_level_name().as_deref(),
            Some("host-b")
        );
    }
}
