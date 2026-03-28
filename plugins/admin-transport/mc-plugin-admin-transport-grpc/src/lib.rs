#![allow(clippy::multiple_crate_versions)]

use mc_core::{AdminTransportCapability, AdminTransportCapabilitySet};
use mc_plugin_api::codec::admin_transport::{
    AdminTransportEndpointView, AdminTransportPauseView, AdminTransportStatusView,
};
use mc_plugin_api::codec::admin_ui::{
    AdminArtifactsReloadView, AdminFullReloadView, AdminNamedCountView, AdminRuntimeReloadDetail,
    AdminRuntimeReloadView, AdminSessionSummaryView, AdminSessionsView, AdminStatusView,
    AdminTopologyReloadView, AdminUpgradeRuntimeView, RuntimeReloadMode, RuntimeUpgradePhase,
    RuntimeUpgradeRole,
};
use mc_plugin_sdk_rust::admin_transport::{
    AdminTransportHost, RustAdminTransportPlugin, SdkAdminTransportHost,
};
use mc_plugin_sdk_rust::capabilities;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use revy_admin_grpc::admin::{
    self as proto, GetStatusResponse, ListSessionsResponse, ReloadRuntimeResponse,
    ShutdownResponse, UpgradeRuntimeResponse,
    admin_control_plane_server::{AdminControlPlane, AdminControlPlaneServer},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::fd::{FromRawFd, IntoRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{FromRawSocket, IntoRawSocket, RawSocket};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

const MANIFEST: StaticPluginManifest = StaticPluginManifest::admin_transport(
    "admin-transport-grpc",
    "gRPC Admin Transport Plugin",
    "grpc-v1",
);

#[derive(Default)]
pub struct GrpcAdminTransportPlugin {
    runtime: OnceLock<Result<tokio::runtime::Runtime, String>>,
    state: Mutex<GrpcTransportState>,
}

enum GrpcTransportState {
    Stopped,
    Active(ActiveGrpcTransport),
    Paused(PausedGrpcTransport),
}

impl Default for GrpcTransportState {
    fn default() -> Self {
        Self::Stopped
    }
}

struct ActiveGrpcTransport {
    handle: AdminGrpcServerHandle,
}

struct PausedGrpcTransport {
    handle: AdminGrpcServerHandle,
}

#[derive(Clone)]
struct AdminGrpcService {
    host: SdkAdminTransportHost,
    auth: ArcAuthMap,
    shutdown_tx: watch::Sender<bool>,
}

type ArcAuthMap = std::sync::Arc<HashMap<String, String>>;

#[derive(Debug)]
struct AdminGrpcServerHandle {
    runtime_handle: tokio::runtime::Handle,
    local_addr: SocketAddr,
    listener: std::net::TcpListener,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    incoming_tx: mpsc::Sender<Result<TcpListenerStream, std::io::Error>>,
    accept_state: AdminGrpcAcceptState,
    server_join_handle: JoinHandle<()>,
    server_done_rx: oneshot::Receiver<Result<(), String>>,
}

#[derive(Debug)]
enum AdminGrpcAcceptState {
    Accepting {
        control_tx: mpsc::Sender<AdminGrpcAcceptControl>,
        join_handle: JoinHandle<Result<(), String>>,
    },
    Paused,
}

#[derive(Debug)]
enum AdminGrpcAcceptControl {
    PauseForUpgrade { ack_tx: oneshot::Sender<()> },
    Shutdown,
}

type TcpListenerStream = tokio::net::TcpStream;

#[derive(Debug)]
struct SpawnedAcceptLoop {
    control_tx: mpsc::Sender<AdminGrpcAcceptControl>,
    join_handle: JoinHandle<Result<(), String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrpcTransportConfig {
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

impl RustAdminTransportPlugin for GrpcAdminTransportPlugin {
    fn descriptor(&self) -> mc_plugin_api::codec::admin_transport::AdminTransportDescriptor {
        mc_plugin_sdk_rust::admin_transport::admin_transport_descriptor("grpc-v1")
    }

    fn capability_set(&self) -> AdminTransportCapabilitySet {
        capabilities::admin_transport_capabilities(&[AdminTransportCapability::RuntimeReload])
    }

    fn start(
        &self,
        host: SdkAdminTransportHost,
        transport_config_path: &str,
    ) -> Result<AdminTransportStatusView, String> {
        let config = load_transport_config(transport_config_path)?;
        let handle = self.block_on_async(spawn_admin_grpc_server(
            self.runtime_handle()?,
            &config,
            host,
        ))?;
        let status = handle.status_view();
        let mut state = self
            .state
            .lock()
            .expect("admin-transport state mutex should not be poisoned");
        ensure_stopped(&state)?;
        *state = GrpcTransportState::Active(ActiveGrpcTransport { handle });
        Ok(status)
    }

    fn pause_for_upgrade(
        &self,
        host: SdkAdminTransportHost,
    ) -> Result<AdminTransportPauseView, String> {
        let mut state = self
            .state
            .lock()
            .expect("admin-transport state mutex should not be poisoned");
        let mut handle = match std::mem::replace(&mut *state, GrpcTransportState::Stopped) {
            GrpcTransportState::Active(active) => active.handle,
            other => {
                *state = other;
                return Err("gRPC admin transport is not active".to_string());
            }
        };
        let listener = self.block_on_async(handle.pause_for_upgrade())?;
        host.publish_tcp_listener_for_upgrade(listener_into_raw(listener))?;
        let pause = AdminTransportPauseView {
            resume_payload: Vec::new(),
        };
        *state = GrpcTransportState::Paused(PausedGrpcTransport { handle });
        Ok(pause)
    }

    fn resume_from_upgrade(
        &self,
        host: SdkAdminTransportHost,
        transport_config_path: &str,
        _resume_payload: &[u8],
    ) -> Result<AdminTransportStatusView, String> {
        let config = load_transport_config(transport_config_path)?;
        let raw_listener = host.take_tcp_listener_from_upgrade()?.ok_or_else(|| {
            "admin-transport upgrade resume did not receive a listener".to_string()
        })?;
        let listener = listener_from_raw(raw_listener);
        let handle = self.block_on_async(spawn_admin_grpc_server_from_std_listener(
            self.runtime_handle()?,
            listener,
            &config,
            host,
        ))?;
        let status = handle.status_view();
        let mut state = self
            .state
            .lock()
            .expect("admin-transport state mutex should not be poisoned");
        ensure_stopped(&state)?;
        *state = GrpcTransportState::Active(ActiveGrpcTransport { handle });
        Ok(status)
    }

    fn resume_after_upgrade_rollback(
        &self,
        _host: SdkAdminTransportHost,
    ) -> Result<AdminTransportStatusView, String> {
        let mut state = self
            .state
            .lock()
            .expect("admin-transport state mutex should not be poisoned");
        let mut handle = match std::mem::replace(&mut *state, GrpcTransportState::Stopped) {
            GrpcTransportState::Paused(paused) => paused.handle,
            other => {
                *state = other;
                return Err("gRPC admin transport was not paused for upgrade".to_string());
            }
        };
        handle.resume_after_upgrade_rollback()?;
        let status = handle.status_view();
        *state = GrpcTransportState::Active(ActiveGrpcTransport { handle });
        Ok(status)
    }

    fn shutdown(&self, _host: SdkAdminTransportHost) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .expect("admin-transport state mutex should not be poisoned");
        let state_value = std::mem::replace(&mut *state, GrpcTransportState::Stopped);
        match state_value {
            GrpcTransportState::Stopped => Ok(()),
            GrpcTransportState::Active(active) => self.block_on_async(active.handle.join()),
            GrpcTransportState::Paused(paused) => self.block_on_async(paused.handle.join()),
        }
    }
}

impl GrpcAdminTransportPlugin {
    fn runtime(&self) -> Result<&tokio::runtime::Runtime, String> {
        match self.runtime.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("admin-transport-grpc")
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

fn ensure_stopped(state: &GrpcTransportState) -> Result<(), String> {
    if matches!(state, GrpcTransportState::Stopped) {
        Ok(())
    } else {
        Err("gRPC admin transport is already running".to_string())
    }
}

fn load_transport_config(path: &str) -> Result<LoadedGrpcTransportConfig, String> {
    let transport_config_path = PathBuf::from(path);
    let contents = fs::read_to_string(&transport_config_path).map_err(|error| {
        format!(
            "failed to read admin transport config {}: {error}",
            transport_config_path.display()
        )
    })?;
    let document: GrpcTransportConfig = toml::from_str(&contents).map_err(|error| {
        format!(
            "failed to parse admin transport config {}: {error}",
            transport_config_path.display()
        )
    })?;
    let bind_addr: SocketAddr = document.bind_addr.parse().map_err(|_| {
        format!(
            "invalid bind_addr `{}` in {}",
            document.bind_addr,
            transport_config_path.display()
        )
    })?;
    if !document.allow_non_loopback && !bind_addr.ip().is_loopback() {
        return Err(format!(
            "bind_addr `{bind_addr}` is non-loopback; set allow_non_loopback=true in {}",
            transport_config_path.display()
        ));
    }
    if document.principals.is_empty() {
        return Err(format!(
            "{} must define at least one principals.<id>.token_file entry",
            transport_config_path.display()
        ));
    }
    let mut token_principals = HashMap::new();
    let base_dir = transport_config_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
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
    Ok(LoadedGrpcTransportConfig {
        bind_addr,
        token_principals: std::sync::Arc::new(token_principals),
    })
}

struct LoadedGrpcTransportConfig {
    bind_addr: SocketAddr,
    token_principals: ArcAuthMap,
}

#[tonic::async_trait]
impl AdminControlPlane for AdminGrpcService {
    async fn get_status(
        &self,
        request: Request<proto::GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        let principal_id = authenticate_request(&self.auth, request.metadata())?;
        self.host
            .status(&principal_id)
            .map(|status| {
                Response::new(GetStatusResponse {
                    status: Some(map_status_view(status)),
                })
            })
            .map_err(map_host_error)
    }

    async fn list_sessions(
        &self,
        request: Request<proto::ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let principal_id = authenticate_request(&self.auth, request.metadata())?;
        self.host
            .sessions(&principal_id)
            .map(|sessions| {
                Response::new(ListSessionsResponse {
                    sessions: Some(map_sessions_view(sessions)),
                })
            })
            .map_err(map_host_error)
    }

    async fn reload_runtime(
        &self,
        request: Request<proto::ReloadRuntimeRequest>,
    ) -> Result<Response<ReloadRuntimeResponse>, Status> {
        let principal_id = authenticate_request(&self.auth, request.metadata())?;
        let mode = map_reload_mode_request(request.into_inner().mode)?;
        self.host
            .reload_runtime(&principal_id, mode)
            .map(|result| {
                Response::new(ReloadRuntimeResponse {
                    result: Some(map_runtime_reload_view(result)),
                })
            })
            .map_err(map_host_error)
    }

    async fn upgrade_runtime(
        &self,
        request: Request<proto::UpgradeRuntimeRequest>,
    ) -> Result<Response<UpgradeRuntimeResponse>, Status> {
        let principal_id = authenticate_request(&self.auth, request.metadata())?;
        self.host
            .upgrade_runtime(&principal_id, &request.into_inner().executable_path)
            .map(|result| {
                Response::new(UpgradeRuntimeResponse {
                    result: Some(map_upgrade_runtime_view(result)),
                })
            })
            .map_err(map_host_error)
    }

    async fn shutdown(
        &self,
        request: Request<proto::ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        let principal_id = authenticate_request(&self.auth, request.metadata())?;
        self.host.shutdown(&principal_id).map_err(map_host_error)?;
        let _ = self.shutdown_tx.send(true);
        Ok(Response::new(ShutdownResponse {}))
    }
}

async fn spawn_admin_grpc_server(
    runtime_handle: tokio::runtime::Handle,
    config: &LoadedGrpcTransportConfig,
    host: SdkAdminTransportHost,
) -> Result<AdminGrpcServerHandle, String> {
    let listener = std::net::TcpListener::bind(config.bind_addr).map_err(|error| {
        format!(
            "failed to bind admin gRPC listener on {}: {error}",
            config.bind_addr
        )
    })?;
    spawn_admin_grpc_server_from_std_listener(runtime_handle, listener, config, host).await
}

async fn spawn_admin_grpc_server_from_std_listener(
    runtime_handle: tokio::runtime::Handle,
    listener: std::net::TcpListener,
    config: &LoadedGrpcTransportConfig,
    host: SdkAdminTransportHost,
) -> Result<AdminGrpcServerHandle, String> {
    let local_addr = listener.local_addr().map_err(|error| error.to_string())?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server_shutdown_rx = shutdown_rx.clone();
    let (incoming_tx, incoming_rx) = mpsc::channel(64);
    let spawned_accept = spawn_accept_loop(
        &runtime_handle,
        listener.try_clone().map_err(|error| error.to_string())?,
        incoming_tx.clone(),
        shutdown_rx.clone(),
    )?;
    let service = AdminGrpcService {
        host,
        auth: std::sync::Arc::clone(&config.token_principals),
        shutdown_tx: shutdown_tx.clone(),
    };
    let (server_done_tx, server_done_rx) = oneshot::channel();
    let server_join_handle = runtime_handle.spawn(async move {
        let result = tonic::transport::Server::builder()
            .add_service(AdminControlPlaneServer::new(service))
            .serve_with_incoming_shutdown(
                ReceiverStream::new(incoming_rx),
                wait_for_shutdown_signal(server_shutdown_rx),
            )
            .await
            .map_err(|error| format!("admin gRPC server failed: {error}"));
        let _ = server_done_tx.send(result);
    });
    Ok(AdminGrpcServerHandle {
        runtime_handle,
        local_addr,
        listener,
        shutdown_tx,
        shutdown_rx,
        incoming_tx,
        accept_state: AdminGrpcAcceptState::Accepting {
            control_tx: spawned_accept.control_tx,
            join_handle: spawned_accept.join_handle,
        },
        server_join_handle,
        server_done_rx,
    })
}

impl AdminGrpcServerHandle {
    fn status_view(&self) -> AdminTransportStatusView {
        AdminTransportStatusView {
            endpoints: vec![AdminTransportEndpointView {
                transport: "grpc".to_string(),
                local_addr: self.local_addr.to_string(),
            }],
        }
    }

    async fn wait_for_server_exit(&mut self) -> Result<(), String> {
        (&mut self.server_done_rx)
            .await
            .map_err(|_| "admin gRPC server task ended unexpectedly".to_string())?
    }

    fn try_clone_listener(&self) -> Result<std::net::TcpListener, String> {
        self.listener.try_clone().map_err(|error| error.to_string())
    }

    async fn pause_for_upgrade(&mut self) -> Result<std::net::TcpListener, String> {
        match std::mem::replace(&mut self.accept_state, AdminGrpcAcceptState::Paused) {
            AdminGrpcAcceptState::Accepting {
                control_tx,
                join_handle,
            } => {
                let (ack_tx, ack_rx) = oneshot::channel();
                control_tx
                    .send(AdminGrpcAcceptControl::PauseForUpgrade { ack_tx })
                    .await
                    .map_err(|_| {
                        "admin gRPC accept loop stopped before upgrade pause".to_string()
                    })?;
                let _ = ack_rx.await;
                join_handle.await.map_err(|error| error.to_string())??;
                self.try_clone_listener()
            }
            AdminGrpcAcceptState::Paused => {
                Err("admin gRPC listener is already paused".to_string())
            }
        }
    }

    fn resume_after_upgrade_rollback(&mut self) -> Result<(), String> {
        if matches!(self.accept_state, AdminGrpcAcceptState::Accepting { .. }) {
            return Ok(());
        }
        let spawned = spawn_accept_loop(
            &self.runtime_handle,
            self.listener
                .try_clone()
                .map_err(|error| error.to_string())?,
            self.incoming_tx.clone(),
            self.shutdown_rx.clone(),
        )?;
        self.accept_state = AdminGrpcAcceptState::Accepting {
            control_tx: spawned.control_tx,
            join_handle: spawned.join_handle,
        };
        Ok(())
    }

    async fn join(mut self) -> Result<(), String> {
        let _ = self.shutdown_tx.send(true);
        if let AdminGrpcAcceptState::Accepting {
            control_tx,
            join_handle,
        } = std::mem::replace(&mut self.accept_state, AdminGrpcAcceptState::Paused)
        {
            let _ = control_tx.send(AdminGrpcAcceptControl::Shutdown).await;
            join_handle.await.map_err(|error| error.to_string())??;
        }
        self.wait_for_server_exit().await?;
        self.server_join_handle.abort();
        Ok(())
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

fn spawn_accept_loop(
    runtime_handle: &tokio::runtime::Handle,
    listener: std::net::TcpListener,
    incoming_tx: mpsc::Sender<Result<TcpListenerStream, std::io::Error>>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<SpawnedAcceptLoop, String> {
    let (control_tx, mut control_rx) = mpsc::channel(4);
    let join_handle = runtime_handle.spawn(async move {
        let listener = TcpListener::from_std(listener).map_err(|error| error.to_string())?;
        let shutdown = wait_for_shutdown_signal(shutdown_rx);
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => return Ok(()),
                Some(control) = control_rx.recv() => match control {
                    AdminGrpcAcceptControl::PauseForUpgrade { ack_tx } => {
                        let _ = ack_tx.send(());
                        return Ok(());
                    }
                    AdminGrpcAcceptControl::Shutdown => return Ok(()),
                },
                accepted = listener.accept() => {
                    let (stream, _) = accepted.map_err(|error| error.to_string())?;
                    if incoming_tx.send(Ok(stream)).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    });
    Ok(SpawnedAcceptLoop {
        control_tx,
        join_handle,
    })
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

fn map_host_error(message: String) -> Status {
    if message.starts_with("invalid admin subject:") {
        Status::unauthenticated(message)
    } else if message.starts_with("permission denied:") {
        Status::permission_denied(message)
    } else {
        Status::failed_precondition(message)
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
            .map(|count| proto::AdminTransportCountView {
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
                admin_transport_count: count_to_u64(plugin_host.admin_transport_count),
                admin_ui_count: count_to_u64(plugin_host.admin_ui_count),
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

export_plugin!(admin_transport, GrpcAdminTransportPlugin, MANIFEST);
