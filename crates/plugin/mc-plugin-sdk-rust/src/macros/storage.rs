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
                let request_bytes =
                    unsafe { $crate::__macro_support::buffers::byte_slice_as_bytes(request) };
                mc_plugin_api::codec::storage::decode_storage_request(request_bytes)
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
                        "storage plugin panicked while decoding request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            let response = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                $crate::__macro_support::handle_storage_request(
                    mc_storage_plugin_instance(),
                    request.clone(),
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
                        "storage plugin panicked while handling request".to_string(),
                    );
                    return mc_plugin_api::abi::PluginErrorCode::Internal;
                }
            };

            match mc_plugin_api::codec::storage::encode_storage_response(&request, &response) {
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

        unsafe extern "C" fn mc_storage_plugin_free_buffer(buffer: mc_plugin_api::abi::OwnedBuffer) {
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

        #[cfg(any(test, feature = "in-process-testing"))]
        #[must_use]
        pub fn in_process_storage_entrypoints() -> $crate::test_support::InProcessStorageEntrypoints {
            $crate::test_support::InProcessStorageEntrypoints {
                manifest: unsafe { &*mc_plugin_manifest_v1() },
                api: unsafe { &*mc_plugin_storage_api_v1() },
            }
        }
    };
}
