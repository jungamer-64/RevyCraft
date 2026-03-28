use crate::process_surfaces::{
    PausedAdminSurfaceInstance, PausedAdminSurfaceResource, ProcessSurfaceCommand,
};
use mc_plugin_api::abi::{ByteSlice, OwnedBuffer, PluginErrorCode, Utf8Slice};
use mc_plugin_api::codec::admin as plugin_admin;
use mc_plugin_api::codec::admin_surface::AdminSurfaceResource;
use mc_plugin_api::host_api::AdminSurfaceHostApiV1;
use revy_server_runtime::RuntimeError;
use revy_server_runtime::runtime::{AdminControlPlaneHandle, ServerSupervisor};
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[cfg(unix)]
use std::os::fd::RawFd;

pub(crate) struct AdminSurfaceSupervisor {
    server: Arc<ServerSupervisor>,
    shared_context: Arc<SharedAdminSurfaceHostContext>,
    state: SupervisorState,
}

enum SupervisorState {
    Inactive,
    Active(HashMap<String, TrackedSurface>),
    Paused(HashMap<String, TrackedSurface>),
}

struct TrackedSurface {
    instance_id: String,
    surface_config_path: Option<PathBuf>,
    profile: Arc<dyn mc_plugin_host::runtime::AdminSurfaceProfileHandle>,
    profile_id: String,
    host_context: Arc<InstanceAdminSurfaceHostContext>,
}

struct SharedAdminSurfaceHostContext {
    control_plane: AdminControlPlaneHandle,
    runtime_handle: tokio::runtime::Handle,
    surface_control_tx: mpsc::Sender<ProcessSurfaceCommand>,
    process_resources: Mutex<HashMap<String, AdminSurfaceResource>>,
    handoff_resources: Mutex<HashMap<String, HashMap<String, AdminSurfaceResource>>>,
}

struct InstanceAdminSurfaceHostContext {
    shared: Arc<SharedAdminSurfaceHostContext>,
    instance_id: String,
}

impl AdminSurfaceSupervisor {
    pub(crate) fn new(
        server: Arc<ServerSupervisor>,
        control_plane: AdminControlPlaneHandle,
        surface_control_tx: mpsc::Sender<ProcessSurfaceCommand>,
    ) -> Self {
        Self {
            server,
            shared_context: Arc::new(SharedAdminSurfaceHostContext {
                control_plane,
                runtime_handle: tokio::runtime::Handle::current(),
                surface_control_tx,
                process_resources: Mutex::new(init_process_resources()),
                handoff_resources: Mutex::new(HashMap::new()),
            }),
            state: SupervisorState::Inactive,
        }
    }

    pub(crate) async fn reconcile(&mut self) -> Result<(), RuntimeError> {
        let desired = self.server.current_admin_surfaces().await;
        match &mut self.state {
            SupervisorState::Paused(_) => Err(RuntimeError::Config(
                "cannot reconcile admin surfaces while they are paused for upgrade".to_string(),
            )),
            SupervisorState::Inactive => {
                let mut active = HashMap::new();
                for selection in desired {
                    start_surface(&self.shared_context, &mut active, selection)?;
                }
                self.state = if active.is_empty() {
                    SupervisorState::Inactive
                } else {
                    SupervisorState::Active(active)
                };
                Ok(())
            }
            SupervisorState::Active(active) => {
                let mut next = HashMap::new();
                for selection in desired {
                    let key = selection.instance_id.clone();
                    if let Some(current) = active.remove(&key)
                        && same_surface(&current, &selection)
                    {
                        next.insert(key, current);
                        continue;
                    }
                    if let Some(previous) = active.remove(&key) {
                        shutdown_surface(previous)?;
                    }
                    start_surface(&self.shared_context, &mut next, selection)?;
                }
                for (_, obsolete) in active.drain() {
                    shutdown_surface(obsolete)?;
                }
                self.state = if next.is_empty() {
                    SupervisorState::Inactive
                } else {
                    SupervisorState::Active(next)
                };
                Ok(())
            }
        }
    }

    pub(crate) async fn pause_for_upgrade(
        &mut self,
    ) -> Result<Vec<PausedAdminSurfaceInstance>, RuntimeError> {
        let active = match std::mem::replace(&mut self.state, SupervisorState::Inactive) {
            SupervisorState::Inactive => return Ok(Vec::new()),
            SupervisorState::Paused(active) => {
                self.state = SupervisorState::Paused(active);
                return Err(RuntimeError::Config(
                    "admin surfaces are already paused for upgrade".to_string(),
                ));
            }
            SupervisorState::Active(active) => active,
        };

        let mut instance_ids = active.keys().cloned().collect::<Vec<_>>();
        instance_ids.sort();
        let mut paused_instances = Vec::with_capacity(instance_ids.len());
        for instance_id in instance_ids {
            let tracked = active
                .get(&instance_id)
                .expect("tracked surface should exist while pausing for upgrade");
            let pause = tracked
                .profile
                .pause_for_upgrade(&tracked.instance_id, tracked.host_context.host_api())?;
            let handoff_resources = tracked
                .host_context
                .shared
                .take_instance_handoff_resources(&tracked.instance_id)
                .into_iter()
                .map(|(name, resource)| PausedAdminSurfaceResource { name, resource })
                .collect();
            paused_instances.push(PausedAdminSurfaceInstance {
                instance_id,
                resume_payload: pause.resume_payload,
                handoff_resources,
            });
        }

        self.state = SupervisorState::Paused(active);
        Ok(paused_instances)
    }

    pub(crate) async fn resume_from_upgrade(
        &mut self,
        paused_instances: Vec<PausedAdminSurfaceInstance>,
    ) -> Result<(), RuntimeError> {
        if !matches!(self.state, SupervisorState::Inactive) {
            return Err(RuntimeError::Config(
                "cannot resume admin surfaces while they are already active".to_string(),
            ));
        }

        let desired = self.server.current_admin_surfaces().await;
        let mut desired_by_instance = desired
            .into_iter()
            .map(|selection| (selection.instance_id.clone(), selection))
            .collect::<BTreeMap<_, _>>();
        self.shared_context
            .restore_handoff_resources(paused_instances.iter().map(|paused| {
                (
                    paused.instance_id.clone(),
                    paused
                        .handoff_resources
                        .iter()
                        .map(|resource| (resource.name.clone(), resource.resource.clone()))
                        .collect::<HashMap<_, _>>(),
                )
            }));

        let mut active = HashMap::new();
        for paused in paused_instances {
            let Some(selection) = desired_by_instance.remove(&paused.instance_id) else {
                return Err(RuntimeError::Config(format!(
                    "upgrade child did not have an active admin surface selection for `{}`",
                    paused.instance_id
                )));
            };
            resume_surface(
                &self.shared_context,
                &mut active,
                selection,
                paused.resume_payload,
            )?;
        }
        for (_, selection) in desired_by_instance {
            start_surface(&self.shared_context, &mut active, selection)?;
        }

        self.state = if active.is_empty() {
            SupervisorState::Inactive
        } else {
            SupervisorState::Active(active)
        };
        Ok(())
    }

    pub(crate) fn activate_after_upgrade_commit(&mut self) -> Result<(), RuntimeError> {
        let active = match &self.state {
            SupervisorState::Inactive => return Ok(()),
            SupervisorState::Paused(_) => {
                return Err(RuntimeError::Config(
                    "cannot activate admin surfaces while they are paused for upgrade".to_string(),
                ));
            }
            SupervisorState::Active(active) => active,
        };
        for tracked in active.values() {
            tracked.profile.activate_after_upgrade_commit(
                &tracked.instance_id,
                tracked.host_context.host_api(),
            )?;
        }
        Ok(())
    }

    pub(crate) fn resume_after_upgrade_rollback(
        &mut self,
        paused_instances: Vec<PausedAdminSurfaceInstance>,
    ) -> Result<(), RuntimeError> {
        let paused = match std::mem::replace(&mut self.state, SupervisorState::Inactive) {
            SupervisorState::Inactive => return Ok(()),
            SupervisorState::Active(active) => {
                self.state = SupervisorState::Active(active);
                return Ok(());
            }
            SupervisorState::Paused(paused) => paused,
        };
        self.shared_context
            .restore_handoff_resources(paused_instances.into_iter().map(|paused| {
                (
                    paused.instance_id,
                    paused
                        .handoff_resources
                        .into_iter()
                        .map(|resource| (resource.name, resource.resource))
                        .collect::<HashMap<_, _>>(),
                )
            }));
        for tracked in paused.values() {
            let _status = tracked.profile.resume_after_upgrade_rollback(
                &tracked.instance_id,
                tracked.host_context.host_api(),
            )?;
        }
        self.state = SupervisorState::Active(paused);
        Ok(())
    }

    pub(crate) fn shutdown_current(&mut self) -> Result<(), RuntimeError> {
        match std::mem::replace(&mut self.state, SupervisorState::Inactive) {
            SupervisorState::Inactive => Ok(()),
            SupervisorState::Active(active) | SupervisorState::Paused(active) => {
                for (_, tracked) in active {
                    shutdown_surface(tracked)?;
                }
                Ok(())
            }
        }
    }
}

fn start_surface(
    shared: &Arc<SharedAdminSurfaceHostContext>,
    active: &mut HashMap<String, TrackedSurface>,
    selection: revy_server_runtime::runtime::AdminSurfaceSelection,
) -> Result<(), RuntimeError> {
    let host_context = Arc::new(InstanceAdminSurfaceHostContext {
        shared: Arc::clone(shared),
        instance_id: selection.instance_id.clone(),
    });
    let _status = selection.profile.start(
        &selection.instance_id,
        selection.surface_config_path.as_deref(),
        host_context.host_api(),
    )?;
    active.insert(
        selection.instance_id.clone(),
        TrackedSurface {
            instance_id: selection.instance_id,
            surface_config_path: selection.surface_config_path,
            profile_id: selection.profile.profile_id().as_str().to_string(),
            profile: selection.profile,
            host_context,
        },
    );
    Ok(())
}

fn resume_surface(
    shared: &Arc<SharedAdminSurfaceHostContext>,
    active: &mut HashMap<String, TrackedSurface>,
    selection: revy_server_runtime::runtime::AdminSurfaceSelection,
    resume_payload: Vec<u8>,
) -> Result<(), RuntimeError> {
    let host_context = Arc::new(InstanceAdminSurfaceHostContext {
        shared: Arc::clone(shared),
        instance_id: selection.instance_id.clone(),
    });
    let _status = selection.profile.resume_from_upgrade(
        &selection.instance_id,
        selection.surface_config_path.as_deref(),
        &resume_payload,
        host_context.host_api(),
    )?;
    active.insert(
        selection.instance_id.clone(),
        TrackedSurface {
            instance_id: selection.instance_id,
            surface_config_path: selection.surface_config_path,
            profile_id: selection.profile.profile_id().as_str().to_string(),
            profile: selection.profile,
            host_context,
        },
    );
    Ok(())
}

fn shutdown_surface(tracked: TrackedSurface) -> Result<(), RuntimeError> {
    tracked
        .profile
        .shutdown(&tracked.instance_id, tracked.host_context.host_api())?;
    Ok(())
}

fn same_surface(
    tracked: &TrackedSurface,
    selection: &revy_server_runtime::runtime::AdminSurfaceSelection,
) -> bool {
    tracked.surface_config_path == selection.surface_config_path
        && tracked.profile_id == selection.profile.profile_id().as_str()
}

impl InstanceAdminSurfaceHostContext {
    fn host_api(self: &Arc<Self>) -> AdminSurfaceHostApiV1 {
        AdminSurfaceHostApiV1 {
            abi: mc_plugin_api::abi::CURRENT_PLUGIN_ABI,
            context: Arc::as_ptr(self) as *mut c_void,
            log: Some(host_log),
            execute: Some(host_execute),
            permissions: Some(host_permissions),
            take_process_resource: Some(host_take_process_resource),
            publish_handoff_resource: Some(host_publish_handoff_resource),
            take_handoff_resource: Some(host_take_handoff_resource),
        }
    }
}

impl SharedAdminSurfaceHostContext {
    fn schedule_reconcile(&self) {
        let _ = self
            .surface_control_tx
            .try_send(ProcessSurfaceCommand::ReconcileAdminSurfaces);
    }

    fn take_instance_handoff_resources(
        &self,
        instance_id: &str,
    ) -> HashMap<String, AdminSurfaceResource> {
        self.handoff_resources
            .lock()
            .expect("admin surface handoff resources mutex should not be poisoned")
            .remove(instance_id)
            .unwrap_or_default()
    }

    fn restore_handoff_resources<I>(&self, resources: I)
    where
        I: IntoIterator<Item = (String, HashMap<String, AdminSurfaceResource>)>,
    {
        let mut handoff_resources = self
            .handoff_resources
            .lock()
            .expect("admin surface handoff resources mutex should not be poisoned");
        handoff_resources.clear();
        handoff_resources.extend(resources);
    }
}

unsafe fn context_from_ptr<'a>(context: *mut c_void) -> &'a InstanceAdminSurfaceHostContext {
    unsafe { &*(context.cast::<InstanceAdminSurfaceHostContext>()) }
}

unsafe fn utf8_slice_to_string(slice: Utf8Slice) -> Result<String, String> {
    let bytes = unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) };
    std::str::from_utf8(bytes)
        .map(|value| value.to_string())
        .map_err(|_| "invalid utf-8".to_string())
}

unsafe fn byte_slice_to_vec(slice: ByteSlice) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) }.to_vec()
}

unsafe extern "C" fn host_log(level: u32, message: Utf8Slice) {
    if let Ok(message) = unsafe { utf8_slice_to_string(message) } {
        eprintln!("admin-surface[{level}]: {message}");
    }
}

unsafe extern "C" fn host_execute(
    context: *mut c_void,
    principal_id: Utf8Slice,
    request: ByteSlice,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let context = unsafe { context_from_ptr(context) };
    let principal_id = match unsafe { utf8_slice_to_string(principal_id) } {
        Ok(principal_id) => principal_id,
        Err(error) => {
            write_error_buffer(error_out, error);
            return PluginErrorCode::Internal;
        }
    };
    let request = match serde_json::from_slice::<plugin_admin::AdminRequest>(&unsafe {
        byte_slice_to_vec(request)
    }) {
        Ok(request) => request,
        Err(error) => {
            write_error_buffer(
                error_out,
                format!("failed to decode admin request: {error}"),
            );
            return PluginErrorCode::Internal;
        }
    };
    let response = match context.shared.runtime_handle.block_on(
        context
            .shared
            .control_plane
            .execute_surface(&principal_id, request),
    ) {
        response => response,
    };
    if matches!(response, plugin_admin::AdminResponse::ReloadRuntime(_)) {
        context.shared.schedule_reconcile();
    }
    match serde_json::to_vec(&response) {
        Ok(bytes) => {
            write_owned_buffer(output, bytes);
            PluginErrorCode::Ok
        }
        Err(error) => {
            write_error_buffer(
                error_out,
                format!("failed to encode admin response: {error}"),
            );
            PluginErrorCode::Internal
        }
    }
}

unsafe extern "C" fn host_permissions(
    context: *mut c_void,
    principal_id: Utf8Slice,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let context = unsafe { context_from_ptr(context) };
    let principal_id = match unsafe { utf8_slice_to_string(principal_id) } {
        Ok(principal_id) => principal_id,
        Err(error) => {
            write_error_buffer(error_out, error);
            return PluginErrorCode::Internal;
        }
    };
    let permissions = match context.shared.runtime_handle.block_on(
        context
            .shared
            .control_plane
            .surface_permissions_for_principal(&principal_id),
    ) {
        Ok(permissions) => permissions,
        Err(error) => {
            write_error_buffer(error_out, error.to_string());
            return PluginErrorCode::Internal;
        }
    };
    match serde_json::to_vec(&permissions) {
        Ok(bytes) => {
            write_owned_buffer(output, bytes);
            PluginErrorCode::Ok
        }
        Err(error) => {
            write_error_buffer(
                error_out,
                format!("failed to encode admin permissions: {error}"),
            );
            PluginErrorCode::Internal
        }
    }
}

unsafe extern "C" fn host_take_process_resource(
    context: *mut c_void,
    name: Utf8Slice,
    present_out: *mut bool,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let context = unsafe { context_from_ptr(context) };
    let name = match unsafe { utf8_slice_to_string(name) } {
        Ok(name) => name,
        Err(error) => {
            write_error_buffer(error_out, error);
            return PluginErrorCode::Internal;
        }
    };
    let resource = context
        .shared
        .process_resources
        .lock()
        .expect("admin surface process resources mutex should not be poisoned")
        .remove(&name);
    unsafe {
        if !present_out.is_null() {
            *present_out = resource.is_some();
        }
    }
    let Some(resource) = resource else {
        return PluginErrorCode::Ok;
    };
    match serde_json::to_vec(&resource) {
        Ok(bytes) => {
            write_owned_buffer(output, bytes);
            PluginErrorCode::Ok
        }
        Err(error) => {
            write_error_buffer(
                error_out,
                format!("failed to encode process resource: {error}"),
            );
            PluginErrorCode::Internal
        }
    }
}

unsafe extern "C" fn host_publish_handoff_resource(
    context: *mut c_void,
    name: Utf8Slice,
    resource: ByteSlice,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let context = unsafe { context_from_ptr(context) };
    let name = match unsafe { utf8_slice_to_string(name) } {
        Ok(name) => name,
        Err(error) => {
            write_error_buffer(error_out, error);
            return PluginErrorCode::Internal;
        }
    };
    let resource = match serde_json::from_slice::<AdminSurfaceResource>(&unsafe {
        byte_slice_to_vec(resource)
    }) {
        Ok(resource) => resource,
        Err(error) => {
            write_error_buffer(
                error_out,
                format!("failed to decode handoff resource: {error}"),
            );
            return PluginErrorCode::Internal;
        }
    };
    context
        .shared
        .handoff_resources
        .lock()
        .expect("admin surface handoff resources mutex should not be poisoned")
        .entry(context.instance_id.clone())
        .or_default()
        .insert(name, resource);
    PluginErrorCode::Ok
}

unsafe extern "C" fn host_take_handoff_resource(
    context: *mut c_void,
    name: Utf8Slice,
    present_out: *mut bool,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let context = unsafe { context_from_ptr(context) };
    let name = match unsafe { utf8_slice_to_string(name) } {
        Ok(name) => name,
        Err(error) => {
            write_error_buffer(error_out, error);
            return PluginErrorCode::Internal;
        }
    };
    let resource = context
        .shared
        .handoff_resources
        .lock()
        .expect("admin surface handoff resources mutex should not be poisoned")
        .entry(context.instance_id.clone())
        .or_default()
        .remove(&name);
    unsafe {
        if !present_out.is_null() {
            *present_out = resource.is_some();
        }
    }
    let Some(resource) = resource else {
        return PluginErrorCode::Ok;
    };
    match serde_json::to_vec(&resource) {
        Ok(bytes) => {
            write_owned_buffer(output, bytes);
            PluginErrorCode::Ok
        }
        Err(error) => {
            write_error_buffer(
                error_out,
                format!("failed to encode handoff resource: {error}"),
            );
            PluginErrorCode::Internal
        }
    }
}

fn write_owned_buffer(output: *mut OwnedBuffer, mut bytes: Vec<u8>) {
    if output.is_null() {
        return;
    }
    unsafe {
        *output = OwnedBuffer {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            cap: bytes.capacity(),
        };
        std::mem::forget(bytes);
    }
}

fn write_error_buffer(error_out: *mut OwnedBuffer, message: String) {
    write_owned_buffer(error_out, message.into_bytes());
}

fn init_process_resources() -> HashMap<String, AdminSurfaceResource> {
    let mut resources = HashMap::new();
    #[cfg(unix)]
    {
        if let Ok(stdin_fd) = dup_fd(libc::STDIN_FILENO) {
            resources.insert("stdio.stdin".to_string(), fd_resource(stdin_fd));
        }
        if let Ok(stdout_fd) = dup_fd(libc::STDOUT_FILENO) {
            resources.insert("stdio.stdout".to_string(), fd_resource(stdout_fd));
        }
        if let Ok(stderr_fd) = dup_fd(libc::STDERR_FILENO) {
            resources.insert("stdio.stderr".to_string(), fd_resource(stderr_fd));
        }
    }
    resources
}

#[cfg(unix)]
fn dup_fd(fd: RawFd) -> Result<RawFd, std::io::Error> {
    let duplicated = unsafe { libc::dup(fd) };
    if duplicated < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(duplicated)
    }
}

#[cfg(unix)]
fn fd_resource(fd: RawFd) -> AdminSurfaceResource {
    AdminSurfaceResource::NativeHandle {
        handle_kind: "fd".to_string(),
        raw_handle: fd as u64,
    }
}
