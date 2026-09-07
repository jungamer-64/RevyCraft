#[doc(hidden)]
#[macro_export]
macro_rules! __export_plugin_non_gameplay {
    (
        $plugin_ty:ty,
        $manifest:expr,
        $api_ty:ty,
        $api_init:expr,
        $decode:path,
        $handle:path,
        $encode:path,
        $api_symbol:ident,
        $panic_decode:literal,
        $panic_handle:literal $(,)?
    ) => {
        static MC_PLUGIN_MANIFEST: std::sync::OnceLock<$crate::manifest::ExportedPluginManifest> =
            std::sync::OnceLock::new();
        static MC_PLUGIN_API: std::sync::OnceLock<$api_ty> = std::sync::OnceLock::new();

        unsafe extern "C" fn mc_plugin_invoke(
            request: $crate::__macro_support::ByteSlice,
            output: *mut $crate::__macro_support::OwnedBuffer,
            error_out: *mut $crate::__macro_support::OwnedBuffer,
        ) -> $crate::__macro_support::PluginStatus {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes =
                    unsafe { $crate::__macro_support::buffers::byte_slice_as_bytes(request) }?;
                $decode(request_bytes).map_err(|error| error.to_string())
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        error.to_string(),
                    );
                    return $crate::__macro_support::PluginStatus::INVALID_INPUT;
                }
                Err(_) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        $panic_decode.to_string(),
                    );
                    return $crate::__macro_support::PluginStatus::INTERNAL;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $handle(mc_plugin_instance(), request.clone())
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::__macro_support::buffers::write_error_buffer(error_out, message);
                    return $crate::__macro_support::PluginStatus::INTERNAL;
                }
                Err(_) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        $panic_handle.to_string(),
                    );
                    return $crate::__macro_support::PluginStatus::INTERNAL;
                }
            };

            match $encode(&request, &response) {
                Ok(bytes) => {
                    $crate::__macro_support::buffers::write_output_buffer(output, bytes);
                    $crate::__macro_support::PluginStatus::OK
                }
                Err(message) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        message.to_string(),
                    );
                    $crate::__macro_support::PluginStatus::INTERNAL
                }
            }
        }

        unsafe extern "C" fn mc_plugin_free_buffer(
            buffer: $crate::__macro_support::OwnedBuffer,
        ) {
            unsafe { $crate::__macro_support::buffers::free_owned_buffer(buffer) };
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn mc_plugin_manifest_v9(
        ) -> *const $crate::__macro_support::PluginManifestV9 {
            std::ptr::from_ref(
                MC_PLUGIN_MANIFEST
                    .get_or_init(|| $crate::manifest::manifest_from_static(&$manifest))
                    .manifest(),
            )
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn $api_symbol() -> *const $api_ty {
            std::ptr::from_ref(MC_PLUGIN_API.get_or_init(|| $api_init))
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __export_plugin_gameplay {
    ($plugin_ty:ty, $manifest:expr $(,)?) => {
        static MC_GAMEPLAY_PLUGIN_MANIFEST: std::sync::OnceLock<$crate::manifest::ExportedPluginManifest> =
            std::sync::OnceLock::new();
        static MC_GAMEPLAY_PLUGIN_API: std::sync::OnceLock<$crate::__macro_support::GameplayPluginApiV9> =
            std::sync::OnceLock::new();

        unsafe extern "C" fn mc_gameplay_plugin_invoke_v9(
            request: $crate::__macro_support::ByteSlice,
            host_api: *const $crate::__macro_support::GameplayHostApiV9,
            output: *mut $crate::__macro_support::OwnedBuffer,
            error_out: *mut $crate::__macro_support::OwnedBuffer,
        ) -> $crate::__macro_support::PluginStatus {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes =
                    unsafe { $crate::__macro_support::buffers::byte_slice_as_bytes(request) }?;
                $crate::__macro_support::codec::gameplay::decode_gameplay_request(request_bytes)
                    .map_err(|error| error.to_string())
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        error.to_string(),
                    );
                    return $crate::__macro_support::PluginStatus::INVALID_INPUT;
                }
                Err(_) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        "gameplay plugin panicked while decoding request".to_string(),
                    );
                    return $crate::__macro_support::PluginStatus::INTERNAL;
                }
            };

            let host_api = match unsafe {
                $crate::__macro_support::buffers::read_host_api(
                    host_api,
                    "gameplay host API",
                )
            } {
                Ok(host_api) => host_api,
                Err(error) => {
                    $crate::__macro_support::buffers::write_error_buffer(error_out, error);
                    return $crate::__macro_support::PluginStatus::ABI_MISMATCH;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::__macro_support::handle_gameplay_request_with_host_api(
                    mc_gameplay_plugin_instance(),
                    request.clone(),
                    Some(host_api),
                )
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::__macro_support::buffers::write_error_buffer(error_out, message);
                    return $crate::__macro_support::PluginStatus::INTERNAL;
                }
                Err(_) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        "gameplay plugin panicked while handling request".to_string(),
                    );
                    return $crate::__macro_support::PluginStatus::INTERNAL;
                }
            };

            match $crate::__macro_support::codec::gameplay::encode_gameplay_response(
                &request, &response,
            ) {
                Ok(bytes) => {
                    $crate::__macro_support::buffers::write_output_buffer(output, bytes);
                    $crate::__macro_support::PluginStatus::OK
                }
                Err(message) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        message.to_string(),
                    );
                    $crate::__macro_support::PluginStatus::INTERNAL
                }
            }
        }

        unsafe extern "C" fn mc_gameplay_plugin_free_buffer(
            buffer: $crate::__macro_support::OwnedBuffer,
        ) {
            unsafe { $crate::__macro_support::buffers::free_owned_buffer(buffer) };
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn mc_plugin_manifest_v9(
        ) -> *const $crate::__macro_support::PluginManifestV9 {
            std::ptr::from_ref(
                MC_GAMEPLAY_PLUGIN_MANIFEST
                    .get_or_init(|| $crate::manifest::manifest_from_static(&$manifest))
                    .manifest(),
            )
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn mc_plugin_gameplay_api_v9(
        ) -> *const $crate::__macro_support::GameplayPluginApiV9 {
            std::ptr::from_ref(MC_GAMEPLAY_PLUGIN_API.get_or_init(|| {
                $crate::__macro_support::GameplayPluginApiV9 {
                    abi: $crate::__macro_support::CURRENT_PLUGIN_ABI,
                    struct_size: std::mem::size_of::<$crate::__macro_support::GameplayPluginApiV9>(),
                    invoke: Some(mc_gameplay_plugin_invoke_v9),
                    free_buffer: Some(mc_gameplay_plugin_free_buffer),
                }
            }))
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __export_plugin_admin_surface {
    ($plugin_ty:ty, $manifest:expr $(,)?) => {
        static MC_ADMIN_SURFACE_PLUGIN_MANIFEST: std::sync::OnceLock<$crate::manifest::ExportedPluginManifest> =
            std::sync::OnceLock::new();
        static MC_ADMIN_SURFACE_PLUGIN_API: std::sync::OnceLock<$crate::__macro_support::AdminSurfacePluginApiV9> =
            std::sync::OnceLock::new();

        unsafe extern "C" fn mc_admin_surface_plugin_invoke_v9(
            request: $crate::__macro_support::ByteSlice,
            host_api: *const $crate::__macro_support::AdminSurfaceHostApiV9,
            output: *mut $crate::__macro_support::OwnedBuffer,
            error_out: *mut $crate::__macro_support::OwnedBuffer,
        ) -> $crate::__macro_support::PluginStatus {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes =
                    unsafe { $crate::__macro_support::buffers::byte_slice_as_bytes(request) }?;
                $crate::__macro_support::codec::admin_surface::decode_admin_surface_request(
                    request_bytes,
                )
                .map_err(|error| error.to_string())
            })) {
                Ok(Ok(request)) => request,
                Ok(Err(error)) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        error.to_string(),
                    );
                    return $crate::__macro_support::PluginStatus::INVALID_INPUT;
                }
                Err(_) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        "admin-surface plugin panicked while decoding request".to_string(),
                    );
                    return $crate::__macro_support::PluginStatus::INTERNAL;
                }
            };

            let host_api = match unsafe {
                $crate::__macro_support::buffers::read_host_api(
                    host_api,
                    "admin-surface host API",
                )
            } {
                Ok(host_api) => host_api,
                Err(error) => {
                    $crate::__macro_support::buffers::write_error_buffer(error_out, error);
                    return $crate::__macro_support::PluginStatus::ABI_MISMATCH;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::__macro_support::handle_admin_surface_request_with_host_api(
                    mc_admin_surface_plugin_instance(),
                    request.clone(),
                    Some(host_api),
                )
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::__macro_support::buffers::write_error_buffer(error_out, message);
                    return $crate::__macro_support::PluginStatus::INTERNAL;
                }
                Err(_) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        "admin-surface plugin panicked while handling request".to_string(),
                    );
                    return $crate::__macro_support::PluginStatus::INTERNAL;
                }
            };

            match $crate::__macro_support::codec::admin_surface::encode_admin_surface_response(
                &request, &response,
            ) {
                Ok(bytes) => {
                    $crate::__macro_support::buffers::write_output_buffer(output, bytes);
                    $crate::__macro_support::PluginStatus::OK
                }
                Err(message) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        message.to_string(),
                    );
                    $crate::__macro_support::PluginStatus::INTERNAL
                }
            }
        }

        unsafe extern "C" fn mc_admin_surface_plugin_free_buffer(
            buffer: $crate::__macro_support::OwnedBuffer,
        ) {
            unsafe { $crate::__macro_support::buffers::free_owned_buffer(buffer) };
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn mc_plugin_manifest_v9(
        ) -> *const $crate::__macro_support::PluginManifestV9 {
            std::ptr::from_ref(
                MC_ADMIN_SURFACE_PLUGIN_MANIFEST
                    .get_or_init(|| $crate::manifest::manifest_from_static(&$manifest))
                    .manifest(),
            )
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn mc_plugin_admin_surface_api_v9(
        ) -> *const $crate::__macro_support::AdminSurfacePluginApiV9 {
            std::ptr::from_ref(MC_ADMIN_SURFACE_PLUGIN_API.get_or_init(|| {
                $crate::__macro_support::AdminSurfacePluginApiV9 {
                    abi: $crate::__macro_support::CURRENT_PLUGIN_ABI,
                    struct_size: std::mem::size_of::<$crate::__macro_support::AdminSurfacePluginApiV9>(),
                    invoke: Some(mc_admin_surface_plugin_invoke_v9),
                    free_buffer: Some(mc_admin_surface_plugin_free_buffer),
                }
            }))
        }
    };
}

#[macro_export]
macro_rules! export_plugin {
    (protocol, $plugin_ty:ty, $manifest:expr $(,)?) => {
        $crate::__export_plugin_non_gameplay!(
            $plugin_ty,
            $manifest,
            $crate::__macro_support::ProtocolPluginApiV9,
            $crate::__macro_support::ProtocolPluginApiV9 {
                abi: $crate::__macro_support::CURRENT_PLUGIN_ABI,
                struct_size: std::mem::size_of::<$crate::__macro_support::ProtocolPluginApiV9>(),
                invoke: Some(mc_plugin_invoke),
                free_buffer: Some(mc_plugin_free_buffer),
            },
            $crate::__macro_support::codec::protocol::decode_protocol_request,
            $crate::__macro_support::handle_protocol_request,
            $crate::__macro_support::codec::protocol::encode_protocol_response,
            mc_plugin_protocol_api_v9,
            "protocol plugin panicked while decoding request",
            "protocol plugin panicked while handling request",
        );
    };
    (storage, $plugin_ty:ty, $manifest:expr $(,)?) => {
        $crate::__export_plugin_non_gameplay!(
            $plugin_ty,
            $manifest,
            $crate::__macro_support::StoragePluginApiV9,
            $crate::__macro_support::StoragePluginApiV9 {
                abi: $crate::__macro_support::CURRENT_PLUGIN_ABI,
                struct_size: std::mem::size_of::<$crate::__macro_support::StoragePluginApiV9>(),
                invoke: Some(mc_plugin_invoke),
                free_buffer: Some(mc_plugin_free_buffer),
            },
            $crate::__macro_support::codec::storage::decode_storage_request,
            $crate::__macro_support::handle_storage_request,
            $crate::__macro_support::codec::storage::encode_storage_response,
            mc_plugin_storage_api_v9,
            "storage plugin panicked while decoding request",
            "storage plugin panicked while handling request",
        );
    };
    (auth, $plugin_ty:ty, $manifest:expr $(,)?) => {
        $crate::__export_plugin_non_gameplay!(
            $plugin_ty,
            $manifest,
            $crate::__macro_support::AuthPluginApiV9,
            $crate::__macro_support::AuthPluginApiV9 {
                abi: $crate::__macro_support::CURRENT_PLUGIN_ABI,
                struct_size: std::mem::size_of::<$crate::__macro_support::AuthPluginApiV9>(),
                invoke: Some(mc_plugin_invoke),
                free_buffer: Some(mc_plugin_free_buffer),
            },
            $crate::__macro_support::codec::auth::decode_auth_request,
            $crate::__macro_support::handle_auth_request,
            $crate::__macro_support::codec::auth::encode_auth_response,
            mc_plugin_auth_api_v9,
            "auth plugin panicked while decoding request",
            "auth plugin panicked while handling request",
        );
    };
    (gameplay, $plugin_ty:ty, $manifest:expr $(,)?) => {
        $crate::__export_plugin_gameplay!($plugin_ty, $manifest);
    };
    (admin_surface, $plugin_ty:ty, $manifest:expr $(,)?) => {
        $crate::__export_plugin_admin_surface!($plugin_ty, $manifest);
    };
}
