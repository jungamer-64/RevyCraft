use crate::process_surfaces::ProcessSurfaceCommand;
use mc_plugin_api::abi::{OwnedBuffer, PluginErrorCode, Utf8Slice};
use mc_plugin_api::host_api::AdminTransportHostApiV1;
use server_runtime::RuntimeError;
use server_runtime::runtime::{
    AdminCommandError, AdminControlPlaneHandle, AdminSubject, AdminTransportSelection,
    RuntimeReloadMode, ServerSupervisor,
};
use std::ffi::c_void;
#[cfg(unix)]
use std::os::fd::{FromRawFd, IntoRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{FromRawSocket, IntoRawSocket, RawSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub(crate) struct PausedAdminTransport {
    pub(crate) listener_for_child: std::net::TcpListener,
    pub(crate) resume_payload: Vec<u8>,
}

pub(crate) struct AdminTransportSupervisor {
    server: Arc<ServerSupervisor>,
    host_context: Arc<AdminTransportHostContext>,
    state: SupervisorState,
}

enum SupervisorState {
    Inactive,
    Active(TrackedTransport),
    Paused(TrackedTransport),
}

struct TrackedTransport {
    transport_config_path: PathBuf,
    profile: Arc<dyn mc_plugin_host::runtime::AdminTransportProfileHandle>,
    profile_id: String,
    generation_id: Option<mc_core::PluginGenerationId>,
}

struct AdminTransportHostContext {
    control_plane: AdminControlPlaneHandle,
    runtime_handle: tokio::runtime::Handle,
    surface_control_tx: mpsc::Sender<ProcessSurfaceCommand>,
    published_listener: Mutex<Option<std::net::TcpListener>>,
    child_resume_listener: Mutex<Option<std::net::TcpListener>>,
}

impl AdminTransportSupervisor {
    pub(crate) fn new(
        server: Arc<ServerSupervisor>,
        control_plane: AdminControlPlaneHandle,
        surface_control_tx: mpsc::Sender<ProcessSurfaceCommand>,
    ) -> Self {
        Self {
            server,
            host_context: Arc::new(AdminTransportHostContext {
                control_plane,
                runtime_handle: tokio::runtime::Handle::current(),
                surface_control_tx,
                published_listener: Mutex::new(None),
                child_resume_listener: Mutex::new(None),
            }),
            state: SupervisorState::Inactive,
        }
    }

    pub(crate) fn host_api(&self) -> AdminTransportHostApiV1 {
        self.host_context.host_api()
    }

    pub(crate) fn has_remote_surface(&self) -> bool {
        matches!(self.state, SupervisorState::Active(_))
    }

    pub(crate) async fn reconcile(&mut self) -> Result<(), RuntimeError> {
        let desired = self.server.current_admin_transport().await;
        match (&self.state, desired.as_ref()) {
            (SupervisorState::Inactive, None) => Ok(()),
            (SupervisorState::Active(active), Some(selection))
                if same_transport(active, selection) =>
            {
                Ok(())
            }
            (SupervisorState::Paused(_), _) => Err(RuntimeError::Config(
                "cannot reconcile admin transport while it is paused for upgrade".to_string(),
            )),
            _ => {
                self.shutdown_current()?;
                if let Some(selection) = desired {
                    let host_api = self.host_api();
                    let _status = selection
                        .profile
                        .start(&selection.transport_config_path, host_api)?;
                    self.state = SupervisorState::Active(track_selection(selection));
                }
                Ok(())
            }
        }
    }

    pub(crate) async fn resume_from_upgrade(
        &mut self,
        listener: std::net::TcpListener,
        resume_payload: Vec<u8>,
    ) -> Result<(), RuntimeError> {
        let Some(selection) = self.server.current_admin_transport().await else {
            return Err(RuntimeError::Config(
                "upgrade child did not have an active admin transport selection".to_string(),
            ));
        };
        self.host_context.install_child_resume_listener(listener);
        let host_api = self.host_api();
        let _status = selection.profile.resume_from_upgrade(
            &selection.transport_config_path,
            &resume_payload,
            host_api,
        )?;
        self.state = SupervisorState::Active(track_selection(selection));
        Ok(())
    }

    pub(crate) async fn pause_for_upgrade(
        &mut self,
    ) -> Result<Option<PausedAdminTransport>, RuntimeError> {
        let active = match std::mem::replace(&mut self.state, SupervisorState::Inactive) {
            SupervisorState::Inactive => return Ok(None),
            SupervisorState::Active(active) => active,
            SupervisorState::Paused(paused) => {
                self.state = SupervisorState::Paused(paused);
                return Err(RuntimeError::Config(
                    "admin transport is already paused for upgrade".to_string(),
                ));
            }
        };
        let pause = match active.profile.pause_for_upgrade(self.host_api()) {
            Ok(pause) => pause,
            Err(error) => {
                self.state = SupervisorState::Active(active);
                return Err(error.into());
            }
        };
        let listener_for_child = self.host_context.take_published_listener().ok_or_else(|| {
            RuntimeError::Config(
                "admin transport did not publish a listener for upgrade handoff".to_string(),
            )
        });
        let listener_for_child = match listener_for_child {
            Ok(listener_for_child) => listener_for_child,
            Err(error) => {
                let _ = active
                    .profile
                    .resume_after_upgrade_rollback(self.host_api());
                self.state = SupervisorState::Active(active);
                return Err(error);
            }
        };
        let paused = PausedAdminTransport {
            listener_for_child,
            resume_payload: pause.resume_payload,
        };
        self.state = SupervisorState::Paused(active);
        Ok(Some(paused))
    }

    pub(crate) fn resume_after_upgrade_rollback(&mut self) -> Result<(), RuntimeError> {
        let paused = match std::mem::replace(&mut self.state, SupervisorState::Inactive) {
            SupervisorState::Inactive => return Ok(()),
            SupervisorState::Active(active) => {
                self.state = SupervisorState::Active(active);
                return Ok(());
            }
            SupervisorState::Paused(paused) => paused,
        };
        let _status = paused
            .profile
            .resume_after_upgrade_rollback(self.host_api())?;
        self.state = SupervisorState::Active(paused);
        Ok(())
    }

    pub(crate) fn shutdown_current(&mut self) -> Result<(), RuntimeError> {
        match std::mem::replace(&mut self.state, SupervisorState::Inactive) {
            SupervisorState::Inactive => Ok(()),
            SupervisorState::Active(active) | SupervisorState::Paused(active) => {
                Ok(active.profile.shutdown(self.host_api())?)
            }
        }
    }
}

fn track_selection(selection: AdminTransportSelection) -> TrackedTransport {
    TrackedTransport {
        transport_config_path: selection.transport_config_path,
        profile_id: selection.profile.profile_id().as_str().to_string(),
        generation_id: selection.profile.plugin_generation_id(),
        profile: selection.profile,
    }
}

fn same_transport(active: &TrackedTransport, selection: &AdminTransportSelection) -> bool {
    active.transport_config_path == selection.transport_config_path
        && active.profile_id == selection.profile.profile_id().as_str()
        && active.generation_id == selection.profile.plugin_generation_id()
}

impl AdminTransportHostContext {
    fn host_api(self: &Arc<Self>) -> AdminTransportHostApiV1 {
        AdminTransportHostApiV1 {
            abi: mc_plugin_api::abi::CURRENT_PLUGIN_ABI,
            context: Arc::as_ptr(self) as *mut c_void,
            log: None,
            get_status: Some(host_get_status),
            list_sessions: Some(host_list_sessions),
            reload_runtime: Some(host_reload_runtime),
            upgrade_runtime: Some(host_upgrade_runtime),
            shutdown: Some(host_shutdown),
            publish_tcp_listener_for_upgrade: Some(host_publish_tcp_listener_for_upgrade),
            take_tcp_listener_from_upgrade: Some(host_take_tcp_listener_from_upgrade),
        }
    }

    fn take_published_listener(&self) -> Option<std::net::TcpListener> {
        self.published_listener
            .lock()
            .expect("published listener mutex should not be poisoned")
            .take()
    }

    fn install_child_resume_listener(&self, listener: std::net::TcpListener) {
        *self
            .child_resume_listener
            .lock()
            .expect("child resume listener mutex should not be poisoned") = Some(listener);
    }

    fn schedule_reconcile(&self) {
        let _ = self
            .surface_control_tx
            .try_send(ProcessSurfaceCommand::ReconcileAdminTransport);
    }
}

unsafe fn context_from_ptr<'a>(context: *mut c_void) -> &'a AdminTransportHostContext {
    unsafe { &*(context.cast::<AdminTransportHostContext>()) }
}

unsafe fn utf8_slice_to_str(slice: Utf8Slice) -> Result<String, String> {
    let bytes = unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) };
    std::str::from_utf8(bytes)
        .map(|value| value.to_string())
        .map_err(|_| "invalid utf-8".to_string())
}

fn block_on_control_plane<F, T>(
    runtime_handle: &tokio::runtime::Handle,
    future: F,
) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| runtime_handle.block_on(future))
    } else {
        runtime_handle.block_on(future)
    }
}

async fn resolve_subject(
    control_plane: &AdminControlPlaneHandle,
    principal_id: &str,
) -> Result<AdminSubject, String> {
    control_plane
        .subject_for_remote_principal(principal_id)
        .await
        .map_err(|error| error.to_string())
}

fn map_command_error(error: AdminCommandError) -> String {
    error.to_string()
}

fn write_buffer(output: *mut OwnedBuffer, bytes: Vec<u8>) {
    if output.is_null() {
        return;
    }
    let mut bytes = bytes;
    unsafe {
        *output = OwnedBuffer {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            cap: bytes.capacity(),
        };
    }
    std::mem::forget(bytes);
}

fn write_json<T: serde::Serialize>(
    output: *mut OwnedBuffer,
    value: &T,
) -> Result<(), PluginErrorCode> {
    let bytes = serde_json::to_vec(value).map_err(|_| PluginErrorCode::Internal)?;
    write_buffer(output, bytes);
    Ok(())
}

fn write_error(error_out: *mut OwnedBuffer, message: String) {
    write_buffer(error_out, message.into_bytes());
}

fn decode_reload_mode(mode: u8) -> Result<RuntimeReloadMode, String> {
    mc_plugin_api::codec::admin_transport::decode_reload_mode(mode)
        .map(|mode| match mode {
            mc_plugin_api::codec::admin_ui::RuntimeReloadMode::Artifacts => {
                RuntimeReloadMode::Artifacts
            }
            mc_plugin_api::codec::admin_ui::RuntimeReloadMode::Topology => {
                RuntimeReloadMode::Topology
            }
            mc_plugin_api::codec::admin_ui::RuntimeReloadMode::Core => RuntimeReloadMode::Core,
            mc_plugin_api::codec::admin_ui::RuntimeReloadMode::Full => RuntimeReloadMode::Full,
        })
        .map_err(|error| error.to_string())
}

unsafe extern "C" fn host_get_status(
    context: *mut c_void,
    principal_id: Utf8Slice,
    output: *mut OwnedBuffer,
    error: *mut OwnedBuffer,
) -> PluginErrorCode {
    host_json_callback(
        context,
        principal_id,
        output,
        error,
        |control_plane, subject| async move {
            control_plane
                .status(&subject)
                .await
                .map_err(map_command_error)
        },
    )
}

unsafe extern "C" fn host_list_sessions(
    context: *mut c_void,
    principal_id: Utf8Slice,
    output: *mut OwnedBuffer,
    error: *mut OwnedBuffer,
) -> PluginErrorCode {
    host_json_callback(
        context,
        principal_id,
        output,
        error,
        |control_plane, subject| async move {
            control_plane
                .sessions(&subject)
                .await
                .map_err(map_command_error)
        },
    )
}

unsafe extern "C" fn host_reload_runtime(
    context: *mut c_void,
    principal_id: Utf8Slice,
    mode: u8,
    output: *mut OwnedBuffer,
    error: *mut OwnedBuffer,
) -> PluginErrorCode {
    let context = unsafe { context_from_ptr(context) };
    let control_plane = context.control_plane.clone();
    let runtime_handle = context.runtime_handle.clone();
    let principal_id = match unsafe { utf8_slice_to_str(principal_id) } {
        Ok(value) => value,
        Err(message) => {
            write_error(error, message);
            return PluginErrorCode::InvalidInput;
        }
    };
    let mode = match decode_reload_mode(mode) {
        Ok(mode) => mode,
        Err(message) => {
            write_error(error, message);
            return PluginErrorCode::InvalidInput;
        }
    };
    let result = block_on_control_plane(&runtime_handle, async move {
        let subject = resolve_subject(&control_plane, &principal_id).await?;
        let result = control_plane
            .reload_runtime(&subject, mode)
            .await
            .map_err(map_command_error)?;
        Ok(result)
    });
    match result {
        Ok(value) => {
            context.schedule_reconcile();
            if write_json(output, &value).is_ok() {
                PluginErrorCode::Ok
            } else {
                write_error(error, "failed to encode reload response".to_string());
                PluginErrorCode::Internal
            }
        }
        Err(message) => {
            write_error(error, message);
            PluginErrorCode::InvalidInput
        }
    }
}

unsafe extern "C" fn host_upgrade_runtime(
    context: *mut c_void,
    principal_id: Utf8Slice,
    executable_path: Utf8Slice,
    output: *mut OwnedBuffer,
    error: *mut OwnedBuffer,
) -> PluginErrorCode {
    let context = unsafe { context_from_ptr(context) };
    let control_plane = context.control_plane.clone();
    let runtime_handle = context.runtime_handle.clone();
    let principal_id = match unsafe { utf8_slice_to_str(principal_id) } {
        Ok(value) => value,
        Err(message) => {
            write_error(error, message);
            return PluginErrorCode::InvalidInput;
        }
    };
    let executable_path = match unsafe { utf8_slice_to_str(executable_path) } {
        Ok(value) => value,
        Err(message) => {
            write_error(error, message);
            return PluginErrorCode::InvalidInput;
        }
    };
    let result = block_on_control_plane(&runtime_handle, async move {
        let subject = resolve_subject(&control_plane, &principal_id).await?;
        control_plane
            .upgrade_runtime(&subject, executable_path)
            .await
            .map_err(map_command_error)
    });
    match result {
        Ok(value) => {
            if write_json(output, &value).is_ok() {
                PluginErrorCode::Ok
            } else {
                write_error(error, "failed to encode upgrade response".to_string());
                PluginErrorCode::Internal
            }
        }
        Err(message) => {
            write_error(error, message);
            PluginErrorCode::InvalidInput
        }
    }
}

unsafe extern "C" fn host_shutdown(
    context: *mut c_void,
    principal_id: Utf8Slice,
    error: *mut OwnedBuffer,
) -> PluginErrorCode {
    let context = unsafe { context_from_ptr(context) };
    let control_plane = context.control_plane.clone();
    let runtime_handle = context.runtime_handle.clone();
    let principal_id = match unsafe { utf8_slice_to_str(principal_id) } {
        Ok(value) => value,
        Err(message) => {
            write_error(error, message);
            return PluginErrorCode::InvalidInput;
        }
    };
    let result = block_on_control_plane(&runtime_handle, async move {
        let subject = resolve_subject(&control_plane, &principal_id).await?;
        control_plane
            .shutdown(&subject)
            .await
            .map_err(map_command_error)
    });
    match result {
        Ok(()) => PluginErrorCode::Ok,
        Err(message) => {
            write_error(error, message);
            PluginErrorCode::InvalidInput
        }
    }
}

unsafe extern "C" fn host_publish_tcp_listener_for_upgrade(
    context: *mut c_void,
    raw_listener: usize,
    error: *mut OwnedBuffer,
) -> PluginErrorCode {
    let context = unsafe { context_from_ptr(context) };
    let listener = listener_from_raw(raw_listener);
    let mut slot = context
        .published_listener
        .lock()
        .expect("published listener mutex should not be poisoned");
    if slot.is_some() {
        drop(listener);
        write_error(
            error,
            "admin transport published more than one listener for upgrade".to_string(),
        );
        return PluginErrorCode::InvalidInput;
    }
    *slot = Some(listener);
    PluginErrorCode::Ok
}

unsafe extern "C" fn host_take_tcp_listener_from_upgrade(
    context: *mut c_void,
    present_out: *mut bool,
    raw_listener_out: *mut usize,
    error: *mut OwnedBuffer,
) -> PluginErrorCode {
    let context = unsafe { context_from_ptr(context) };
    let mut slot = context
        .child_resume_listener
        .lock()
        .expect("child resume listener mutex should not be poisoned");
    let Some(listener) = slot.take() else {
        if !present_out.is_null() {
            unsafe {
                *present_out = false;
            }
        }
        return PluginErrorCode::Ok;
    };
    if present_out.is_null() || raw_listener_out.is_null() {
        write_error(error, "missing listener output pointers".to_string());
        drop(listener);
        return PluginErrorCode::InvalidInput;
    }
    unsafe {
        *present_out = true;
        *raw_listener_out = listener_into_raw(listener);
    }
    PluginErrorCode::Ok
}

fn host_json_callback<F, Fut, T>(
    context: *mut c_void,
    principal_id: Utf8Slice,
    output: *mut OwnedBuffer,
    error: *mut OwnedBuffer,
    callback: F,
) -> PluginErrorCode
where
    F: FnOnce(AdminControlPlaneHandle, AdminSubject) -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
    T: serde::Serialize,
{
    let context = unsafe { context_from_ptr(context) };
    let control_plane = context.control_plane.clone();
    let runtime_handle = context.runtime_handle.clone();
    let principal_id = match unsafe { utf8_slice_to_str(principal_id) } {
        Ok(value) => value,
        Err(message) => {
            write_error(error, message);
            return PluginErrorCode::InvalidInput;
        }
    };
    let result = block_on_control_plane(&runtime_handle, async move {
        let subject = resolve_subject(&control_plane, &principal_id).await?;
        callback(control_plane, subject).await
    });
    match result {
        Ok(value) => match write_json(output, &value) {
            Ok(()) => PluginErrorCode::Ok,
            Err(code) => {
                write_error(error, "failed to encode host response".to_string());
                code
            }
        },
        Err(message) => {
            write_error(error, message);
            PluginErrorCode::InvalidInput
        }
    }
}

#[cfg(unix)]
fn listener_into_raw(listener: std::net::TcpListener) -> usize {
    listener.into_raw_fd() as usize
}

#[cfg(windows)]
fn listener_into_raw(listener: std::net::TcpListener) -> usize {
    listener.into_raw_socket() as usize
}

#[cfg(unix)]
fn listener_from_raw(raw: usize) -> std::net::TcpListener {
    unsafe { std::net::TcpListener::from_raw_fd(raw as RawFd) }
}

#[cfg(windows)]
fn listener_from_raw(raw: usize) -> std::net::TcpListener {
    unsafe { std::net::TcpListener::from_raw_socket(raw as RawSocket) }
}
