use mc_core::{AdminTransportCapabilitySet, AdminTransportProfileId};
use mc_plugin_api::abi::{OwnedBuffer, PluginErrorCode, Utf8Slice};
use mc_plugin_api::codec::admin_transport::{
    AdminTransportDescriptor, AdminTransportPauseView, AdminTransportStatusView,
};
use mc_plugin_api::codec::admin_ui::{
    AdminRuntimeReloadView, AdminSessionsView, AdminStatusView, AdminUpgradeRuntimeView,
    RuntimeReloadMode,
};
use mc_plugin_api::host_api::AdminTransportHostApiV1;

pub trait AdminTransportHost: Send + Sync {
    fn log(&self, level: u32, message: &str) -> Result<(), String>;

    fn status(&self, principal_id: &str) -> Result<AdminStatusView, String>;

    fn sessions(&self, principal_id: &str) -> Result<AdminSessionsView, String>;

    fn reload_runtime(
        &self,
        principal_id: &str,
        mode: RuntimeReloadMode,
    ) -> Result<AdminRuntimeReloadView, String>;

    fn upgrade_runtime(
        &self,
        principal_id: &str,
        executable_path: &str,
    ) -> Result<AdminUpgradeRuntimeView, String>;

    fn shutdown(&self, principal_id: &str) -> Result<(), String>;

    fn publish_tcp_listener_for_upgrade(&self, raw_listener: usize) -> Result<(), String>;

    fn take_tcp_listener_from_upgrade(&self) -> Result<Option<usize>, String>;
}

#[derive(Clone, Copy)]
pub struct SdkAdminTransportHost {
    api: AdminTransportHostApiV1,
}

impl SdkAdminTransportHost {
    #[must_use]
    pub const fn new(api: AdminTransportHostApiV1) -> Self {
        Self { api }
    }
}

impl AdminTransportHost for SdkAdminTransportHost {
    fn log(&self, level: u32, message: &str) -> Result<(), String> {
        let Some(log) = self.api.log else {
            return Ok(());
        };
        unsafe {
            log(
                level,
                Utf8Slice {
                    ptr: message.as_ptr(),
                    len: message.len(),
                },
            );
        }
        Ok(())
    }

    fn status(&self, principal_id: &str) -> Result<AdminStatusView, String> {
        let Some(callback) = self.api.get_status else {
            return Err("admin-transport host did not provide get_status".to_string());
        };
        call_host_json(self.api.context, principal_id, callback)
    }

    fn sessions(&self, principal_id: &str) -> Result<AdminSessionsView, String> {
        let Some(callback) = self.api.list_sessions else {
            return Err("admin-transport host did not provide list_sessions".to_string());
        };
        call_host_json(self.api.context, principal_id, callback)
    }

    fn reload_runtime(
        &self,
        principal_id: &str,
        mode: RuntimeReloadMode,
    ) -> Result<AdminRuntimeReloadView, String> {
        let Some(callback) = self.api.reload_runtime else {
            return Err("admin-transport host did not provide reload_runtime".to_string());
        };
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            callback(
                self.api.context,
                Utf8Slice {
                    ptr: principal_id.as_ptr(),
                    len: principal_id.len(),
                },
                mc_plugin_api::codec::admin_transport::encode_reload_mode(mode),
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            return Err(read_error_buffer(error));
        }
        decode_output_buffer(output)
    }

    fn upgrade_runtime(
        &self,
        principal_id: &str,
        executable_path: &str,
    ) -> Result<AdminUpgradeRuntimeView, String> {
        let Some(callback) = self.api.upgrade_runtime else {
            return Err("admin-transport host did not provide upgrade_runtime".to_string());
        };
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            callback(
                self.api.context,
                Utf8Slice {
                    ptr: principal_id.as_ptr(),
                    len: principal_id.len(),
                },
                Utf8Slice {
                    ptr: executable_path.as_ptr(),
                    len: executable_path.len(),
                },
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            return Err(read_error_buffer(error));
        }
        decode_output_buffer(output)
    }

    fn shutdown(&self, principal_id: &str) -> Result<(), String> {
        let Some(callback) = self.api.shutdown else {
            return Err("admin-transport host did not provide shutdown".to_string());
        };
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            callback(
                self.api.context,
                Utf8Slice {
                    ptr: principal_id.as_ptr(),
                    len: principal_id.len(),
                },
                &raw mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            return Err(read_error_buffer(error));
        }
        Ok(())
    }

    fn publish_tcp_listener_for_upgrade(&self, raw_listener: usize) -> Result<(), String> {
        let Some(callback) = self.api.publish_tcp_listener_for_upgrade else {
            return Err(
                "admin-transport host did not provide publish_tcp_listener_for_upgrade".to_string(),
            );
        };
        let mut error = OwnedBuffer::empty();
        let status = unsafe { callback(self.api.context, raw_listener, &raw mut error) };
        if status != PluginErrorCode::Ok {
            return Err(read_error_buffer(error));
        }
        Ok(())
    }

    fn take_tcp_listener_from_upgrade(&self) -> Result<Option<usize>, String> {
        let Some(callback) = self.api.take_tcp_listener_from_upgrade else {
            return Err(
                "admin-transport host did not provide take_tcp_listener_from_upgrade".to_string(),
            );
        };
        let mut present = false;
        let mut raw_listener = 0;
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            callback(
                self.api.context,
                &raw mut present,
                &raw mut raw_listener,
                &raw mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            return Err(read_error_buffer(error));
        }
        Ok(present.then_some(raw_listener))
    }
}

pub trait RustAdminTransportPlugin: Send + Sync + 'static {
    fn descriptor(&self) -> AdminTransportDescriptor;

    fn capability_set(&self) -> AdminTransportCapabilitySet {
        AdminTransportCapabilitySet::default()
    }

    fn start(
        &self,
        host: SdkAdminTransportHost,
        transport_config_path: &str,
    ) -> Result<AdminTransportStatusView, String>;

    fn pause_for_upgrade(
        &self,
        host: SdkAdminTransportHost,
    ) -> Result<AdminTransportPauseView, String>;

    fn resume_from_upgrade(
        &self,
        host: SdkAdminTransportHost,
        transport_config_path: &str,
        resume_payload: &[u8],
    ) -> Result<AdminTransportStatusView, String>;

    fn resume_after_upgrade_rollback(
        &self,
        host: SdkAdminTransportHost,
    ) -> Result<AdminTransportStatusView, String>;

    fn shutdown(&self, host: SdkAdminTransportHost) -> Result<(), String>;
}

#[must_use]
pub fn admin_transport_descriptor(profile: impl Into<String>) -> AdminTransportDescriptor {
    AdminTransportDescriptor {
        transport_profile: AdminTransportProfileId::new(profile.into()),
    }
}

fn call_host_json<T: for<'de> serde::Deserialize<'de>>(
    context: *mut std::ffi::c_void,
    principal_id: &str,
    callback: unsafe extern "C" fn(
        *mut std::ffi::c_void,
        Utf8Slice,
        *mut OwnedBuffer,
        *mut OwnedBuffer,
    ) -> PluginErrorCode,
) -> Result<T, String> {
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        callback(
            context,
            Utf8Slice {
                ptr: principal_id.as_ptr(),
                len: principal_id.len(),
            },
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        return Err(read_error_buffer(error));
    }
    decode_output_buffer(output)
}

fn decode_output_buffer<T: for<'de> serde::Deserialize<'de>>(
    output: OwnedBuffer,
) -> Result<T, String> {
    let bytes = if output.ptr.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec()
    };
    if !output.ptr.is_null() {
        unsafe {
            crate::__macro_support::buffers::free_owned_buffer(output);
        }
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("failed to decode admin-transport host payload: {error}"))
}

fn read_error_buffer(buffer: OwnedBuffer) -> String {
    if buffer.ptr.is_null() {
        return "admin-transport host callback failed".to_string();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len) }.to_vec();
    unsafe {
        crate::__macro_support::buffers::free_owned_buffer(buffer);
    }
    String::from_utf8(bytes)
        .unwrap_or_else(|_| "admin-transport host callback returned invalid utf-8".to_string())
}
