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
        static MC_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> = std::sync::OnceLock::new();
        static MC_PLUGIN_MANIFEST: std::sync::OnceLock<$crate::manifest::ExportedPluginManifest> =
            std::sync::OnceLock::new();
        static MC_PLUGIN_API: std::sync::OnceLock<$api_ty> = std::sync::OnceLock::new();

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
                $decode(request_bytes)
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
                        $panic_decode.to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $handle(mc_plugin_instance(), request.clone())
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::__macro_support::buffers::write_error_buffer(error_out, message);
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        $panic_handle.to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            match $encode(&request, &response) {
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
                    .get_or_init(|| $crate::manifest::manifest_from_static(&$manifest))
                    .manifest(),
            )
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn $api_symbol() -> *const $api_ty {
            std::ptr::from_ref(MC_PLUGIN_API.get_or_init(|| $api_init))
        }

        #[cfg(any(test, feature = "in-process-testing"))]
        #[must_use]
        pub fn in_process_plugin_entrypoints()
        -> $crate::test_support::InProcessPluginEntrypoints<$api_ty> {
            $crate::test_support::InProcessPluginEntrypoints::new(
                unsafe { &*mc_plugin_manifest_v1() },
                unsafe { &*$api_symbol() },
            )
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __export_plugin_gameplay {
    ($plugin_ty:ty, $manifest:expr $(,)?) => {
        static MC_GAMEPLAY_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> =
            std::sync::OnceLock::new();
        static MC_GAMEPLAY_PLUGIN_MANIFEST: std::sync::OnceLock<$crate::manifest::ExportedPluginManifest> =
            std::sync::OnceLock::new();
        static MC_GAMEPLAY_PLUGIN_API: std::sync::OnceLock<mc_plugin_api::host_api::GameplayPluginApiV2> =
            std::sync::OnceLock::new();

        fn mc_gameplay_plugin_instance() -> &'static $plugin_ty {
            MC_GAMEPLAY_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        unsafe extern "C" fn mc_gameplay_plugin_invoke_v2(
            request: mc_plugin_api::abi::ByteSlice,
            host_api: *const mc_plugin_api::host_api::HostApiTableV1,
            output: *mut mc_plugin_api::abi::OwnedBuffer,
            error_out: *mut mc_plugin_api::abi::OwnedBuffer,
        ) -> mc_plugin_api::abi::PluginErrorCode {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes =
                    unsafe { $crate::__macro_support::buffers::byte_slice_as_bytes(request) };
                mc_plugin_api::codec::gameplay::decode_gameplay_request(request_bytes)
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
                        "gameplay plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            let Some(host_api) = (unsafe { host_api.as_ref() }) else {
                $crate::__macro_support::buffers::write_error_buffer(
                    error_out,
                    "gameplay host api was null".to_string(),
                );
                return mc_plugin_api::abi::PluginErrorCode::InvalidInput;
            };
            if host_api.abi != mc_plugin_api::abi::CURRENT_PLUGIN_ABI {
                $crate::__macro_support::buffers::write_error_buffer(
                    error_out,
                    format!(
                        "gameplay host api ABI {} did not match plugin ABI {}",
                        host_api.abi,
                        mc_plugin_api::abi::CURRENT_PLUGIN_ABI
                    ),
                );
                return mc_plugin_api::abi::PluginErrorCode::AbiMismatch;
            }

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::__macro_support::handle_gameplay_request_with_host_api(
                    mc_gameplay_plugin_instance(),
                    request.clone(),
                    Some(*host_api),
                )
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::__macro_support::buffers::write_error_buffer(error_out, message);
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        "gameplay plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::codec::gameplay::encode_gameplay_response(&request, &response) {
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

        unsafe extern "C" fn mc_gameplay_plugin_free_buffer(
            buffer: mc_plugin_api::abi::OwnedBuffer,
        ) {
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
                MC_GAMEPLAY_PLUGIN_MANIFEST
                    .get_or_init(|| $crate::manifest::manifest_from_static(&$manifest))
                    .manifest(),
            )
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_gameplay_api_v2() -> *const mc_plugin_api::host_api::GameplayPluginApiV2 {
            std::ptr::from_ref(MC_GAMEPLAY_PLUGIN_API.get_or_init(|| {
                mc_plugin_api::host_api::GameplayPluginApiV2 {
                    invoke: mc_gameplay_plugin_invoke_v2,
                    free_buffer: mc_gameplay_plugin_free_buffer,
                }
            }))
        }

        #[cfg(any(test, feature = "in-process-testing"))]
        #[must_use]
        pub fn in_process_plugin_entrypoints()
        -> $crate::test_support::InProcessPluginEntrypoints<mc_plugin_api::host_api::GameplayPluginApiV2>
        {
            $crate::test_support::InProcessPluginEntrypoints::new(
                unsafe { &*mc_plugin_manifest_v1() },
                unsafe { &*mc_plugin_gameplay_api_v2() },
            )
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __export_plugin_admin_ui {
    ($plugin_ty:ty, $manifest:expr $(,)?) => {
        static MC_ADMIN_UI_PLUGIN_INSTANCE: std::sync::OnceLock<$plugin_ty> =
            std::sync::OnceLock::new();
        static MC_ADMIN_UI_PLUGIN_MANIFEST: std::sync::OnceLock<$crate::manifest::ExportedPluginManifest> =
            std::sync::OnceLock::new();
        static MC_ADMIN_UI_PLUGIN_API: std::sync::OnceLock<mc_plugin_api::host_api::AdminUiPluginApiV1> =
            std::sync::OnceLock::new();

        fn mc_admin_ui_plugin_instance() -> &'static $plugin_ty {
            MC_ADMIN_UI_PLUGIN_INSTANCE.get_or_init(<$plugin_ty>::default)
        }

        unsafe extern "C" fn mc_admin_ui_plugin_invoke_v1(
            request: mc_plugin_api::abi::ByteSlice,
            host_api: *const mc_plugin_api::host_api::HostApiTableV1,
            output: *mut mc_plugin_api::abi::OwnedBuffer,
            error_out: *mut mc_plugin_api::abi::OwnedBuffer,
        ) -> mc_plugin_api::abi::PluginErrorCode {
            let request = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let request_bytes =
                    unsafe { $crate::__macro_support::buffers::byte_slice_as_bytes(request) };
                mc_plugin_api::codec::admin_ui::decode_admin_ui_input(request_bytes)
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
                        "admin-ui plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            let Some(host_api) = (unsafe { host_api.as_ref() }) else {
                $crate::__macro_support::buffers::write_error_buffer(
                    error_out,
                    "admin-ui host api was null".to_string(),
                );
                return mc_plugin_api::abi::PluginErrorCode::InvalidInput;
            };
            if host_api.abi != mc_plugin_api::abi::CURRENT_PLUGIN_ABI {
                $crate::__macro_support::buffers::write_error_buffer(
                    error_out,
                    format!(
                        "admin-ui host api ABI {} did not match plugin ABI {}",
                        host_api.abi,
                        mc_plugin_api::abi::CURRENT_PLUGIN_ABI
                    ),
                );
                return mc_plugin_api::abi::PluginErrorCode::AbiMismatch;
            }

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::__macro_support::handle_admin_ui_request_with_host_api(
                    mc_admin_ui_plugin_instance(),
                    request.clone(),
                    Some(*host_api),
                )
            })) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => {
                    $crate::__macro_support::buffers::write_error_buffer(error_out, message);
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
                Err(_) => {
                    $crate::__macro_support::buffers::write_error_buffer(
                        error_out,
                        "admin-ui plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::codec::admin_ui::encode_admin_ui_output(&request, &response) {
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

        unsafe extern "C" fn mc_admin_ui_plugin_free_buffer(
            buffer: mc_plugin_api::abi::OwnedBuffer,
        ) {
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
                MC_ADMIN_UI_PLUGIN_MANIFEST
                    .get_or_init(|| $crate::manifest::manifest_from_static(&$manifest))
                    .manifest(),
            )
        }

        #[cfg_attr(
            all(not(test), not(feature = "disable-exported-symbols")),
            unsafe(no_mangle)
        )]
        pub extern "C" fn mc_plugin_admin_ui_api_v1() -> *const mc_plugin_api::host_api::AdminUiPluginApiV1 {
            std::ptr::from_ref(MC_ADMIN_UI_PLUGIN_API.get_or_init(|| {
                mc_plugin_api::host_api::AdminUiPluginApiV1 {
                    invoke: mc_admin_ui_plugin_invoke_v1,
                    free_buffer: mc_admin_ui_plugin_free_buffer,
                }
            }))
        }

        #[cfg(any(test, feature = "in-process-testing"))]
        #[must_use]
        pub fn in_process_plugin_entrypoints()
        -> $crate::test_support::InProcessPluginEntrypoints<mc_plugin_api::host_api::AdminUiPluginApiV1>
        {
            $crate::test_support::InProcessPluginEntrypoints::new(
                unsafe { &*mc_plugin_manifest_v1() },
                unsafe { &*mc_plugin_admin_ui_api_v1() },
            )
        }
    };
}

#[macro_export]
macro_rules! export_plugin {
    (protocol, $plugin_ty:ty, $manifest:expr $(,)?) => {
        $crate::__export_plugin_non_gameplay!(
            $plugin_ty,
            $manifest,
            mc_plugin_api::host_api::ProtocolPluginApiV1,
            mc_plugin_api::host_api::ProtocolPluginApiV1 {
                invoke: mc_plugin_invoke,
                free_buffer: mc_plugin_free_buffer,
            },
            mc_plugin_api::codec::protocol::decode_protocol_request,
            $crate::__macro_support::handle_protocol_request,
            mc_plugin_api::codec::protocol::encode_protocol_response,
            mc_plugin_protocol_api_v1,
            "protocol plugin panicked while decoding request",
            "protocol plugin panicked while handling request",
        );
    };
    (storage, $plugin_ty:ty, $manifest:expr $(,)?) => {
        $crate::__export_plugin_non_gameplay!(
            $plugin_ty,
            $manifest,
            mc_plugin_api::host_api::StoragePluginApiV1,
            mc_plugin_api::host_api::StoragePluginApiV1 {
                invoke: mc_plugin_invoke,
                free_buffer: mc_plugin_free_buffer,
            },
            mc_plugin_api::codec::storage::decode_storage_request,
            $crate::__macro_support::handle_storage_request,
            mc_plugin_api::codec::storage::encode_storage_response,
            mc_plugin_storage_api_v1,
            "storage plugin panicked while decoding request",
            "storage plugin panicked while handling request",
        );
    };
    (auth, $plugin_ty:ty, $manifest:expr $(,)?) => {
        $crate::__export_plugin_non_gameplay!(
            $plugin_ty,
            $manifest,
            mc_plugin_api::host_api::AuthPluginApiV1,
            mc_plugin_api::host_api::AuthPluginApiV1 {
                invoke: mc_plugin_invoke,
                free_buffer: mc_plugin_free_buffer,
            },
            mc_plugin_api::codec::auth::decode_auth_request,
            $crate::__macro_support::handle_auth_request,
            mc_plugin_api::codec::auth::encode_auth_response,
            mc_plugin_auth_api_v1,
            "auth plugin panicked while decoding request",
            "auth plugin panicked while handling request",
        );
    };
    (gameplay, $plugin_ty:ty, $manifest:expr $(,)?) => {
        $crate::__export_plugin_gameplay!($plugin_ty, $manifest);
    };
    (admin_ui, $plugin_ty:ty, $manifest:expr $(,)?) => {
        $crate::__export_plugin_admin_ui!($plugin_ty, $manifest);
    };
}
