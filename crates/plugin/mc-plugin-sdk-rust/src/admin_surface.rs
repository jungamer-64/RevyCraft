use mc_plugin_api::abi::{ByteSlice, OwnedBuffer, PluginErrorCode, Utf8Slice};
use mc_plugin_api::codec::admin::{AdminPermission, AdminRequest, AdminResponse};
use mc_plugin_api::codec::admin_surface::{
    AdminSurfaceDescriptor, AdminSurfaceInstanceDeclaration, AdminSurfacePauseView,
    AdminSurfaceResource, AdminSurfaceStatusView,
};
use mc_plugin_api::host_api::AdminSurfaceHostApiV1;
use crate::{AdminSurfaceCapabilitySet, AdminSurfaceProfileId};

pub trait AdminSurfaceHost: Send + Sync {
    fn log(&self, level: u32, message: &str) -> Result<(), String>;

    fn execute(&self, principal_id: &str, request: &AdminRequest) -> Result<AdminResponse, String>;

    fn permissions(&self, principal_id: &str) -> Result<Vec<AdminPermission>, String>;

    fn take_process_resource(&self, name: &str) -> Result<Option<AdminSurfaceResource>, String>;

    fn publish_handoff_resource(
        &self,
        name: &str,
        resource: &AdminSurfaceResource,
    ) -> Result<(), String>;

    fn take_handoff_resource(&self, name: &str) -> Result<Option<AdminSurfaceResource>, String>;
}

#[derive(Clone, Copy)]
pub struct SdkAdminSurfaceHost {
    api: AdminSurfaceHostApiV1,
}

impl SdkAdminSurfaceHost {
    #[must_use]
    pub const fn new(api: AdminSurfaceHostApiV1) -> Self {
        Self { api }
    }
}

impl AdminSurfaceHost for SdkAdminSurfaceHost {
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

    fn execute(&self, principal_id: &str, request: &AdminRequest) -> Result<AdminResponse, String> {
        let Some(callback) = self.api.execute else {
            return Err("admin-surface host did not provide execute".to_string());
        };
        let request_bytes = serde_json::to_vec(request)
            .map_err(|error| format!("failed to encode request: {error}"))?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            callback(
                self.api.context,
                Utf8Slice {
                    ptr: principal_id.as_ptr(),
                    len: principal_id.len(),
                },
                ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
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

    fn permissions(&self, principal_id: &str) -> Result<Vec<AdminPermission>, String> {
        let Some(callback) = self.api.permissions else {
            return Err("admin-surface host did not provide permissions".to_string());
        };
        call_host_json(self.api.context, principal_id, callback)
    }

    fn take_process_resource(&self, name: &str) -> Result<Option<AdminSurfaceResource>, String> {
        let Some(callback) = self.api.take_process_resource else {
            return Err("admin-surface host did not provide take_process_resource".to_string());
        };
        call_host_resource(self.api.context, name, callback)
    }

    fn publish_handoff_resource(
        &self,
        name: &str,
        resource: &AdminSurfaceResource,
    ) -> Result<(), String> {
        let Some(callback) = self.api.publish_handoff_resource else {
            return Err("admin-surface host did not provide publish_handoff_resource".to_string());
        };
        let resource_bytes = serde_json::to_vec(resource)
            .map_err(|error| format!("failed to encode admin-surface resource: {error}"))?;
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            callback(
                self.api.context,
                Utf8Slice {
                    ptr: name.as_ptr(),
                    len: name.len(),
                },
                ByteSlice {
                    ptr: resource_bytes.as_ptr(),
                    len: resource_bytes.len(),
                },
                &raw mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            return Err(read_error_buffer(error));
        }
        Ok(())
    }

    fn take_handoff_resource(&self, name: &str) -> Result<Option<AdminSurfaceResource>, String> {
        let Some(callback) = self.api.take_handoff_resource else {
            return Err("admin-surface host did not provide take_handoff_resource".to_string());
        };
        call_host_resource(self.api.context, name, callback)
    }
}

pub trait RustAdminSurfacePlugin: Send + Sync + 'static {
    fn descriptor(&self) -> AdminSurfaceDescriptor;

    fn capability_set(&self) -> AdminSurfaceCapabilitySet {
        AdminSurfaceCapabilitySet::default()
    }

    fn declare_instance(
        &self,
        instance_id: &str,
        surface_config_path: Option<&str>,
    ) -> Result<AdminSurfaceInstanceDeclaration, String>;

    fn start(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost,
        surface_config_path: Option<&str>,
    ) -> Result<AdminSurfaceStatusView, String>;

    fn pause_for_upgrade(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost,
    ) -> Result<AdminSurfacePauseView, String>;

    fn resume_from_upgrade(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost,
        surface_config_path: Option<&str>,
        resume_payload: &[u8],
    ) -> Result<AdminSurfaceStatusView, String>;

    fn activate_after_upgrade_commit(
        &self,
        _instance_id: &str,
        _host: SdkAdminSurfaceHost,
    ) -> Result<(), String> {
        Ok(())
    }

    fn resume_after_upgrade_rollback(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost,
    ) -> Result<AdminSurfaceStatusView, String>;

    fn shutdown(&self, instance_id: &str, host: SdkAdminSurfaceHost) -> Result<(), String>;
}

#[must_use]
pub fn admin_surface_descriptor(profile: impl Into<String>) -> AdminSurfaceDescriptor {
    AdminSurfaceDescriptor {
        surface_profile: AdminSurfaceProfileId::new(profile.into()),
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

fn call_host_resource(
    context: *mut std::ffi::c_void,
    name: &str,
    callback: unsafe extern "C" fn(
        *mut std::ffi::c_void,
        Utf8Slice,
        *mut bool,
        *mut OwnedBuffer,
        *mut OwnedBuffer,
    ) -> PluginErrorCode,
) -> Result<Option<AdminSurfaceResource>, String> {
    let mut present = false;
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        callback(
            context,
            Utf8Slice {
                ptr: name.as_ptr(),
                len: name.len(),
            },
            &raw mut present,
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        return Err(read_error_buffer(error));
    }
    if !present {
        if !output.ptr.is_null() {
            unsafe {
                crate::__macro_support::buffers::free_owned_buffer(output);
            }
        }
        return Ok(None);
    }
    decode_output_buffer(output).map(Some)
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
        .map_err(|error| format!("failed to decode admin-surface host payload: {error}"))
}

fn read_error_buffer(buffer: OwnedBuffer) -> String {
    if buffer.ptr.is_null() {
        return "admin-surface host callback failed".to_string();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len) }.to_vec();
    unsafe {
        crate::__macro_support::buffers::free_owned_buffer(buffer);
    }
    String::from_utf8(bytes)
        .unwrap_or_else(|_| "admin-surface host callback returned invalid utf-8".to_string())
}
