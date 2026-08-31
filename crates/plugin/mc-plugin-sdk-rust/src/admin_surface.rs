use crate::{AdminSurfaceCapabilitySet, AdminSurfaceProfileId};
use mc_plugin_abi::host::{AdminSurfaceHostApiV9, HostFreeBufferFn};
use mc_plugin_abi::raw::{ByteSlice, OwnedBuffer, PluginStatus, Utf8Slice};
use mc_plugin_contract::codec::admin::{AdminPermission, AdminRequest, AdminResponse};
use mc_plugin_contract::codec::admin_surface::{
    AdminSurfaceDescriptor, AdminSurfaceInstanceDeclaration, AdminSurfacePauseView,
    AdminSurfaceResource, AdminSurfaceStatusView,
};
use std::marker::PhantomData;

pub trait AdminSurfaceHost {
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
pub struct SdkAdminSurfaceHost<'call> {
    api: AdminSurfaceHostApiV9,
    call: PhantomData<&'call ()>,
}

impl<'call> SdkAdminSurfaceHost<'call> {
    #[must_use]
    pub const fn new(api: AdminSurfaceHostApiV9) -> Self {
        Self {
            api,
            call: PhantomData,
        }
    }

    /// Acquires a generation-scoped host lease that can outlive this invocation.
    ///
    /// # Errors
    ///
    /// Returns an error if the host does not offer a retainable context or rejects retention.
    pub fn acquire_lease(&self) -> Result<AdminSurfaceHostLease, String> {
        let retain = self
            .api
            .retain_context
            .ok_or_else(|| "admin-surface host context cannot be retained".to_string())?;
        let release = self
            .api
            .release_context
            .ok_or_else(|| "admin-surface host context cannot be released".to_string())?;
        if !unsafe { retain(self.api.context) } {
            return Err("admin-surface host context retention was rejected".to_string());
        }
        Ok(AdminSurfaceHostLease {
            api: self.api,
            release,
        })
    }
}

pub struct AdminSurfaceHostLease {
    api: AdminSurfaceHostApiV9,
    release: mc_plugin_abi::host::HostReleaseContextFn,
}

impl Clone for AdminSurfaceHostLease {
    fn clone(&self) -> Self {
        let retain = self
            .api
            .retain_context
            .expect("retained admin-surface host lease must have a retain callback");
        assert!(
            unsafe { retain(self.api.context) },
            "retained admin-surface host lease could not be cloned"
        );
        Self {
            api: self.api,
            release: self.release,
        }
    }
}

impl Drop for AdminSurfaceHostLease {
    fn drop(&mut self) {
        unsafe { (self.release)(self.api.context) };
    }
}

// SAFETY: construction requires the host to retain the context for the lease lifetime. ABI 9
// defines retained admin-surface contexts and their callbacks as concurrently callable.
unsafe impl Send for AdminSurfaceHostLease {}
// SAFETY: the retained context contract permits concurrent immutable callback dispatch.
unsafe impl Sync for AdminSurfaceHostLease {}

macro_rules! impl_admin_surface_host {
    ($host:ty) => {
        impl AdminSurfaceHost for $host {
            fn log(&self, level: u32, message: &str) -> Result<(), String> {
                let Some(log) = self.api.log else {
                    return Ok(());
                };
                unsafe {
                    log(
                        self.api.context,
                        level,
                        Utf8Slice {
                            ptr: message.as_ptr(),
                            len: message.len(),
                        },
                    );
                }
                Ok(())
            }

            fn execute(
                &self,
                principal_id: &str,
                request: &AdminRequest,
            ) -> Result<AdminResponse, String> {
                let Some(callback) = self.api.execute else {
                    return Err("admin-surface host did not provide execute".to_string());
                };
                let request_bytes = serde_json::to_vec(request)
                    .map_err(|error| format!("failed to encode request: {error}"))?;
                let mut output = OwnedBuffer::empty();
                let mut error = OwnedBuffer::empty();
                let free_buffer = required_free_buffer(&self.api)?;
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
                if status != PluginStatus::OK {
                    crate::__macro_support::buffers::release_host_owned_buffer(output, free_buffer);
                    return Err(read_error_buffer(error, free_buffer));
                }
                crate::__macro_support::buffers::release_host_owned_buffer(error, free_buffer);
                decode_output_buffer(output, free_buffer)
            }

            fn permissions(&self, principal_id: &str) -> Result<Vec<AdminPermission>, String> {
                let Some(callback) = self.api.permissions else {
                    return Err("admin-surface host did not provide permissions".to_string());
                };
                call_host_json(
                    self.api.context,
                    principal_id,
                    callback,
                    required_free_buffer(&self.api)?,
                )
            }

            fn take_process_resource(
                &self,
                name: &str,
            ) -> Result<Option<AdminSurfaceResource>, String> {
                let Some(callback) = self.api.take_process_resource else {
                    return Err(
                        "admin-surface host did not provide take_process_resource".to_string()
                    );
                };
                call_host_resource(
                    self.api.context,
                    name,
                    callback,
                    required_free_buffer(&self.api)?,
                )
            }

            fn publish_handoff_resource(
                &self,
                name: &str,
                resource: &AdminSurfaceResource,
            ) -> Result<(), String> {
                let Some(callback) = self.api.publish_handoff_resource else {
                    return Err(
                        "admin-surface host did not provide publish_handoff_resource".to_string(),
                    );
                };
                let resource_bytes = serde_json::to_vec(resource)
                    .map_err(|error| format!("failed to encode admin-surface resource: {error}"))?;
                let mut error = OwnedBuffer::empty();
                let free_buffer = required_free_buffer(&self.api)?;
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
                if status != PluginStatus::OK {
                    return Err(read_error_buffer(error, free_buffer));
                }
                crate::__macro_support::buffers::release_host_owned_buffer(error, free_buffer);
                Ok(())
            }

            fn take_handoff_resource(
                &self,
                name: &str,
            ) -> Result<Option<AdminSurfaceResource>, String> {
                let Some(callback) = self.api.take_handoff_resource else {
                    return Err(
                        "admin-surface host did not provide take_handoff_resource".to_string()
                    );
                };
                call_host_resource(
                    self.api.context,
                    name,
                    callback,
                    required_free_buffer(&self.api)?,
                )
            }
        }
    };
}

impl_admin_surface_host!(SdkAdminSurfaceHost<'_>);
impl_admin_surface_host!(AdminSurfaceHostLease);

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
        host: SdkAdminSurfaceHost<'_>,
        surface_config_path: Option<&str>,
    ) -> Result<AdminSurfaceStatusView, String>;

    fn pause_for_upgrade(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost<'_>,
    ) -> Result<AdminSurfacePauseView, String>;

    fn resume_from_upgrade(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost<'_>,
        surface_config_path: Option<&str>,
        resume_payload: &[u8],
    ) -> Result<AdminSurfaceStatusView, String>;

    fn activate_after_upgrade_commit(
        &self,
        _instance_id: &str,
        _host: SdkAdminSurfaceHost<'_>,
    ) -> Result<(), String> {
        Ok(())
    }

    fn resume_after_upgrade_rollback(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost<'_>,
    ) -> Result<AdminSurfaceStatusView, String>;

    fn shutdown(&self, instance_id: &str, host: SdkAdminSurfaceHost<'_>) -> Result<(), String>;
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
    ) -> PluginStatus,
    free_buffer: HostFreeBufferFn,
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
    if status != PluginStatus::OK {
        crate::__macro_support::buffers::release_host_owned_buffer(output, free_buffer);
        return Err(read_error_buffer(error, free_buffer));
    }
    crate::__macro_support::buffers::release_host_owned_buffer(error, free_buffer);
    decode_output_buffer(output, free_buffer)
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
    ) -> PluginStatus,
    free_buffer: HostFreeBufferFn,
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
    if status != PluginStatus::OK {
        crate::__macro_support::buffers::release_host_owned_buffer(output, free_buffer);
        return Err(read_error_buffer(error, free_buffer));
    }
    crate::__macro_support::buffers::release_host_owned_buffer(error, free_buffer);
    if !present {
        crate::__macro_support::buffers::release_host_owned_buffer(output, free_buffer);
        return Ok(None);
    }
    decode_output_buffer(output, free_buffer).map(Some)
}

fn decode_output_buffer<T: for<'de> serde::Deserialize<'de>>(
    output: OwnedBuffer,
    free_buffer: HostFreeBufferFn,
) -> Result<T, String> {
    let bytes = crate::__macro_support::buffers::copy_host_owned_buffer(
        output,
        free_buffer,
        "admin-surface host output",
    )?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("failed to decode admin-surface host payload: {error}"))
}

fn read_error_buffer(buffer: OwnedBuffer, free_buffer: HostFreeBufferFn) -> String {
    match crate::__macro_support::buffers::copy_host_owned_buffer(
        buffer,
        free_buffer,
        "admin-surface host error",
    ) {
        Ok(bytes) if bytes.is_empty() => "admin-surface host callback failed".to_string(),
        Ok(bytes) => String::from_utf8(bytes)
            .unwrap_or_else(|_| "admin-surface host callback returned invalid utf-8".to_string()),
        Err(error) => format!("admin-surface host returned invalid error buffer: {error}"),
    }
}

fn required_free_buffer(api: &AdminSurfaceHostApiV9) -> Result<HostFreeBufferFn, String> {
    api.free_buffer
        .ok_or_else(|| "admin-surface host did not provide free_buffer".to_string())
}
