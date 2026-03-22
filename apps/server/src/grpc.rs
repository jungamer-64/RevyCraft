use revy_admin_grpc::admin::{
    self as proto, GetStatusResponse, ListSessionsResponse, ReloadConfigResponse,
    ReloadPluginsResponse, ReloadTopologyResponse, ShutdownResponse,
    admin_control_plane_server::{AdminControlPlane, AdminControlPlaneServer},
};
use server_runtime::RuntimeError;
use server_runtime::runtime::{
    AdminAuthError, AdminCommandError, AdminConfigReloadView, AdminControlPlaneHandle,
    AdminNamedCountView, AdminSessionSummaryView, AdminSessionsView, AdminStatusView, AdminSubject,
    AdminTopologyReloadView,
};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

#[derive(Debug)]
pub(crate) struct AdminGrpcServerHandle {
    local_addr: SocketAddr,
    join_handle: JoinHandle<Result<(), RuntimeError>>,
}

impl AdminGrpcServerHandle {
    #[must_use]
    pub(crate) const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub(crate) async fn join(self) -> Result<(), RuntimeError> {
        self.join_handle.await?
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

    async fn reload_config(
        &self,
        request: Request<proto::ReloadConfigRequest>,
    ) -> Result<Response<ReloadConfigResponse>, Status> {
        let subject = authenticate_request(&self.control_plane, request.metadata()).await?;
        self.control_plane
            .reload_config(&subject)
            .await
            .map(|result| {
                Response::new(ReloadConfigResponse {
                    result: Some(map_config_reload_view(result)),
                })
            })
            .map_err(map_command_error)
    }

    async fn reload_plugins(
        &self,
        request: Request<proto::ReloadPluginsRequest>,
    ) -> Result<Response<ReloadPluginsResponse>, Status> {
        let subject = authenticate_request(&self.control_plane, request.metadata()).await?;
        self.control_plane
            .reload_plugins(&subject)
            .await
            .map(|result| {
                Response::new(ReloadPluginsResponse {
                    result: Some(proto::AdminPluginsReloadView {
                        reloaded_plugin_ids: result.reloaded_plugin_ids,
                    }),
                })
            })
            .map_err(map_command_error)
    }

    async fn reload_topology(
        &self,
        request: Request<proto::ReloadTopologyRequest>,
    ) -> Result<Response<ReloadTopologyResponse>, Status> {
        let subject = authenticate_request(&self.control_plane, request.metadata()).await?;
        self.control_plane
            .reload_topology(&subject)
            .await
            .map(|result| {
                Response::new(ReloadTopologyResponse {
                    result: Some(map_topology_reload_view(result)),
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
    let listener = TcpListener::bind(bind_addr).await.map_err(|error| {
        RuntimeError::Config(format!(
            "failed to bind admin gRPC listener on {bind_addr}: {error}"
        ))
    })?;
    let local_addr = listener.local_addr()?;
    let service = AdminGrpcService {
        control_plane,
        shutdown_tx,
    };
    let join_handle = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AdminControlPlaneServer::new(service))
            .serve_with_incoming_shutdown(
                TcpListenerStream::new(listener),
                wait_for_shutdown_signal(shutdown_rx),
            )
            .await
            .map_err(|error| RuntimeError::Config(format!("admin gRPC server failed: {error}")))
    });
    Ok(AdminGrpcServerHandle {
        local_addr,
        join_handle,
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
        by_topology_generation: summary
            .by_topology_generation
            .into_iter()
            .map(|count| proto::AdminTopologyGenerationCountView {
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
        active_topology_generation_id: status.active_topology_generation_id,
        draining_topology_generation_ids: status.draining_topology_generation_ids,
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
                topology_generation_id: session.topology_generation_id,
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

fn map_config_reload_view(result: AdminConfigReloadView) -> proto::AdminConfigReloadView {
    proto::AdminConfigReloadView {
        reloaded_plugin_ids: result.reloaded_plugin_ids,
        topology: Some(map_topology_reload_view(result.topology)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_plugin_host::host::plugin_host_from_config;
    use mc_plugin_test_support::PackagedPluginHarness;
    use server_runtime::config::{AdminGrpcPrincipalConfig, ServerConfig, ServerConfigSource};
    use server_runtime::runtime::{AdminPermission, ServerBuilder};
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
            "je-1_7_10".to_string(),
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
                permissions: vec![AdminPermission::Status],
            },
        );
        config.admin.grpc.principals.insert(
            "ops-admin".to_string(),
            AdminGrpcPrincipalConfig {
                token_file: PathBuf::from("runtime/admin/ops-admin.token"),
                token: "admin-token".to_string(),
                permissions: vec![
                    AdminPermission::Status,
                    AdminPermission::Sessions,
                    AdminPermission::ReloadConfig,
                    AdminPermission::ReloadPlugins,
                    AdminPermission::ReloadTopology,
                    AdminPermission::Shutdown,
                ],
            },
        );
        Ok(config)
    }

    async fn start_grpc_test_server(
        config: ServerConfig,
    ) -> Result<
        (
            server_runtime::runtime::ReloadableRunningServer,
            watch::Sender<bool>,
            AdminGrpcServerHandle,
        ),
        RuntimeError,
    > {
        let plugin_host = plugin_host_from_config(&config.plugin_host_bootstrap_config())?
            .ok_or_else(|| RuntimeError::Config("packaged plugin host should exist".to_string()))?;
        let loaded_plugins =
            plugin_host.load_plugin_set(&config.plugin_host_runtime_selection_config())?;
        let server = ServerBuilder::new(ServerConfigSource::Inline(config.clone()), loaded_plugins)
            .with_reload_host(plugin_host)
            .build()
            .await?;
        let control_plane = server.admin_control_plane();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let grpc = spawn_admin_grpc_server(
            config.admin.grpc.bind_addr,
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
            .reload_plugins(authorized_request(
                proto::ReloadPluginsRequest {},
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

        let reload_config = client
            .reload_config(authorized_request(
                proto::ReloadConfigRequest {},
                "admin-token",
            ))
            .await?
            .into_inner();
        assert!(reload_config.result.is_some());

        let reload_plugins = client
            .reload_plugins(authorized_request(
                proto::ReloadPluginsRequest {},
                "admin-token",
            ))
            .await?
            .into_inner();
        assert!(reload_plugins.result.is_some());

        let reload_topology = client
            .reload_topology(authorized_request(
                proto::ReloadTopologyRequest {},
                "admin-token",
            ))
            .await?
            .into_inner();
        assert!(reload_topology.result.is_some());

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
        let plugin_host = plugin_host_from_config(&config.plugin_host_bootstrap_config())?
            .ok_or_else(|| RuntimeError::Config("packaged plugin host should exist".to_string()))?;
        let loaded_plugins =
            plugin_host.load_plugin_set(&config.plugin_host_runtime_selection_config())?;
        let server = ServerBuilder::new(ServerConfigSource::Inline(config.clone()), loaded_plugins)
            .with_reload_host(plugin_host)
            .build()
            .await?;
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
