#![allow(clippy::multiple_crate_versions)]

pub mod admin {
    tonic::include_proto!("revy.admin.v1");
}

use crate::admin::{
    self as proto, GetStatusResponse, ListSessionsResponse, ReloadRuntimeResponse,
    ShutdownResponse, UpgradeRuntimeResponse,
    admin_control_plane_server::{AdminControlPlane, AdminControlPlaneServer},
};
use mc_plugin_api::codec::admin::{
    self as surface_admin, AdminArtifactsReloadView, AdminFullReloadView, AdminNamedCountView,
    AdminRuntimeReloadDetail, AdminRuntimeReloadView, AdminSessionSummaryView, AdminSessionsView,
    AdminStatusView, AdminTopologyReloadView, AdminUpgradeRuntimeView, RuntimeReloadMode,
    RuntimeUpgradePhase, RuntimeUpgradeRole,
};
use mc_plugin_api::codec::admin_surface::{
    AdminSurfaceEndpointView, AdminSurfaceInstanceDeclaration, AdminSurfacePauseView,
    AdminSurfaceResource, AdminSurfaceStatusView,
};
use mc_plugin_sdk_rust::admin_surface::{
    AdminSurfaceHost, RustAdminSurfacePlugin, SdkAdminSurfaceHost,
};
use mc_plugin_sdk_rust::capabilities;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use revy_voxel_core::{AdminSurfaceCapability, AdminSurfaceCapabilitySet};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU8, Ordering},
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

#[cfg(unix)]
use std::os::fd::{FromRawFd, IntoRawFd};

#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, FromRawSocket, IntoRawSocket, RawSocket};

const MANIFEST: StaticPluginManifest =
    StaticPluginManifest::admin_surface("admin-grpc", "gRPC Admin Surface Plugin", "grpc-v1");

#[derive(Default)]
pub struct GrpcAdminSurfacePlugin {
    runtime: OnceLock<Result<tokio::runtime::Runtime, String>>,
    instances: Mutex<HashMap<String, GrpcSurfaceInstanceState>>,
}

enum GrpcSurfaceInstanceState {
    Active {
        config: LoadedGrpcSurfaceConfig,
        handle: AdminGrpcServerHandle,
    },
    Paused {
        config: LoadedGrpcSurfaceConfig,
        handle: AdminGrpcServerHandle,
    },
}

#[derive(Clone)]
struct AdminGrpcService {
    host: SdkAdminSurfaceHost,
    auth: ArcAuthMap,
    serve_mode: Arc<AtomicU8>,
}

type ArcAuthMap = Arc<HashMap<String, String>>;

enum ListenerSource {
    Bind(SocketAddr),
    Inherited(std::net::TcpListener),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AcceptLoopMode {
    Running,
    Paused,
    Shutdown,
}

impl AcceptLoopMode {
    const fn as_u8(self) -> u8 {
        match self {
            Self::Running => 1,
            Self::Paused => 2,
            Self::Shutdown => 3,
        }
    }
}

#[derive(Debug)]
struct AdminGrpcServerHandle {
    local_addr: SocketAddr,
    handoff_listener: Option<std::net::TcpListener>,
    serve_mode: Arc<AtomicU8>,
    accept_mode_tx: watch::Sender<AcceptLoopMode>,
    shutdown_tx: watch::Sender<bool>,
    server_join_handle: JoinHandle<()>,
    server_done_rx: oneshot::Receiver<Result<(), String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrpcSurfaceConfig {
    bind_addr: String,
    #[serde(default)]
    allow_non_loopback: bool,
    #[serde(default)]
    principals: HashMap<String, GrpcPrincipalConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrpcPrincipalConfig {
    token_file: PathBuf,
}

#[derive(Clone)]
struct LoadedGrpcSurfaceConfig {
    bind_addr: SocketAddr,
    token_principals: ArcAuthMap,
}

impl RustAdminSurfacePlugin for GrpcAdminSurfacePlugin {
    fn descriptor(&self) -> mc_plugin_api::codec::admin_surface::AdminSurfaceDescriptor {
        mc_plugin_sdk_rust::admin_surface::admin_surface_descriptor("grpc-v1")
    }

    fn capability_set(&self) -> AdminSurfaceCapabilitySet {
        capabilities::admin_surface_capabilities(&[AdminSurfaceCapability::RuntimeReload])
    }

    fn declare_instance(
        &self,
        _instance_id: &str,
        surface_config_path: Option<&str>,
    ) -> Result<AdminSurfaceInstanceDeclaration, String> {
        let _config = load_surface_config(surface_config_path)?;
        Ok(AdminSurfaceInstanceDeclaration {
            principals: Vec::new(),
            required_process_resources: Vec::new(),
            supports_upgrade_handoff: true,
        })
    }

    fn start(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost,
        surface_config_path: Option<&str>,
    ) -> Result<AdminSurfaceStatusView, String> {
        let config = load_surface_config(surface_config_path)?;
        let handle = self.block_on_async(spawn_admin_grpc_server(
            self.runtime_handle()?,
            &config,
            host,
            ListenerSource::Bind(config.bind_addr),
            AcceptLoopMode::Running,
        ))?;
        let status = handle.status_view();
        let mut instances = self
            .instances
            .lock()
            .expect("admin-surface grpc mutex should not be poisoned");
        ensure_instance_absent(&instances, instance_id)?;
        instances.insert(
            instance_id.to_string(),
            GrpcSurfaceInstanceState::Active { config, handle },
        );
        Ok(status)
    }

    fn pause_for_upgrade(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost,
    ) -> Result<AdminSurfacePauseView, String> {
        let state = self
            .instances
            .lock()
            .expect("admin-surface grpc mutex should not be poisoned")
            .remove(instance_id)
            .ok_or_else(|| format!("gRPC admin surface `{instance_id}` is not active"))?;
        let (config, handle) = match state {
            GrpcSurfaceInstanceState::Active { config, handle } => (config, handle),
            GrpcSurfaceInstanceState::Paused { config, handle } => {
                self.instances
                    .lock()
                    .expect("admin-surface grpc mutex should not be poisoned")
                    .insert(
                        instance_id.to_string(),
                        GrpcSurfaceInstanceState::Paused { config, handle },
                    );
                return Err(format!(
                    "gRPC admin surface `{instance_id}` is already paused"
                ));
            }
        };
        let mut handle = handle;
        let listener_resource =
            listener_resource_from_tcp_listener(handle.export_handoff_listener()?)?;
        host.publish_handoff_resource("listener", &listener_resource)?;
        self.instances
            .lock()
            .expect("admin-surface grpc mutex should not be poisoned")
            .insert(
                instance_id.to_string(),
                GrpcSurfaceInstanceState::Paused { config, handle },
            );
        Ok(AdminSurfacePauseView {
            resume_payload: Vec::new(),
        })
    }

    fn resume_from_upgrade(
        &self,
        instance_id: &str,
        host: SdkAdminSurfaceHost,
        surface_config_path: Option<&str>,
        _resume_payload: &[u8],
    ) -> Result<AdminSurfaceStatusView, String> {
        let config = load_surface_config(surface_config_path)?;
        let listener = take_handoff_listener(&host)?;
        let handle = self.block_on_async(spawn_admin_grpc_server(
            self.runtime_handle()?,
            &config,
            host,
            ListenerSource::Inherited(listener),
            AcceptLoopMode::Paused,
        ))?;
        let status = handle.status_view();
        let mut instances = self
            .instances
            .lock()
            .expect("admin-surface grpc mutex should not be poisoned");
        if let Some(previous) = instances.remove(instance_id) {
            match previous {
                GrpcSurfaceInstanceState::Active { .. } => {
                    return Err(format!(
                        "gRPC admin surface `{instance_id}` is already active"
                    ));
                }
                GrpcSurfaceInstanceState::Paused { .. } => {}
            }
        }
        instances.insert(
            instance_id.to_string(),
            GrpcSurfaceInstanceState::Active { config, handle },
        );
        Ok(status)
    }

    fn activate_after_upgrade_commit(
        &self,
        instance_id: &str,
        _host: SdkAdminSurfaceHost,
    ) -> Result<(), String> {
        let instances = self
            .instances
            .lock()
            .expect("admin-surface grpc mutex should not be poisoned");
        let Some(GrpcSurfaceInstanceState::Active { handle, .. }) = instances.get(instance_id)
        else {
            return Err(format!("gRPC admin surface `{instance_id}` is not active"));
        };
        handle.activate_after_upgrade_commit()
    }

    fn resume_after_upgrade_rollback(
        &self,
        instance_id: &str,
        _host: SdkAdminSurfaceHost,
    ) -> Result<AdminSurfaceStatusView, String> {
        let mut instances = self
            .instances
            .lock()
            .expect("admin-surface grpc mutex should not be poisoned");
        match instances.remove(instance_id) {
            Some(GrpcSurfaceInstanceState::Paused { config, handle }) => {
                handle.activate_after_upgrade_commit()?;
                let status = handle.status_view();
                instances.insert(
                    instance_id.to_string(),
                    GrpcSurfaceInstanceState::Active { config, handle },
                );
                Ok(status)
            }
            Some(state @ GrpcSurfaceInstanceState::Active { .. }) => {
                instances.insert(instance_id.to_string(), state);
                Err(format!(
                    "gRPC admin surface `{instance_id}` is already active"
                ))
            }
            None => Err(format!(
                "gRPC admin surface `{instance_id}` was not paused for upgrade"
            )),
        }
    }

    fn shutdown(&self, instance_id: &str, _host: SdkAdminSurfaceHost) -> Result<(), String> {
        let state = self
            .instances
            .lock()
            .expect("admin-surface grpc mutex should not be poisoned")
            .remove(instance_id);
        match state {
            None => Ok(()),
            Some(GrpcSurfaceInstanceState::Paused { handle, .. }) => {
                self.block_on_async(handle.join())
            }
            Some(GrpcSurfaceInstanceState::Active { handle, .. }) => {
                self.block_on_async(handle.join())
            }
        }
    }
}

impl GrpcAdminSurfacePlugin {
    fn runtime(&self) -> Result<&tokio::runtime::Runtime, String> {
        match self.runtime.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("admin-surface-grpc")
                .enable_all()
                .build()
                .map_err(|error| format!("failed to build admin gRPC runtime: {error}"))
        }) {
            Ok(runtime) => Ok(runtime),
            Err(error) => Err(error.clone()),
        }
    }

    fn runtime_handle(&self) -> Result<tokio::runtime::Handle, String> {
        Ok(self.runtime()?.handle().clone())
    }

    fn block_on_async<F, T>(&self, future: F) -> Result<T, String>
    where
        F: std::future::Future<Output = Result<T, String>>,
    {
        self.runtime()?.block_on(future)
    }
}

fn ensure_instance_absent(
    instances: &HashMap<String, GrpcSurfaceInstanceState>,
    instance_id: &str,
) -> Result<(), String> {
    if instances.contains_key(instance_id) {
        Err(format!(
            "gRPC admin surface `{instance_id}` is already running"
        ))
    } else {
        Ok(())
    }
}

fn load_surface_config(path: Option<&str>) -> Result<LoadedGrpcSurfaceConfig, String> {
    let path = path.ok_or_else(|| "gRPC admin surface requires a config path".to_string())?;
    let surface_config_path = PathBuf::from(path);
    let contents = fs::read_to_string(&surface_config_path).map_err(|error| {
        format!(
            "failed to read admin surface config {}: {error}",
            surface_config_path.display()
        )
    })?;
    let document: GrpcSurfaceConfig = toml::from_str(&contents).map_err(|error| {
        format!(
            "failed to parse admin surface config {}: {error}",
            surface_config_path.display()
        )
    })?;
    let bind_addr: SocketAddr = document.bind_addr.parse().map_err(|_| {
        format!(
            "invalid bind_addr `{}` in {}",
            document.bind_addr,
            surface_config_path.display()
        )
    })?;
    if !document.allow_non_loopback && !bind_addr.ip().is_loopback() {
        return Err(format!(
            "bind_addr `{bind_addr}` is non-loopback; set allow_non_loopback=true in {}",
            surface_config_path.display()
        ));
    }
    if document.principals.is_empty() {
        return Err(format!(
            "{} must define at least one principals.<id>.token_file entry",
            surface_config_path.display()
        ));
    }
    let base_dir = surface_config_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let mut token_principals = HashMap::new();
    let mut entries = document.principals.into_iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    for (principal_id, principal) in entries {
        let token_file = if principal.token_file.is_relative() {
            base_dir.join(principal.token_file)
        } else {
            principal.token_file
        };
        let token = fs::read_to_string(&token_file)
            .map_err(|error| {
                format!(
                    "failed to read token_file {}: {error}",
                    token_file.display()
                )
            })?
            .trim()
            .to_string();
        if token.is_empty() {
            return Err(format!(
                "token_file {} resolved to an empty token",
                token_file.display()
            ));
        }
        if let Some(previous) = token_principals.insert(token.clone(), principal_id.clone()) {
            return Err(format!(
                "principals `{previous}` and `{principal_id}` resolved to the same bearer token"
            ));
        }
    }
    Ok(LoadedGrpcSurfaceConfig {
        bind_addr,
        token_principals: Arc::new(token_principals),
    })
}

fn take_handoff_listener(host: &SdkAdminSurfaceHost) -> Result<std::net::TcpListener, String> {
    let resource = host.take_handoff_resource("listener")?.ok_or_else(|| {
        "gRPC admin surface did not receive a listener handoff resource".to_string()
    })?;
    tcp_listener_from_resource(resource)
}

fn listener_resource_from_tcp_listener(
    listener: std::net::TcpListener,
) -> Result<AdminSurfaceResource, String> {
    #[cfg(unix)]
    {
        let _ = listener
            .local_addr()
            .map_err(|error| format!("failed to inspect gRPC listener: {error}"))?;
        Ok(AdminSurfaceResource::NativeHandle {
            handle_kind: "tcp-listener".to_string(),
            raw_handle: u64::try_from(listener.into_raw_fd())
                .expect("unix listener fd should fit into u64"),
        })
    }

    #[cfg(windows)]
    {
        let _ = listener
            .local_addr()
            .map_err(|error| format!("failed to inspect gRPC listener: {error}"))?;
        Ok(AdminSurfaceResource::NativeHandle {
            handle_kind: "tcp-listener".to_string(),
            raw_handle: listener.into_raw_socket() as u64,
        })
    }
}

fn tcp_listener_from_resource(
    resource: AdminSurfaceResource,
) -> Result<std::net::TcpListener, String> {
    match resource {
        AdminSurfaceResource::NativeHandle {
            handle_kind,
            raw_handle,
        } if handle_kind == "tcp-listener" => {
            #[cfg(unix)]
            {
                let raw_fd = i32::try_from(raw_handle).map_err(|_| {
                    format!("listener handoff fd `{raw_handle}` did not fit into i32")
                })?;
                return Ok(unsafe { std::net::TcpListener::from_raw_fd(raw_fd) });
            }

            #[cfg(windows)]
            {
                return Ok(unsafe {
                    std::net::TcpListener::from_raw_socket(raw_handle as RawSocket)
                });
            }
        }
        AdminSurfaceResource::Bytes(_) => {
            Err("gRPC admin surface expected a native listener handoff resource".to_string())
        }
        AdminSurfaceResource::NativeHandle { handle_kind, .. } => Err(format!(
            "gRPC admin surface expected a tcp-listener handoff, got `{handle_kind}`"
        )),
    }
}

#[tonic::async_trait]
impl AdminControlPlane for AdminGrpcService {
    async fn get_status(
        &self,
        request: Request<proto::GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        let principal_id = authenticate_request(&self.auth, request.metadata())?;
        let response = self
            .host
            .execute(&principal_id, &surface_admin::AdminRequest::Status)
            .map_err(map_host_callback_error)?;
        match response {
            surface_admin::AdminResponse::Status(status) => Ok(Response::new(GetStatusResponse {
                status: Some(map_status_view(status)),
            })),
            other => Err(map_surface_response_error(other)),
        }
    }

    async fn list_sessions(
        &self,
        request: Request<proto::ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let principal_id = authenticate_request(&self.auth, request.metadata())?;
        let response = self
            .host
            .execute(&principal_id, &surface_admin::AdminRequest::Sessions)
            .map_err(map_host_callback_error)?;
        match response {
            surface_admin::AdminResponse::Sessions(sessions) => {
                Ok(Response::new(ListSessionsResponse {
                    sessions: Some(map_sessions_view(sessions)),
                }))
            }
            other => Err(map_surface_response_error(other)),
        }
    }

    async fn reload_runtime(
        &self,
        request: Request<proto::ReloadRuntimeRequest>,
    ) -> Result<Response<ReloadRuntimeResponse>, Status> {
        reject_mutating_request_while_paused(&self.serve_mode, "reload runtime")?;
        let principal_id = authenticate_request(&self.auth, request.metadata())?;
        let mode = map_reload_mode_request(request.into_inner().mode)?;
        let response = self
            .host
            .execute(
                &principal_id,
                &surface_admin::AdminRequest::ReloadRuntime { mode },
            )
            .map_err(map_host_callback_error)?;
        match response {
            surface_admin::AdminResponse::ReloadRuntime(result) => {
                Ok(Response::new(ReloadRuntimeResponse {
                    result: Some(map_runtime_reload_view(result)),
                }))
            }
            other => Err(map_surface_response_error(other)),
        }
    }

    async fn upgrade_runtime(
        &self,
        request: Request<proto::UpgradeRuntimeRequest>,
    ) -> Result<Response<UpgradeRuntimeResponse>, Status> {
        reject_mutating_request_while_paused(&self.serve_mode, "upgrade runtime")?;
        let principal_id = authenticate_request(&self.auth, request.metadata())?;
        let executable_path = request.into_inner().executable_path;
        let response = self
            .host
            .execute(
                &principal_id,
                &surface_admin::AdminRequest::UpgradeRuntime { executable_path },
            )
            .map_err(map_host_callback_error)?;
        match response {
            surface_admin::AdminResponse::UpgradeRuntime(result) => {
                Ok(Response::new(UpgradeRuntimeResponse {
                    result: Some(map_upgrade_runtime_view(result)),
                }))
            }
            other => Err(map_surface_response_error(other)),
        }
    }

    async fn shutdown(
        &self,
        request: Request<proto::ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        reject_mutating_request_while_paused(&self.serve_mode, "shutdown")?;
        let principal_id = authenticate_request(&self.auth, request.metadata())?;
        let response = self
            .host
            .execute(&principal_id, &surface_admin::AdminRequest::Shutdown)
            .map_err(map_host_callback_error)?;
        match response {
            surface_admin::AdminResponse::ShutdownScheduled => {
                Ok(Response::new(ShutdownResponse {}))
            }
            other => Err(map_surface_response_error(other)),
        }
    }
}

async fn spawn_admin_grpc_server(
    runtime_handle: tokio::runtime::Handle,
    config: &LoadedGrpcSurfaceConfig,
    host: SdkAdminSurfaceHost,
    listener_source: ListenerSource,
    initial_accept_mode: AcceptLoopMode,
) -> Result<AdminGrpcServerHandle, String> {
    let handoff_listener = match listener_source {
        ListenerSource::Bind(bind_addr) => {
            std::net::TcpListener::bind(bind_addr).map_err(|error| {
                format!("failed to bind admin gRPC listener on {bind_addr}: {error}")
            })?
        }
        ListenerSource::Inherited(listener) => listener,
    };
    let local_addr = handoff_listener
        .local_addr()
        .map_err(|error| error.to_string())?;
    let listener = handoff_listener
        .try_clone()
        .map_err(|error| format!("failed to duplicate admin gRPC listener: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let listener = TcpListener::from_std(listener).map_err(|error| error.to_string())?;
    let (accept_mode_tx, accept_mode_rx) = watch::channel(initial_accept_mode);
    let serve_mode = Arc::new(AtomicU8::new(initial_accept_mode.as_u8()));
    let (incoming_tx, incoming_rx) = mpsc::channel::<Result<TcpStream, std::io::Error>>(32);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let service = AdminGrpcService {
        host,
        auth: Arc::clone(&config.token_principals),
        serve_mode: Arc::clone(&serve_mode),
    };
    runtime_handle.spawn(run_accept_loop(listener, incoming_tx, accept_mode_rx));
    let (server_done_tx, server_done_rx) = oneshot::channel();
    let server_join_handle = runtime_handle.spawn(async move {
        let result = tonic::transport::Server::builder()
            .add_service(AdminControlPlaneServer::new(service))
            .serve_with_incoming_shutdown(
                ReceiverStream::new(incoming_rx),
                wait_for_shutdown_signal(shutdown_rx),
            )
            .await
            .map_err(|error| format!("admin gRPC server failed: {error}"));
        let _ = server_done_tx.send(result);
    });
    Ok(AdminGrpcServerHandle {
        local_addr,
        handoff_listener: Some(handoff_listener),
        serve_mode,
        accept_mode_tx,
        shutdown_tx,
        server_join_handle,
        server_done_rx,
    })
}

async fn run_accept_loop(
    listener: TcpListener,
    incoming_tx: mpsc::Sender<Result<TcpStream, std::io::Error>>,
    mut accept_mode_rx: watch::Receiver<AcceptLoopMode>,
) {
    loop {
        let mode = *accept_mode_rx.borrow_and_update();
        match mode {
            AcceptLoopMode::Running => {}
            AcceptLoopMode::Paused => {
                if accept_mode_rx.changed().await.is_err() {
                    break;
                }
                continue;
            }
            AcceptLoopMode::Shutdown => break,
        }
        tokio::select! {
            changed = accept_mode_rx.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _addr)) => {
                        if incoming_tx.send(Ok(stream)).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = incoming_tx.send(Err(error)).await;
                        break;
                    }
                }
            }
        }
    }
}

impl AdminGrpcServerHandle {
    fn status_view(&self) -> AdminSurfaceStatusView {
        AdminSurfaceStatusView {
            endpoints: vec![AdminSurfaceEndpointView {
                surface: "grpc".to_string(),
                local_addr: self.local_addr.to_string(),
            }],
        }
    }

    async fn wait_for_server_exit(mut self) -> Result<(), String> {
        let result = (&mut self.server_done_rx)
            .await
            .map_err(|_| "admin gRPC server task ended unexpectedly".to_string())?;
        self.server_join_handle.abort();
        result
    }

    fn export_handoff_listener(&mut self) -> Result<std::net::TcpListener, String> {
        let listener = self.handoff_listener.take().ok_or_else(|| {
            "admin gRPC server did not retain a listener for upgrade handoff".to_string()
        })?;
        self.serve_mode
            .store(AcceptLoopMode::Paused.as_u8(), Ordering::SeqCst);
        let _ = self.accept_mode_tx.send(AcceptLoopMode::Paused);
        Ok(listener)
    }

    fn activate_after_upgrade_commit(&self) -> Result<(), String> {
        self.serve_mode
            .store(AcceptLoopMode::Running.as_u8(), Ordering::SeqCst);
        self.accept_mode_tx
            .send(AcceptLoopMode::Running)
            .map_err(|_| "admin gRPC accept loop was not available".to_string())
    }

    async fn join(self) -> Result<(), String> {
        self.serve_mode
            .store(AcceptLoopMode::Shutdown.as_u8(), Ordering::SeqCst);
        let _ = self.accept_mode_tx.send(AcceptLoopMode::Shutdown);
        let _ = self.shutdown_tx.send(true);
        self.wait_for_server_exit().await
    }
}

async fn wait_for_shutdown_signal(mut shutdown_rx: watch::Receiver<bool>) {
    if *shutdown_rx.borrow() {
        return;
    }
    while shutdown_rx.changed().await.is_ok() {
        if *shutdown_rx.borrow() {
            break;
        }
    }
}

fn reject_mutating_request_while_paused(serve_mode: &AtomicU8, action: &str) -> Result<(), Status> {
    if serve_mode.load(Ordering::SeqCst) == AcceptLoopMode::Paused.as_u8() {
        return Err(Status::failed_precondition(format!(
            "admin action `{action}` is unavailable while runtime upgrade freeze is active"
        )));
    }
    Ok(())
}

fn authenticate_request(
    auth: &HashMap<String, String>,
    metadata: &MetadataMap,
) -> Result<String, Status> {
    let mut authorizations = metadata.get_all("authorization").iter();
    let authorization = authorizations
        .next()
        .ok_or_else(|| Status::unauthenticated("missing remote admin token"))?;
    if authorizations.next().is_some() {
        return Err(Status::unauthenticated("invalid remote admin token"));
    }
    let authorization = authorization
        .to_str()
        .map_err(|_| Status::unauthenticated("invalid remote admin token"))?;
    if authorization.trim() != authorization {
        return Err(Status::unauthenticated("invalid remote admin token"));
    }
    let Some((scheme, token)) = authorization.split_once(' ') else {
        return Err(Status::unauthenticated("invalid remote admin token"));
    };
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.chars().any(char::is_whitespace)
    {
        return Err(Status::unauthenticated("invalid remote admin token"));
    }
    auth.get(token)
        .cloned()
        .ok_or_else(|| Status::unauthenticated("invalid remote admin token"))
}

fn map_host_callback_error(message: String) -> Status {
    Status::failed_precondition(message)
}

fn map_surface_response_error(response: surface_admin::AdminResponse) -> Status {
    match response {
        surface_admin::AdminResponse::PermissionDenied { permission, .. } => {
            Status::permission_denied(format!("permission denied: {}", permission.as_str()))
        }
        surface_admin::AdminResponse::Error { message } => {
            if message.starts_with("invalid admin subject:") {
                Status::unauthenticated(message)
            } else {
                Status::failed_precondition(message)
            }
        }
        other => Status::internal(format!("unexpected admin response: {other:?}")),
    }
}

fn map_transport(transport: mc_proto_common::TransportKind) -> i32 {
    match transport {
        mc_proto_common::TransportKind::Tcp => proto::TransportKind::Tcp as i32,
        mc_proto_common::TransportKind::Udp => proto::TransportKind::Udp as i32,
    }
}

fn map_phase(phase: mc_proto_common::ConnectionPhase) -> i32 {
    match phase {
        mc_proto_common::ConnectionPhase::Handshaking => proto::ConnectionPhase::Handshaking as i32,
        mc_proto_common::ConnectionPhase::Status => proto::ConnectionPhase::Status as i32,
        mc_proto_common::ConnectionPhase::Login => proto::ConnectionPhase::Login as i32,
        mc_proto_common::ConnectionPhase::Play => proto::ConnectionPhase::Play as i32,
    }
}

fn map_reload_mode(mode: RuntimeReloadMode) -> i32 {
    match mode {
        RuntimeReloadMode::Artifacts => proto::RuntimeReloadMode::Artifacts as i32,
        RuntimeReloadMode::Topology => proto::RuntimeReloadMode::Topology as i32,
        RuntimeReloadMode::Core => proto::RuntimeReloadMode::Core as i32,
        RuntimeReloadMode::Full => proto::RuntimeReloadMode::Full as i32,
    }
}

fn map_reload_mode_request(mode: i32) -> Result<RuntimeReloadMode, Status> {
    let mode = proto::RuntimeReloadMode::try_from(mode)
        .map_err(|_| Status::invalid_argument("invalid reload runtime mode"))?;
    Ok(match mode {
        proto::RuntimeReloadMode::Artifacts => RuntimeReloadMode::Artifacts,
        proto::RuntimeReloadMode::Topology => RuntimeReloadMode::Topology,
        proto::RuntimeReloadMode::Core => RuntimeReloadMode::Core,
        proto::RuntimeReloadMode::Full => RuntimeReloadMode::Full,
        proto::RuntimeReloadMode::Unspecified => {
            return Err(Status::invalid_argument(
                "reload runtime mode must be specified",
            ));
        }
    })
}

fn count_to_u64(value: usize) -> u64 {
    u64::try_from(value).expect("count should fit into u64")
}

fn map_named_counts(counts: Vec<AdminNamedCountView>) -> Vec<proto::AdminNamedCountView> {
    counts
        .into_iter()
        .map(|count| proto::AdminNamedCountView {
            value: count.value,
            count: count_to_u64(count.count),
        })
        .collect()
}

fn map_session_summary(summary: AdminSessionSummaryView) -> proto::AdminSessionSummaryView {
    proto::AdminSessionSummaryView {
        total: count_to_u64(summary.total),
        by_transport: summary
            .by_transport
            .into_iter()
            .map(|count| proto::AdminSessionTransportCountView {
                transport: map_transport(count.transport),
                count: count_to_u64(count.count),
            })
            .collect(),
        by_phase: summary
            .by_phase
            .into_iter()
            .map(|count| proto::AdminPhaseCountView {
                phase: map_phase(count.phase),
                count: count_to_u64(count.count),
            })
            .collect(),
        by_generation: summary
            .by_generation
            .into_iter()
            .map(|count| proto::AdminGenerationCountView {
                generation_id: count.generation_id,
                count: count_to_u64(count.count),
            })
            .collect(),
        by_adapter_id: map_named_counts(summary.by_adapter_id),
        by_gameplay_profile: map_named_counts(summary.by_gameplay_profile),
    }
}

fn map_status_view(status: AdminStatusView) -> proto::AdminStatusView {
    proto::AdminStatusView {
        active_generation_id: status.active_generation_id,
        draining_generation_ids: status.draining_generation_ids,
        listener_bindings: status
            .listener_bindings
            .into_iter()
            .map(|binding| proto::AdminListenerBindingView {
                transport: map_transport(binding.transport),
                local_addr: binding.local_addr,
                adapter_ids: binding.adapter_ids,
            })
            .collect(),
        default_adapter_id: status.default_adapter_id,
        default_bedrock_adapter_id: status.default_bedrock_adapter_id,
        enabled_adapter_ids: status.enabled_adapter_ids,
        enabled_bedrock_adapter_ids: status.enabled_bedrock_adapter_ids,
        motd: status.motd,
        max_players: u32::from(status.max_players),
        session_summary: Some(map_session_summary(status.session_summary)),
        dirty: status.dirty,
        plugin_host: status
            .plugin_host
            .map(|plugin_host| proto::AdminPluginHostView {
                protocol_count: count_to_u64(plugin_host.protocol_count),
                gameplay_count: count_to_u64(plugin_host.gameplay_count),
                storage_count: count_to_u64(plugin_host.storage_count),
                auth_count: count_to_u64(plugin_host.auth_count),
                admin_surface_count: count_to_u64(plugin_host.admin_surface_count),
                active_quarantine_count: count_to_u64(plugin_host.active_quarantine_count),
                artifact_quarantine_count: count_to_u64(plugin_host.artifact_quarantine_count),
                pending_fatal_error: plugin_host.pending_fatal_error,
            }),
        upgrade: status
            .upgrade
            .map(|upgrade| proto::RuntimeUpgradeStateView {
                role: match upgrade.role {
                    RuntimeUpgradeRole::Parent => proto::RuntimeUpgradeRole::Parent as i32,
                    RuntimeUpgradeRole::Child => proto::RuntimeUpgradeRole::Child as i32,
                },
                phase: match upgrade.phase {
                    RuntimeUpgradePhase::ParentFreezing => {
                        proto::RuntimeUpgradePhase::ParentFreezing as i32
                    }
                    RuntimeUpgradePhase::ParentWaitingChildReady => {
                        proto::RuntimeUpgradePhase::ParentWaitingChildReady as i32
                    }
                    RuntimeUpgradePhase::ParentRollingBack => {
                        proto::RuntimeUpgradePhase::ParentRollingBack as i32
                    }
                    RuntimeUpgradePhase::ChildWaitingCommit => {
                        proto::RuntimeUpgradePhase::ChildWaitingCommit as i32
                    }
                },
            }),
    }
}

fn map_sessions_view(sessions: AdminSessionsView) -> proto::AdminSessionsView {
    proto::AdminSessionsView {
        summary: Some(map_session_summary(sessions.summary)),
        sessions: sessions
            .sessions
            .into_iter()
            .map(|session| proto::AdminSessionView {
                connection_id: session.connection_id.0,
                generation_id: session.generation_id,
                transport: map_transport(session.transport),
                phase: map_phase(session.phase),
                adapter_id: session.adapter_id,
                gameplay_profile: session.gameplay_profile,
                player_id: session
                    .player_id
                    .map(|player_id| player_id.0.hyphenated().to_string()),
                entity_id: session.entity_id.map(|entity_id| entity_id.0),
                protocol_generation: session.protocol_generation.map(|generation| generation.0),
                gameplay_generation: session.gameplay_generation.map(|generation| generation.0),
            })
            .collect(),
    }
}

fn map_topology_reload_view(result: AdminTopologyReloadView) -> proto::AdminTopologyReloadView {
    proto::AdminTopologyReloadView {
        activated_generation_id: result.activated_generation_id,
        retired_generation_ids: result.retired_generation_ids,
        applied_config_change: result.applied_config_change,
        reconfigured_adapter_ids: result.reconfigured_adapter_ids,
    }
}

fn map_runtime_reload_view(result: AdminRuntimeReloadView) -> proto::AdminRuntimeReloadView {
    let detail = match result.detail {
        AdminRuntimeReloadDetail::Artifacts(AdminArtifactsReloadView {
            reloaded_plugin_ids,
        }) => {
            proto::admin_runtime_reload_view::Detail::Artifacts(proto::AdminArtifactsReloadView {
                reloaded_plugin_ids,
            })
        }
        AdminRuntimeReloadDetail::Topology(result) => {
            proto::admin_runtime_reload_view::Detail::Topology(map_topology_reload_view(result))
        }
        AdminRuntimeReloadDetail::Core(_) => {
            proto::admin_runtime_reload_view::Detail::Core(proto::AdminCoreReloadView {})
        }
        AdminRuntimeReloadDetail::Full(AdminFullReloadView {
            reloaded_plugin_ids,
            topology,
        }) => proto::admin_runtime_reload_view::Detail::Full(proto::AdminFullReloadView {
            reloaded_plugin_ids,
            topology: Some(map_topology_reload_view(topology)),
        }),
    };
    proto::AdminRuntimeReloadView {
        mode: map_reload_mode(result.mode),
        detail: Some(detail),
    }
}

fn map_upgrade_runtime_view(result: AdminUpgradeRuntimeView) -> proto::AdminUpgradeRuntimeView {
    proto::AdminUpgradeRuntimeView {
        executable_path: result.executable_path,
    }
}

export_plugin!(admin_surface, GrpcAdminSurfacePlugin, MANIFEST);
