use revy_admin_grpc::admin::{
    self as proto, GetStatusResponse, ListSessionsResponse, ReloadRuntimeResponse,
    ShutdownResponse, UpgradeRuntimeResponse,
    admin_control_plane_server::{AdminControlPlane, AdminControlPlaneServer},
};
use server_runtime::RuntimeError;
use server_runtime::runtime::{
    AdminArtifactsReloadView, AdminAuthError, AdminCommandError, AdminControlPlaneHandle,
    AdminFullReloadView, AdminNamedCountView, AdminRuntimeReloadDetail, AdminRuntimeReloadView,
    AdminSessionSummaryView, AdminSessionsView, AdminStatusView, AdminSubject,
    AdminTopologyReloadView, AdminUpgradeRuntimeView, RuntimeReloadMode, RuntimeUpgradePhase,
    RuntimeUpgradeRole,
};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

#[derive(Debug)]
pub(crate) struct AdminGrpcServerHandle {
    local_addr: SocketAddr,
    listener: std::net::TcpListener,
    shutdown_rx: watch::Receiver<bool>,
    incoming_tx: mpsc::Sender<Result<TcpListenerStream, std::io::Error>>,
    accept_state: AdminGrpcAcceptState,
    server_done_rx: oneshot::Receiver<Result<(), RuntimeError>>,
}

#[derive(Debug)]
enum AdminGrpcAcceptState {
    Accepting {
        control_tx: mpsc::Sender<AdminGrpcAcceptControl>,
        join_handle: JoinHandle<Result<(), RuntimeError>>,
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
    join_handle: JoinHandle<Result<(), RuntimeError>>,
}

impl AdminGrpcServerHandle {
    #[must_use]
    pub(crate) const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub(crate) async fn wait_for_server_exit(&mut self) -> Result<(), RuntimeError> {
        (&mut self.server_done_rx).await.map_err(|_| {
            RuntimeError::Config("admin gRPC server task ended unexpectedly".to_string())
        })?
    }

    pub(crate) fn try_clone_listener(&self) -> Result<std::net::TcpListener, RuntimeError> {
        self.listener.try_clone().map_err(Into::into)
    }

    pub(crate) async fn pause_for_upgrade(
        &mut self,
    ) -> Result<std::net::TcpListener, RuntimeError> {
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
                        RuntimeError::Config(
                            "admin gRPC accept loop stopped before upgrade pause".to_string(),
                        )
                    })?;
                let _ = ack_rx.await;
                join_handle.await.map_err(RuntimeError::from)??;
                self.try_clone_listener()
            }
            AdminGrpcAcceptState::Paused => Err(RuntimeError::Config(
                "admin gRPC listener is already paused for upgrade".to_string(),
            )),
        }
    }

    pub(crate) fn resume_after_upgrade_rollback(&mut self) -> Result<(), RuntimeError> {
        if matches!(self.accept_state, AdminGrpcAcceptState::Accepting { .. }) {
            return Ok(());
        }
        let spawned = spawn_accept_loop(
            self.listener.try_clone()?,
            self.incoming_tx.clone(),
            self.shutdown_rx.clone(),
        )?;
        self.accept_state = AdminGrpcAcceptState::Accepting {
            control_tx: spawned.control_tx,
            join_handle: spawned.join_handle,
        };
        Ok(())
    }

    pub(crate) async fn join(mut self) -> Result<(), RuntimeError> {
        if let AdminGrpcAcceptState::Accepting {
            control_tx,
            join_handle,
        } = std::mem::replace(&mut self.accept_state, AdminGrpcAcceptState::Paused)
        {
            let _ = control_tx.send(AdminGrpcAcceptControl::Shutdown).await;
            join_handle.await.map_err(RuntimeError::from)??;
        }
        self.wait_for_server_exit().await
    }
}

#[derive(Clone)]
struct AdminGrpcService {
    control_plane: AdminControlPlaneHandle,
    shutdown_tx: watch::Sender<bool>,
}

#[tonic::async_trait]
impl AdminControlPlane for AdminGrpcService {
    async fn get_status(
        &self,
        request: Request<proto::GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        let subject = authenticate_request(&self.control_plane, request.metadata()).await?;
        self.control_plane
            .status(&subject)
            .await
            .map(|status| {
                Response::new(GetStatusResponse {
                    status: Some(map_status_view(status)),
                })
            })
            .map_err(map_command_error)
    }

    async fn list_sessions(
        &self,
        request: Request<proto::ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let subject = authenticate_request(&self.control_plane, request.metadata()).await?;
        self.control_plane
            .sessions(&subject)
            .await
            .map(|sessions| {
                Response::new(ListSessionsResponse {
                    sessions: Some(map_sessions_view(sessions)),
                })
            })
            .map_err(map_command_error)
    }

    async fn reload_runtime(
        &self,
        request: Request<proto::ReloadRuntimeRequest>,
    ) -> Result<Response<ReloadRuntimeResponse>, Status> {
        let subject = authenticate_request(&self.control_plane, request.metadata()).await?;
        let mode = map_reload_mode_request(request.into_inner().mode)?;
        self.control_plane
            .reload_runtime(&subject, mode)
            .await
            .map(|result| {
                Response::new(ReloadRuntimeResponse {
                    result: Some(map_runtime_reload_view(result)),
                })
            })
            .map_err(map_command_error)
    }

    async fn upgrade_runtime(
        &self,
        request: Request<proto::UpgradeRuntimeRequest>,
    ) -> Result<Response<UpgradeRuntimeResponse>, Status> {
        let subject = authenticate_request(&self.control_plane, request.metadata()).await?;
        self.control_plane
            .upgrade_runtime(&subject, request.into_inner().executable_path)
            .await
            .map(|result| {
                Response::new(UpgradeRuntimeResponse {
                    result: Some(map_upgrade_runtime_view(result)),
                })
            })
            .map_err(map_command_error)
    }

    async fn shutdown(
        &self,
        request: Request<proto::ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        let subject = authenticate_request(&self.control_plane, request.metadata()).await?;
        self.control_plane
            .shutdown(&subject)
            .await
            .map_err(map_command_error)?;
        let _ = self.shutdown_tx.send(true);
        Ok(Response::new(ShutdownResponse {}))
    }
}

pub(crate) async fn spawn_admin_grpc_server(
    bind_addr: SocketAddr,
    control_plane: AdminControlPlaneHandle,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<AdminGrpcServerHandle, RuntimeError> {
    let listener = std::net::TcpListener::bind(bind_addr).map_err(|error| {
        RuntimeError::Config(format!(
            "failed to bind admin gRPC listener on {bind_addr}: {error}"
        ))
    })?;
    spawn_admin_grpc_server_from_std_listener(listener, control_plane, shutdown_tx, shutdown_rx)
        .await
}

pub(crate) async fn spawn_admin_grpc_server_from_std_listener(
    listener: std::net::TcpListener,
    control_plane: AdminControlPlaneHandle,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<AdminGrpcServerHandle, RuntimeError> {
    let local_addr = listener.local_addr()?;
    listener.set_nonblocking(true)?;
    let server_shutdown_rx = shutdown_rx.clone();
    let (incoming_tx, incoming_rx) = mpsc::channel(64);
    let spawned_accept = spawn_accept_loop(
        listener.try_clone()?,
        incoming_tx.clone(),
        shutdown_rx.clone(),
    )?;
    let service = AdminGrpcService {
        control_plane,
        shutdown_tx,
    };
    let (server_done_tx, server_done_rx) = oneshot::channel();
    tokio::spawn(async move {
        let result = async move {
            tonic::transport::Server::builder()
                .add_service(AdminControlPlaneServer::new(service))
                .serve_with_incoming_shutdown(
                    ReceiverStream::new(incoming_rx),
                    wait_for_shutdown_signal(server_shutdown_rx),
                )
                .await
                .map_err(|error| RuntimeError::Config(format!("admin gRPC server failed: {error}")))
        }
        .await;
        let _ = server_done_tx.send(result);
    });
    Ok(AdminGrpcServerHandle {
        local_addr,
        listener,
        shutdown_rx,
        incoming_tx,
        accept_state: AdminGrpcAcceptState::Accepting {
            control_tx: spawned_accept.control_tx,
            join_handle: spawned_accept.join_handle,
        },
        server_done_rx,
    })
}

pub(crate) async fn wait_for_shutdown_signal(mut shutdown_rx: watch::Receiver<bool>) {
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
    listener: std::net::TcpListener,
    incoming_tx: mpsc::Sender<Result<TcpListenerStream, std::io::Error>>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<SpawnedAcceptLoop, RuntimeError> {
    let (control_tx, mut control_rx) = mpsc::channel(4);
    let join_handle = tokio::spawn(async move {
        let listener = TcpListener::from_std(listener)?;
        let shutdown = wait_for_shutdown_signal(shutdown_rx);
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => {
                    return Ok(());
                }
                Some(control) = control_rx.recv() => {
                    match control {
                        AdminGrpcAcceptControl::PauseForUpgrade { ack_tx } => {
                            let _ = ack_tx.send(());
                            return Ok(());
                        }
                        AdminGrpcAcceptControl::Shutdown => {
                            return Ok(());
                        }
                    }
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
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

async fn authenticate_request(
    control_plane: &AdminControlPlaneHandle,
    metadata: &MetadataMap,
) -> Result<AdminSubject, Status> {
    let mut authorizations = metadata.get_all("authorization").iter();
    let authorization = authorizations
        .next()
        .ok_or_else(|| map_auth_error(AdminAuthError::MissingToken))?;
    if authorizations.next().is_some() {
        return Err(map_auth_error(AdminAuthError::InvalidToken));
    }
    let authorization = authorization
        .to_str()
        .map_err(|_| map_auth_error(AdminAuthError::InvalidToken))?;
    if authorization.trim() != authorization {
        return Err(map_auth_error(AdminAuthError::InvalidToken));
    }
    let Some((scheme, token)) = authorization.split_once(' ') else {
        return Err(map_auth_error(AdminAuthError::InvalidToken));
    };
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.chars().any(char::is_whitespace)
    {
        return Err(map_auth_error(AdminAuthError::InvalidToken));
    }
    control_plane
        .authenticate_remote_token(token)
        .await
        .map_err(map_auth_error)
}

fn map_auth_error(error: AdminAuthError) -> Status {
    Status::unauthenticated(error.to_string())
}

fn map_command_error(error: AdminCommandError) -> Status {
    match error {
        AdminCommandError::InvalidSubject { subject } => {
            Status::unauthenticated(format!("invalid admin subject: subject={subject}"))
        }
        AdminCommandError::PermissionDenied {
            subject,
            permission,
        } => Status::permission_denied(format!(
            "permission denied: subject={} permission={}",
            subject.principal_id(),
            permission.as_str()
        )),
        AdminCommandError::Runtime(RuntimeError::Config(message))
        | AdminCommandError::Runtime(RuntimeError::Unsupported(message)) => {
            Status::failed_precondition(message)
        }
        AdminCommandError::Runtime(error) => Status::internal(error.to_string()),
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
                admin_ui_count: count_to_u64(plugin_host.admin_ui_count),
                active_quarantine_count: count_to_u64(plugin_host.active_quarantine_count),
                artifact_quarantine_count: count_to_u64(plugin_host.artifact_quarantine_count),
                pending_fatal_error: plugin_host.pending_fatal_error,
            }),
        upgrade: status.upgrade.map(|upgrade| proto::RuntimeUpgradeStateView {
            role: map_upgrade_role(upgrade.role),
            phase: map_upgrade_phase(upgrade.phase),
        }),
    }
}

fn map_upgrade_role(role: RuntimeUpgradeRole) -> i32 {
    match role {
        RuntimeUpgradeRole::Parent => proto::RuntimeUpgradeRole::Parent as i32,
        RuntimeUpgradeRole::Child => proto::RuntimeUpgradeRole::Child as i32,
    }
}

fn map_upgrade_phase(phase: RuntimeUpgradePhase) -> i32 {
    match phase {
        RuntimeUpgradePhase::ParentFreezing => proto::RuntimeUpgradePhase::ParentFreezing as i32,
        RuntimeUpgradePhase::ParentWaitingChildReady => {
            proto::RuntimeUpgradePhase::ParentWaitingChildReady as i32
        }
        RuntimeUpgradePhase::ParentRollingBack => {
            proto::RuntimeUpgradePhase::ParentRollingBack as i32
        }
        RuntimeUpgradePhase::ChildWaitingCommit => {
            proto::RuntimeUpgradePhase::ChildWaitingCommit as i32
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use mc_plugin_test_support::PackagedPluginHarness;
    use server_runtime::config::{
        AdminGrpcPrincipalConfig, AdminPermission as ConfigAdminPermission, ServerConfig,
        ServerConfigSource,
    };
    use server_runtime::runtime::ServerSupervisor;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tonic::metadata::MetadataValue;

    fn grpc_test_config() -> Result<ServerConfig, RuntimeError> {
        let harness = PackagedPluginHarness::shared()
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        let mut config = ServerConfig::default();
        config.bootstrap.world_dir = std::env::temp_dir().join(format!(
            "revy-admin-grpc-test-world-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        config.bootstrap.plugins_dir = harness.dist_dir().to_path_buf();
        config.network.server_ip = Some("127.0.0.1".parse().expect("loopback should parse"));
        config.network.server_port = 0;
        config.topology.be_enabled = false;
        config.plugins.allowlist = Some(vec![
            "je-5".to_string(),
            "gameplay-canonical".to_string(),
            "gameplay-readonly".to_string(),
            "storage-je-anvil-1_7_10".to_string(),
            "auth-offline".to_string(),
            "admin-ui-console".to_string(),
        ]);
        config.admin.grpc.enabled = true;
        config.admin.grpc.bind_addr = "127.0.0.1:0"
            .parse()
            .expect("loopback gRPC addr should parse");
        config.admin.grpc.principals.insert(
            "ops-status".to_string(),
            AdminGrpcPrincipalConfig {
                token_file: PathBuf::from("runtime/admin/ops-status.token"),
                token: "status-token".to_string(),
                permissions: vec![ConfigAdminPermission::Status],
            },
        );
        config.admin.grpc.principals.insert(
            "ops-admin".to_string(),
            AdminGrpcPrincipalConfig {
                token_file: PathBuf::from("runtime/admin/ops-admin.token"),
                token: "admin-token".to_string(),
                permissions: vec![
                    ConfigAdminPermission::Status,
                    ConfigAdminPermission::Sessions,
                    ConfigAdminPermission::ReloadRuntime,
                    ConfigAdminPermission::Shutdown,
                ],
            },
        );
        Ok(config)
    }

    async fn start_grpc_test_server(
        config: ServerConfig,
    ) -> Result<(ServerSupervisor, watch::Sender<bool>, AdminGrpcServerHandle), RuntimeError> {
        let server = ServerSupervisor::boot(ServerConfigSource::Inline(config)).await?;
        let control_plane = server.admin_control_plane();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let grpc = spawn_admin_grpc_server(
            server
                .admin_grpc_bind_addr()
                .expect("test config should enable admin gRPC"),
            control_plane,
            shutdown_tx.clone(),
            shutdown_rx,
        )
        .await?;
        Ok((server, shutdown_tx, grpc))
    }

    async fn grpc_client(
        local_addr: SocketAddr,
    ) -> Result<
        proto::admin_control_plane_client::AdminControlPlaneClient<tonic::transport::Channel>,
        tonic::transport::Error,
    > {
        proto::admin_control_plane_client::AdminControlPlaneClient::connect(format!(
            "http://{local_addr}"
        ))
        .await
    }

    fn authorized_request<T>(message: T, token: &str) -> Request<T> {
        let mut request = Request::new(message);
        request.metadata_mut().insert(
            "authorization",
            MetadataValue::try_from(format!("Bearer {token}"))
                .expect("bearer token metadata should be valid"),
        );
        request
    }

    fn request_with_authorization_values<T>(message: T, values: &[&str]) -> Request<T> {
        let mut request = Request::new(message);
        for value in values {
            request.metadata_mut().append(
                "authorization",
                MetadataValue::try_from(*value).expect("authorization metadata should be valid"),
            );
        }
        request
    }

    #[tokio::test]
    async fn grpc_admin_control_plane_authenticates_and_enforces_permissions()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = grpc_test_config()?;
        let (server, shutdown_tx, grpc) = start_grpc_test_server(config).await?;
        let local_addr = grpc.local_addr();
        let mut client = grpc_client(local_addr).await?;

        let missing = client
            .get_status(Request::new(proto::GetStatusRequest {}))
            .await;
        assert_eq!(
            missing.expect_err("missing token should fail").code(),
            tonic::Code::Unauthenticated
        );

        let invalid = client
            .get_status(authorized_request(
                proto::GetStatusRequest {},
                "wrong-token",
            ))
            .await;
        assert_eq!(
            invalid.expect_err("invalid token should fail").code(),
            tonic::Code::Unauthenticated
        );

        let malformed_spacing = client
            .get_status(request_with_authorization_values(
                proto::GetStatusRequest {},
                &["Bearer  status-token"],
            ))
            .await;
        assert_eq!(
            malformed_spacing
                .expect_err("malformed authorization spacing should fail")
                .code(),
            tonic::Code::Unauthenticated
        );

        let duplicated = client
            .get_status(request_with_authorization_values(
                proto::GetStatusRequest {},
                &["Bearer status-token", "Bearer admin-token"],
            ))
            .await;
        assert_eq!(
            duplicated
                .expect_err("multiple authorization headers should fail")
                .code(),
            tonic::Code::Unauthenticated
        );

        let status = client
            .get_status(authorized_request(
                proto::GetStatusRequest {},
                "status-token",
            ))
            .await?
            .into_inner();
        assert!(status.status.is_some());

        let denied = client
            .reload_runtime(authorized_request(
                proto::ReloadRuntimeRequest {
                    mode: proto::RuntimeReloadMode::Artifacts as i32,
                },
                "status-token",
            ))
            .await;
        assert_eq!(
            denied
                .expect_err("permission-mismatched token should fail")
                .code(),
            tonic::Code::PermissionDenied
        );

        let _ = shutdown_tx.send(true);
        grpc.join().await?;
        server.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn grpc_admin_control_plane_supports_reload_and_shutdown_rpcs()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = grpc_test_config()?;
        let (server, _shutdown_tx, grpc) = start_grpc_test_server(config).await?;
        let local_addr = grpc.local_addr();
        let mut client = grpc_client(local_addr).await?;

        let status = client
            .get_status(authorized_request(
                proto::GetStatusRequest {},
                "admin-token",
            ))
            .await?
            .into_inner();
        assert!(status.status.is_some());

        let sessions = client
            .list_sessions(authorized_request(
                proto::ListSessionsRequest {},
                "admin-token",
            ))
            .await?
            .into_inner();
        assert!(sessions.sessions.is_some());

        let reload_artifacts = client
            .reload_runtime(authorized_request(
                proto::ReloadRuntimeRequest {
                    mode: proto::RuntimeReloadMode::Artifacts as i32,
                },
                "admin-token",
            ))
            .await?
            .into_inner();
        assert!(reload_artifacts.result.is_some());

        let reload_topology = client
            .reload_runtime(authorized_request(
                proto::ReloadRuntimeRequest {
                    mode: proto::RuntimeReloadMode::Topology as i32,
                },
                "admin-token",
            ))
            .await?
            .into_inner();
        assert!(reload_topology.result.is_some());

        let reload_full = client
            .reload_runtime(authorized_request(
                proto::ReloadRuntimeRequest {
                    mode: proto::RuntimeReloadMode::Full as i32,
                },
                "admin-token",
            ))
            .await?
            .into_inner();
        assert!(reload_full.result.is_some());

        let reload_core = client
            .reload_runtime(authorized_request(
                proto::ReloadRuntimeRequest {
                    mode: proto::RuntimeReloadMode::Core as i32,
                },
                "admin-token",
            ))
            .await?
            .into_inner();
        assert!(reload_core.result.is_some());

        let shutdown = client
            .shutdown(authorized_request(proto::ShutdownRequest {}, "admin-token"))
            .await?
            .into_inner();
        let _ = shutdown;

        grpc.join().await?;
        server.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn grpc_bind_conflict_is_a_startup_failure() -> Result<(), Box<dyn std::error::Error>> {
        let config = grpc_test_config()?;
        let server = ServerSupervisor::boot(ServerConfigSource::Inline(config.clone())).await?;
        let control_plane = server.admin_control_plane();
        let occupied = TcpListener::bind("127.0.0.1:0").await?;
        let occupied_addr = occupied.local_addr()?;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let error = spawn_admin_grpc_server(occupied_addr, control_plane, shutdown_tx, shutdown_rx)
            .await
            .expect_err("conflicting bind should fail");
        assert!(matches!(
            error,
            RuntimeError::Config(message) if message.contains("failed to bind admin gRPC listener")
        ));

        drop(occupied);
        server.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn grpc_non_loopback_bind_requires_explicit_opt_in()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut config = grpc_test_config()?;
        config.admin.grpc.bind_addr = "0.0.0.0:0"
            .parse()
            .expect("non-loopback gRPC addr should parse");

        let error = match start_grpc_test_server(config).await {
            Ok(_) => panic!("non-loopback gRPC bind should require explicit opt-in"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            RuntimeError::Config(message)
                if message.contains("admin.grpc.allow_non_loopback=true")
        ));
        Ok(())
    }

    #[tokio::test]
    async fn grpc_non_loopback_bind_allows_explicit_opt_in()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut config = grpc_test_config()?;
        config.admin.grpc.bind_addr = "0.0.0.0:0"
            .parse()
            .expect("non-loopback gRPC addr should parse");
        config.admin.grpc.allow_non_loopback = true;

        let (server, shutdown_tx, grpc) = start_grpc_test_server(config).await?;
        assert!(
            !grpc.local_addr().ip().is_loopback(),
            "explicit opt-in should allow non-loopback admin gRPC binding"
        );

        let _ = shutdown_tx.send(true);
        grpc.join().await?;
        server.shutdown().await?;
        Ok(())
    }
}
