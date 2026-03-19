use mc_plugin_api::{
    ByteSlice, CURRENT_PLUGIN_ABI, OwnedBuffer, PluginAbiVersion, PluginManifestV1,
    ProtocolPluginApiV1, ProtocolRequest, ProtocolResponse, ProtocolSessionSnapshot, Utf8Slice,
};
use mc_proto_common::{HandshakeProbe, ProtocolAdapter, ProtocolError};

pub struct StaticPluginManifest {
    pub plugin_id: &'static str,
    pub display_name: &'static str,
    pub plugin_abi: PluginAbiVersion,
    pub min_host_abi: PluginAbiVersion,
    pub max_host_abi: PluginAbiVersion,
}

impl StaticPluginManifest {
    #[must_use]
    pub const fn protocol(plugin_id: &'static str, display_name: &'static str) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
        }
    }
}

#[derive(Clone, Copy)]
pub struct InProcessProtocolEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub api: &'static ProtocolPluginApiV1,
}

pub trait RustProtocolPlugin: HandshakeProbe + ProtocolAdapter + Send + Sync + 'static {
    fn export_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        Ok(Vec::new())
    }

    fn import_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), ProtocolError> {
        Ok(())
    }
}

impl<T> RustProtocolPlugin for T where T: HandshakeProbe + ProtocolAdapter + Send + Sync + 'static {}

#[must_use]
pub fn manifest_from_static(manifest: &StaticPluginManifest) -> PluginManifestV1 {
    PluginManifestV1 {
        plugin_id: Utf8Slice::from_static_str(manifest.plugin_id),
        display_name: Utf8Slice::from_static_str(manifest.display_name),
        plugin_kind: mc_plugin_api::PluginKind::Protocol,
        plugin_abi: manifest.plugin_abi,
        min_host_abi: manifest.min_host_abi,
        max_host_abi: manifest.max_host_abi,
        capabilities: std::ptr::null(),
        capabilities_len: 0,
    }
}

#[must_use]
pub fn into_owned_buffer(mut buffer: Vec<u8>) -> OwnedBuffer {
    let owned = OwnedBuffer {
        ptr: buffer.as_mut_ptr(),
        len: buffer.len(),
        cap: buffer.capacity(),
    };
    std::mem::forget(buffer);
    owned
}

/// # Safety
///
/// `buffer` must have been allocated by [`into_owned_buffer`].
pub unsafe fn free_owned_buffer(buffer: OwnedBuffer) {
    if !buffer.ptr.is_null() {
        // SAFETY: Caller guarantees the buffer originated from `into_owned_buffer`.
        unsafe {
            let _ = Vec::from_raw_parts(buffer.ptr, buffer.len, buffer.cap);
        }
    }
}

#[doc(hidden)]
pub unsafe fn byte_slice_as_bytes(slice: ByteSlice) -> &'static [u8] {
    if slice.ptr.is_null() || slice.len == 0 {
        &[]
    } else {
        // SAFETY: Caller guarantees the pointer and length describe a valid buffer.
        unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) }
    }
}

#[doc(hidden)]
pub fn write_error_buffer(error_out: *mut OwnedBuffer, message: String) {
    if error_out.is_null() {
        return;
    }
    // SAFETY: The host passes a valid mutable pointer for error output.
    unsafe {
        *error_out = into_owned_buffer(message.into_bytes());
    }
}

#[doc(hidden)]
pub fn write_output_buffer(output: *mut OwnedBuffer, bytes: Vec<u8>) {
    if output.is_null() {
        return;
    }
    // SAFETY: The host passes a valid mutable pointer for output.
    unsafe {
        *output = into_owned_buffer(bytes);
    }
}

#[doc(hidden)]
pub fn handle_protocol_request<P: RustProtocolPlugin>(
    plugin: &P,
    request: ProtocolRequest,
) -> Result<ProtocolResponse, String> {
    match request {
        ProtocolRequest::Describe => Ok(ProtocolResponse::Descriptor(plugin.descriptor())),
        ProtocolRequest::CapabilitySet => Ok(ProtocolResponse::CapabilitySet(
            plugin.capability_set(),
        )),
        ProtocolRequest::TryRoute { frame } => plugin
            .try_route(&frame)
            .map(ProtocolResponse::HandshakeIntent)
            .map_err(|error| error.to_string()),
        ProtocolRequest::DecodeStatus { frame } => plugin
            .decode_status(&frame)
            .map(ProtocolResponse::StatusRequest)
            .map_err(|error| error.to_string()),
        ProtocolRequest::DecodeLogin { frame } => plugin
            .decode_login(&frame)
            .map(ProtocolResponse::LoginRequest)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeStatusResponse { status } => plugin
            .encode_status_response(&status)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeStatusPong { payload } => plugin
            .encode_status_pong(payload)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeDisconnect { phase, reason } => plugin
            .encode_disconnect(phase, &reason)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeLoginSuccess { player } => plugin
            .encode_login_success(&player)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::DecodePlay { player_id, frame } => plugin
            .decode_play(player_id, &frame)
            .map(ProtocolResponse::CoreCommand)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodePlayEvent { event, context } => plugin
            .encode_play_event(&event, &context)
            .map(ProtocolResponse::Frames)
            .map_err(|error| error.to_string()),
        ProtocolRequest::ExportSessionState { session } => plugin
            .export_session_state(&session)
            .map(ProtocolResponse::SessionTransferBlob)
            .map_err(|error| error.to_string()),
        ProtocolRequest::ImportSessionState { session, blob } => plugin
            .import_session_state(&session, &blob)
            .map(|()| ProtocolResponse::Empty)
            .map_err(|error| error.to_string()),
    }
}

#[macro_export]
macro_rules! export_protocol_plugin {
    ($plugin_ty:ty, $manifest:expr) => {
        static MC_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> = std::sync::OnceLock::new();
        static MC_PLUGIN_MANIFEST: std::sync::OnceLock<mc_plugin_api::PluginManifestV1> =
            std::sync::OnceLock::new();
        static MC_PLUGIN_API: std::sync::OnceLock<mc_plugin_api::ProtocolPluginApiV1> =
            std::sync::OnceLock::new();

        fn mc_plugin_instance() -> &'static $plugin_ty {
            MC_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        unsafe extern "C" fn mc_plugin_invoke(
            request: mc_plugin_api::ByteSlice,
            output: *mut mc_plugin_api::OwnedBuffer,
            error_out: *mut mc_plugin_api::OwnedBuffer,
        ) -> mc_plugin_api::PluginErrorCode {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes =
                    // SAFETY: Host provides a valid request slice for the duration of this call.
                    unsafe { $crate::byte_slice_as_bytes(request) };
                mc_plugin_api::decode_protocol_request(request_bytes)
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::write_error_buffer(error_out, error.to_string());
                    return mc_plugin_api::PluginErrorCode::InvalidInput;
                }
                Err(_) => {
                    $crate::write_error_buffer(
                        error_out,
                        "protocol plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::handle_protocol_request(mc_plugin_instance(), request.clone())
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::write_error_buffer(error_out, message);
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::write_error_buffer(
                        error_out,
                        "protocol plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::encode_protocol_response(&request, &response) {
                Ok(bytes) => {
                    $crate::write_output_buffer(output, bytes);
                    mc_plugin_api::PluginErrorCode::Ok
                }
                Err(message) => {
                    $crate::write_error_buffer(error_out, message.to_string());
                    mc_plugin_api::PluginErrorCode::Internal
                }
            }
        }

        unsafe extern "C" fn mc_plugin_free_buffer(buffer: mc_plugin_api::OwnedBuffer) {
            // SAFETY: Buffers handed back to the host were allocated by the SDK.
            unsafe {
                $crate::free_owned_buffer(buffer);
            }
        }

        #[cfg_attr(not(feature = "disable-exported-symbols"), unsafe(no_mangle))]
        pub extern "C" fn mc_plugin_manifest_v1() -> *const mc_plugin_api::PluginManifestV1 {
            MC_PLUGIN_MANIFEST
                .get_or_init(|| $crate::manifest_from_static(&$manifest))
                as *const mc_plugin_api::PluginManifestV1
        }

        #[cfg_attr(not(feature = "disable-exported-symbols"), unsafe(no_mangle))]
        pub extern "C" fn mc_plugin_protocol_api_v1() -> *const mc_plugin_api::ProtocolPluginApiV1
        {
            MC_PLUGIN_API.get_or_init(|| mc_plugin_api::ProtocolPluginApiV1 {
                invoke: mc_plugin_invoke,
                free_buffer: mc_plugin_free_buffer,
            }) as *const mc_plugin_api::ProtocolPluginApiV1
        }

        #[must_use]
        pub fn in_process_protocol_entrypoints() -> $crate::InProcessProtocolEntrypoints {
            $crate::InProcessProtocolEntrypoints {
                // SAFETY: The exported functions return pointers to process-global statics.
                manifest: unsafe { &*mc_plugin_manifest_v1() },
                // SAFETY: The exported functions return pointers to process-global statics.
                api: unsafe { &*mc_plugin_protocol_api_v1() },
            }
        }
    };
}
