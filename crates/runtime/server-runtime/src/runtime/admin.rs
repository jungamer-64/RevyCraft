use super::{
    AdminArtifactsReloadView, AdminCoreReloadView, AdminFullReloadView, AdminGenerationCountView,
    AdminListenerBindingView, AdminNamedCountView, AdminPermission, AdminPhaseCountView,
    AdminPluginHostView, AdminPrincipal, AdminRequest, AdminResponse, AdminRuntimeReloadDetail,
    AdminRuntimeReloadView, AdminSessionSummaryView, AdminSessionView, AdminSessionsView,
    AdminStatusView, AdminTopologyReloadView, AdminTransportCountView, AdminUpgradeRuntimeView,
    RuntimeReloadMode, RuntimeReloadResult, RuntimeServer,
};
use crate::RuntimeError;
use crate::runtime::selection::AdminCredentialTag;
use mc_plugin_api::codec::admin_ui as plugin_admin;
use std::future::Future;
use std::fmt::{Debug, Display, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use thiserror::Error;

pub type RuntimeUpgradeFuture =
    Pin<Box<dyn Future<Output = Result<AdminUpgradeRuntimeView, RuntimeError>> + Send>>;
pub type RuntimeUpgradeCallback =
    Arc<dyn Fn(AdminSubject, String) -> RuntimeUpgradeFuture + Send + Sync>;

#[derive(Clone, PartialEq, Eq)]
pub struct AdminSubject {
    kind: AdminSubjectKind,
}

impl AdminSubject {
    #[must_use]
    pub fn is_local_console(&self) -> bool {
        matches!(&self.kind, AdminSubjectKind::LocalConsole)
    }

    #[must_use]
    pub fn principal_id(&self) -> &str {
        match &self.kind {
            AdminSubjectKind::LocalConsole => AdminPrincipal::LocalConsole.as_str(),
            AdminSubjectKind::Remote(remote) => remote.principal_id.as_str(),
        }
    }

    #[must_use]
    pub(crate) fn local_console() -> Self {
        Self {
            kind: AdminSubjectKind::LocalConsole,
        }
    }

    #[must_use]
    pub(crate) fn remote(token: &str, principal_id: impl Into<String>) -> Self {
        Self {
            kind: AdminSubjectKind::Remote(RemoteAdminSubject {
                credential_tag: AdminCredentialTag::from_token(token),
                principal_id: principal_id.into(),
            }),
        }
    }
}

impl Debug for AdminSubject {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            AdminSubjectKind::LocalConsole => f
                .debug_struct("AdminSubject")
                .field("kind", &"local-console")
                .finish(),
            AdminSubjectKind::Remote(remote) => f
                .debug_struct("AdminSubject")
                .field("kind", &"remote")
                .field("principal_id", &remote.principal_id)
                .field("credential_tag", &remote.credential_tag)
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum AdminSubjectKind {
    LocalConsole,
    Remote(RemoteAdminSubject),
}

#[derive(Clone, PartialEq, Eq)]
struct RemoteAdminSubject {
    credential_tag: AdminCredentialTag,
    principal_id: String,
}

impl Display for AdminSubject {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.principal_id())
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AdminAuthError {
    #[error("missing remote admin token")]
    MissingToken,
    #[error("invalid remote admin token")]
    InvalidToken,
}

#[derive(Debug, Error)]
pub enum AdminCommandError {
    #[error("invalid admin subject: subject={subject}")]
    InvalidSubject { subject: AdminSubject },
    #[error("permission denied: subject={subject} permission={permission:?}")]
    PermissionDenied {
        subject: AdminSubject,
        permission: AdminPermission,
    },
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

#[derive(Clone)]
pub struct AdminControlPlaneHandle {
    service: AdminService,
    ui_adapter: AdminUiAdapter,
}

#[derive(Clone)]
struct AdminService {
    runtime: Arc<RuntimeServer>,
    upgrade_callback: Option<RuntimeUpgradeCallback>,
}

#[derive(Clone)]
struct AdminUiAdapter {
    runtime: Arc<RuntimeServer>,
}

impl AdminControlPlaneHandle {
    pub(crate) fn new(runtime: Arc<RuntimeServer>) -> Self {
        Self {
            service: AdminService {
                runtime: Arc::clone(&runtime),
                upgrade_callback: None,
            },
            ui_adapter: AdminUiAdapter { runtime },
        }
    }

    #[must_use]
    pub fn with_runtime_upgrader(mut self, upgrade_callback: RuntimeUpgradeCallback) -> Self {
        self.service.upgrade_callback = Some(upgrade_callback);
        self
    }

    pub async fn authenticate_remote_token(
        &self,
        token: &str,
    ) -> Result<AdminSubject, AdminAuthError> {
        self.service.authenticate_remote_token(token).await
    }

    pub async fn status(
        &self,
        subject: &AdminSubject,
    ) -> Result<AdminStatusView, AdminCommandError> {
        self.service.status(subject).await
    }

    pub async fn sessions(
        &self,
        subject: &AdminSubject,
    ) -> Result<AdminSessionsView, AdminCommandError> {
        self.service.sessions(subject).await
    }

    pub async fn reload_runtime(
        &self,
        subject: &AdminSubject,
        mode: RuntimeReloadMode,
    ) -> Result<AdminRuntimeReloadView, AdminCommandError> {
        self.service.reload_runtime(subject, mode).await
    }

    pub async fn upgrade_runtime(
        &self,
        subject: &AdminSubject,
        executable_path: String,
    ) -> Result<AdminUpgradeRuntimeView, AdminCommandError> {
        self.service.upgrade_runtime(subject, executable_path).await
    }

    pub async fn shutdown(&self, subject: &AdminSubject) -> Result<(), AdminCommandError> {
        self.service.shutdown(subject).await
    }

    pub async fn parse_local_command(&self, line: &str) -> Result<AdminRequest, String> {
        self.ui_adapter.parse_local_command(line).await
    }

    pub async fn render_local_response(&self, response: &AdminResponse) -> Result<String, String> {
        self.ui_adapter.render_local_response(response).await
    }

    pub async fn execute_local_console(&self, request: AdminRequest) -> AdminResponse {
        self.service.execute_local_console(request).await
    }

    pub async fn execute(&self, principal: AdminPrincipal, request: AdminRequest) -> AdminResponse {
        self.service.execute(principal, request).await
    }
}

impl AdminControlPlaneHandle {
    fn local_response_from_error(error: AdminCommandError) -> AdminResponse {
        match error {
            AdminCommandError::PermissionDenied { permission, .. } => {
                AdminResponse::PermissionDenied {
                    principal: AdminPrincipal::LocalConsole,
                    permission,
                }
            }
            AdminCommandError::InvalidSubject { subject } => AdminResponse::Error {
                message: format!("invalid admin subject: subject={subject}"),
            },
            AdminCommandError::Runtime(error) => AdminResponse::Error {
                message: error.to_string(),
            },
        }
    }
}

impl AdminService {
    async fn authenticate_remote_token(&self, token: &str) -> Result<AdminSubject, AdminAuthError> {
        self.runtime.authenticate_remote_token(token).await
    }

    async fn status(&self, subject: &AdminSubject) -> Result<AdminStatusView, AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::Status)
            .await?;
        Ok(self.runtime.admin_status_view().await)
    }

    async fn sessions(
        &self,
        subject: &AdminSubject,
    ) -> Result<AdminSessionsView, AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::Sessions)
            .await?;
        Ok(self.runtime.admin_sessions_view().await)
    }

    async fn reload_runtime(
        &self,
        subject: &AdminSubject,
        mode: RuntimeReloadMode,
    ) -> Result<AdminRuntimeReloadView, AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::ReloadRuntime)
            .await?;
        self.runtime
            .admin_reload_runtime_view(mode)
            .await
            .map_err(Into::into)
    }

    async fn shutdown(&self, subject: &AdminSubject) -> Result<(), AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::Shutdown)
            .await?;
        let _ = self.runtime.request_shutdown();
        Ok(())
    }

    async fn upgrade_runtime(
        &self,
        subject: &AdminSubject,
        executable_path: String,
    ) -> Result<AdminUpgradeRuntimeView, AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::UpgradeRuntime)
            .await?;
        let Some(upgrade_callback) = &self.upgrade_callback else {
            return Err(RuntimeError::Config(
                "runtime upgrade is unavailable without an app-level upgrade coordinator"
                    .to_string(),
            )
            .into());
        };
        upgrade_callback(subject.clone(), executable_path)
            .await
            .map_err(Into::into)
    }

    async fn execute_local_console(&self, request: AdminRequest) -> AdminResponse {
        let subject = AdminSubject::local_console();
        match request {
            AdminRequest::Help => AdminResponse::Help,
            AdminRequest::Status => self
                .status(&subject)
                .await
                .map(AdminResponse::Status)
                .unwrap_or_else(AdminControlPlaneHandle::local_response_from_error),
            AdminRequest::Sessions => self
                .sessions(&subject)
                .await
                .map(AdminResponse::Sessions)
                .unwrap_or_else(AdminControlPlaneHandle::local_response_from_error),
            AdminRequest::ReloadRuntime { mode } => self
                .reload_runtime(&subject, mode)
                .await
                .map(AdminResponse::ReloadRuntime)
                .unwrap_or_else(AdminControlPlaneHandle::local_response_from_error),
            AdminRequest::UpgradeRuntime { executable_path } => self
                .upgrade_runtime(&subject, executable_path)
                .await
                .map(AdminResponse::UpgradeRuntime)
                .unwrap_or_else(AdminControlPlaneHandle::local_response_from_error),
            AdminRequest::Shutdown => self
                .shutdown(&subject)
                .await
                .map(|()| AdminResponse::ShutdownScheduled)
                .unwrap_or_else(AdminControlPlaneHandle::local_response_from_error),
        }
    }

    async fn execute(&self, principal: AdminPrincipal, request: AdminRequest) -> AdminResponse {
        match principal {
            AdminPrincipal::LocalConsole => self.execute_local_console(request).await,
        }
    }
}

impl AdminUiAdapter {
    async fn parse_local_command(&self, line: &str) -> Result<AdminRequest, String> {
        if let Some(ui) = self.runtime.current_admin_ui().await {
            return ui
                .parse_line(line)
                .map(runtime_request_from_plugin_request)
                .map_err(|error| error.to_string());
        }
        parse_builtin_local_command(line)
    }

    async fn render_local_response(&self, response: &AdminResponse) -> Result<String, String> {
        if let Some(ui) = self.runtime.current_admin_ui().await {
            return ui
                .render_response(&plugin_response_from_runtime_response(response))
                .map_err(|error| error.to_string());
        }
        Ok(render_builtin_local_response(response))
    }
}

impl RuntimeServer {
    pub(crate) async fn current_admin_ui(
        &self,
    ) -> Option<Arc<dyn mc_plugin_host::runtime::AdminUiProfileHandle>> {
        self.selection.current_admin_ui().await
    }

    pub(crate) fn request_shutdown(&self) -> bool {
        self.reload.request_shutdown()
    }

    pub(crate) async fn authenticate_remote_token(
        &self,
        token: &str,
    ) -> Result<AdminSubject, AdminAuthError> {
        let token = token.trim();
        if token.is_empty() {
            return Err(AdminAuthError::InvalidToken);
        }
        self.selection_state()
            .await
            .remote_admin_subjects
            .get(token)
            .map(|principal| AdminSubject::remote(token, principal.principal_id.clone()))
            .ok_or(AdminAuthError::InvalidToken)
    }

    async fn authorize(
        &self,
        subject: &AdminSubject,
        permission: AdminPermission,
    ) -> Result<(), AdminCommandError> {
        let selection_state = self.selection_state().await;
        match &subject.kind {
            AdminSubjectKind::LocalConsole => {
                if selection_state
                    .config
                    .admin
                    .local_console_permissions
                    .iter()
                    .any(|configured| runtime_permission_from_config(*configured) == permission)
                {
                    Ok(())
                } else {
                    Err(AdminCommandError::PermissionDenied {
                        subject: subject.clone(),
                        permission,
                    })
                }
            }
            AdminSubjectKind::Remote(remote) => {
                let Some(principal) =
                    selection_state
                        .remote_admin_subjects
                        .values()
                        .find(|principal| {
                            principal.principal_id == remote.principal_id
                                && principal.credential_tag == remote.credential_tag
                        })
                else {
                    return Err(AdminCommandError::InvalidSubject {
                        subject: subject.clone(),
                    });
                };
                if principal.permissions.contains(&permission) {
                    Ok(())
                } else {
                    Err(AdminCommandError::PermissionDenied {
                        subject: subject.clone(),
                        permission,
                    })
                }
            }
        }
    }

    async fn admin_status_view(&self) -> AdminStatusView {
        let snapshot = self.status_snapshot().await;
        AdminStatusView {
            active_generation_id: snapshot.active_generation.generation_id.0,
            draining_generation_ids: snapshot
                .draining_generations
                .iter()
                .map(|generation| generation.generation_id.0)
                .collect(),
            listener_bindings: snapshot
                .listener_bindings
                .into_iter()
                .map(|binding| AdminListenerBindingView {
                    transport: binding.transport,
                    local_addr: binding.local_addr.to_string(),
                    adapter_ids: binding
                        .adapter_ids
                        .into_iter()
                        .map(|adapter_id| adapter_id.to_string())
                        .collect(),
                })
                .collect(),
            default_adapter_id: snapshot.active_generation.default_adapter_id,
            default_bedrock_adapter_id: snapshot.active_generation.default_bedrock_adapter_id,
            enabled_adapter_ids: snapshot.active_generation.enabled_adapter_ids,
            enabled_bedrock_adapter_ids: snapshot.active_generation.enabled_bedrock_adapter_ids,
            motd: snapshot.active_generation.motd,
            max_players: snapshot.active_generation.max_players,
            session_summary: AdminSessionSummaryView {
                total: snapshot.session_summary.total,
                by_transport: snapshot
                    .session_summary
                    .by_transport
                    .into_iter()
                    .map(|count| AdminTransportCountView {
                        transport: count.transport,
                        count: count.count,
                    })
                    .collect(),
                by_phase: snapshot
                    .session_summary
                    .by_phase
                    .into_iter()
                    .map(|count| AdminPhaseCountView {
                        phase: count.phase,
                        count: count.count,
                    })
                    .collect(),
                by_generation: snapshot
                    .session_summary
                    .by_generation
                    .into_iter()
                    .map(|count| AdminGenerationCountView {
                        generation_id: count.generation_id.0,
                        count: count.count,
                    })
                    .collect(),
                by_adapter_id: snapshot
                    .session_summary
                    .by_adapter_id
                    .into_iter()
                    .map(|count| AdminNamedCountView {
                        value: count.value,
                        count: count.count,
                    })
                    .collect(),
                by_gameplay_profile: snapshot
                    .session_summary
                    .by_gameplay_profile
                    .into_iter()
                    .map(|count| AdminNamedCountView {
                        value: count.value,
                        count: count.count,
                    })
                    .collect(),
            },
            dirty: snapshot.dirty,
            plugin_host: snapshot.plugin_host.map(|status| AdminPluginHostView {
                protocol_count: status.protocol_count,
                gameplay_count: status.gameplay_count,
                storage_count: status.storage_count,
                auth_count: status.auth_count,
                admin_ui_count: status.admin_ui_count,
                active_quarantine_count: status.active_quarantine_count,
                artifact_quarantine_count: status.artifact_quarantine_count,
                pending_fatal_error: status.pending_fatal_error,
            }),
        }
    }

    async fn admin_sessions_view(&self) -> AdminSessionsView {
        let sessions = self.session_status_snapshot().await;
        let summary = super::status::summarize_sessions(&sessions);
        AdminSessionsView {
            summary: AdminSessionSummaryView {
                total: summary.total,
                by_transport: summary
                    .by_transport
                    .into_iter()
                    .map(|count| AdminTransportCountView {
                        transport: count.transport,
                        count: count.count,
                    })
                    .collect(),
                by_phase: summary
                    .by_phase
                    .into_iter()
                    .map(|count| AdminPhaseCountView {
                        phase: count.phase,
                        count: count.count,
                    })
                    .collect(),
                by_generation: summary
                    .by_generation
                    .into_iter()
                    .map(|count| AdminGenerationCountView {
                        generation_id: count.generation_id.0,
                        count: count.count,
                    })
                    .collect(),
                by_adapter_id: summary
                    .by_adapter_id
                    .into_iter()
                    .map(|count| AdminNamedCountView {
                        value: count.value,
                        count: count.count,
                    })
                    .collect(),
                by_gameplay_profile: summary
                    .by_gameplay_profile
                    .into_iter()
                    .map(|count| AdminNamedCountView {
                        value: count.value,
                        count: count.count,
                    })
                    .collect(),
            },
            sessions: sessions
                .into_iter()
                .map(|session| AdminSessionView {
                    connection_id: session.connection_id,
                    generation_id: session.generation_id.0,
                    transport: session.transport,
                    phase: session.phase,
                    adapter_id: session.adapter_id,
                    gameplay_profile: session.gameplay_profile,
                    player_id: session.player_id,
                    entity_id: session.entity_id,
                    protocol_generation: session.protocol_generation,
                    gameplay_generation: session.gameplay_generation,
                })
                .collect(),
        }
    }

    async fn admin_reload_runtime_view(
        &self,
        mode: RuntimeReloadMode,
    ) -> Result<AdminRuntimeReloadView, RuntimeError> {
        let Some(reload_host) = self.reload.reload_host() else {
            return Err(RuntimeError::Config(
                "runtime reload is unavailable without a reload-capable host".to_string(),
            ));
        };
        let detail = match self.reload_runtime(reload_host.as_ref(), mode).await? {
            RuntimeReloadResult::Artifacts(result) => {
                AdminRuntimeReloadDetail::Artifacts(AdminArtifactsReloadView {
                    reloaded_plugin_ids: result.reloaded_plugin_ids,
                })
            }
            RuntimeReloadResult::Topology(result) => {
                AdminRuntimeReloadDetail::Topology(admin_topology_reload_view(result))
            }
            RuntimeReloadResult::Core(_result) => {
                AdminRuntimeReloadDetail::Core(AdminCoreReloadView {})
            }
            RuntimeReloadResult::Full(result) => {
                AdminRuntimeReloadDetail::Full(AdminFullReloadView {
                    reloaded_plugin_ids: result.reloaded_plugin_ids,
                    topology: admin_topology_reload_view(result.topology),
                })
            }
        };
        Ok(AdminRuntimeReloadView { mode, detail })
    }
}

fn runtime_request_from_plugin_request(request: plugin_admin::AdminRequest) -> AdminRequest {
    match request {
        plugin_admin::AdminRequest::Help => AdminRequest::Help,
        plugin_admin::AdminRequest::Status => AdminRequest::Status,
        plugin_admin::AdminRequest::Sessions => AdminRequest::Sessions,
        plugin_admin::AdminRequest::ReloadRuntime { mode } => AdminRequest::ReloadRuntime {
            mode: runtime_reload_mode_from_plugin(mode),
        },
        plugin_admin::AdminRequest::UpgradeRuntime { executable_path } => {
            AdminRequest::UpgradeRuntime { executable_path }
        }
        plugin_admin::AdminRequest::Shutdown => AdminRequest::Shutdown,
    }
}

fn parse_builtin_local_command(line: &str) -> Result<AdminRequest, String> {
    let trimmed = line.trim();
    let normalized = trimmed
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    match normalized.as_str() {
        "" => Err("empty command".to_string()),
        "help" | "?" => Ok(AdminRequest::Help),
        "status" => Ok(AdminRequest::Status),
        "sessions" => Ok(AdminRequest::Sessions),
        "reload runtime artifacts" => Ok(AdminRequest::ReloadRuntime {
            mode: RuntimeReloadMode::Artifacts,
        }),
        "reload runtime topology" => Ok(AdminRequest::ReloadRuntime {
            mode: RuntimeReloadMode::Topology,
        }),
        "reload runtime core" => Ok(AdminRequest::ReloadRuntime {
            mode: RuntimeReloadMode::Core,
        }),
        "reload runtime full" => Ok(AdminRequest::ReloadRuntime {
            mode: RuntimeReloadMode::Full,
        }),
        _ if normalized.starts_with("upgrade runtime executable ") => {
            let executable_path = trimmed["upgrade runtime executable ".len()..].trim();
            if executable_path.is_empty() {
                Err(format!("unknown command: {line}"))
            } else {
                Ok(AdminRequest::UpgradeRuntime {
                    executable_path: executable_path.to_string(),
                })
            }
        }
        "shutdown" | "stop" => Ok(AdminRequest::Shutdown),
        _ => Err(format!("unknown command: {line}")),
    }
}

fn render_builtin_local_response(response: &AdminResponse) -> String {
    match response {
        AdminResponse::Help => [
            "commands:",
            "  status",
            "  sessions",
            "  reload runtime artifacts",
            "  reload runtime topology",
            "  reload runtime core",
            "  reload runtime full",
            "  upgrade runtime executable <path>",
            "  shutdown",
        ]
        .join("\n"),
        AdminResponse::Status(status) => format!(
            "status: generation={} sessions={} dirty={} motd={:?} max_players={}",
            status.active_generation_id,
            status.session_summary.total,
            status.dirty,
            status.motd,
            status.max_players,
        ),
        AdminResponse::Sessions(sessions) => format!(
            "sessions: total={} listed={}",
            sessions.summary.total,
            sessions.sessions.len(),
        ),
        AdminResponse::ReloadRuntime(result) => render_builtin_runtime_reload_response(result),
        AdminResponse::UpgradeRuntime(result) => format!(
            "upgrade runtime: executable={}",
            result.executable_path
        ),
        AdminResponse::ShutdownScheduled => "shutdown: scheduled".to_string(),
        AdminResponse::PermissionDenied {
            principal,
            permission,
        } => format!(
            "permission denied: principal={} permission={}",
            principal.as_str(),
            permission.as_str(),
        ),
        AdminResponse::Error { message } => format!("error: {message}"),
    }
}

fn render_csv_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values.join(",")
    }
}

fn plugin_principal_from_runtime(principal: AdminPrincipal) -> plugin_admin::AdminPrincipal {
    match principal {
        AdminPrincipal::LocalConsole => plugin_admin::AdminPrincipal::LocalConsole,
    }
}

fn plugin_permission_from_runtime(permission: AdminPermission) -> plugin_admin::AdminPermission {
    match permission {
        AdminPermission::Status => plugin_admin::AdminPermission::Status,
        AdminPermission::Sessions => plugin_admin::AdminPermission::Sessions,
        AdminPermission::ReloadRuntime => plugin_admin::AdminPermission::ReloadRuntime,
        AdminPermission::UpgradeRuntime => plugin_admin::AdminPermission::UpgradeRuntime,
        AdminPermission::Shutdown => plugin_admin::AdminPermission::Shutdown,
    }
}

fn plugin_response_from_runtime_response(response: &AdminResponse) -> plugin_admin::AdminResponse {
    match response {
        AdminResponse::Help => plugin_admin::AdminResponse::Help,
        AdminResponse::Status(status) => {
            plugin_admin::AdminResponse::Status(plugin_status_view_from_runtime(status))
        }
        AdminResponse::Sessions(sessions) => {
            plugin_admin::AdminResponse::Sessions(plugin_sessions_view_from_runtime(sessions))
        }
        AdminResponse::ReloadRuntime(result) => plugin_admin::AdminResponse::ReloadRuntime(
            plugin_runtime_reload_view_from_runtime(result),
        ),
        AdminResponse::UpgradeRuntime(result) => {
            plugin_admin::AdminResponse::UpgradeRuntime(plugin_admin::AdminUpgradeRuntimeView {
                executable_path: result.executable_path.clone(),
            })
        }
        AdminResponse::ShutdownScheduled => plugin_admin::AdminResponse::ShutdownScheduled,
        AdminResponse::PermissionDenied {
            principal,
            permission,
        } => plugin_admin::AdminResponse::PermissionDenied {
            principal: plugin_principal_from_runtime(*principal),
            permission: plugin_permission_from_runtime(*permission),
        },
        AdminResponse::Error { message } => plugin_admin::AdminResponse::Error {
            message: message.clone(),
        },
    }
}

fn plugin_status_view_from_runtime(status: &AdminStatusView) -> plugin_admin::AdminStatusView {
    plugin_admin::AdminStatusView {
        active_generation_id: status.active_generation_id,
        draining_generation_ids: status.draining_generation_ids.clone(),
        listener_bindings: status
            .listener_bindings
            .iter()
            .map(|binding| plugin_admin::AdminListenerBindingView {
                transport: binding.transport,
                local_addr: binding.local_addr.clone(),
                adapter_ids: binding.adapter_ids.clone(),
            })
            .collect(),
        default_adapter_id: status.default_adapter_id.clone(),
        default_bedrock_adapter_id: status.default_bedrock_adapter_id.clone(),
        enabled_adapter_ids: status.enabled_adapter_ids.clone(),
        enabled_bedrock_adapter_ids: status.enabled_bedrock_adapter_ids.clone(),
        motd: status.motd.clone(),
        max_players: status.max_players,
        session_summary: plugin_summary_view_from_runtime(&status.session_summary),
        dirty: status.dirty,
        plugin_host: status.plugin_host.as_ref().map(|plugin_host| {
            plugin_admin::AdminPluginHostView {
                protocol_count: plugin_host.protocol_count,
                gameplay_count: plugin_host.gameplay_count,
                storage_count: plugin_host.storage_count,
                auth_count: plugin_host.auth_count,
                admin_ui_count: plugin_host.admin_ui_count,
                active_quarantine_count: plugin_host.active_quarantine_count,
                artifact_quarantine_count: plugin_host.artifact_quarantine_count,
                pending_fatal_error: plugin_host.pending_fatal_error.clone(),
            }
        }),
    }
}

fn plugin_summary_view_from_runtime(
    summary: &AdminSessionSummaryView,
) -> plugin_admin::AdminSessionSummaryView {
    plugin_admin::AdminSessionSummaryView {
        total: summary.total,
        by_transport: summary
            .by_transport
            .iter()
            .map(|entry| plugin_admin::AdminTransportCountView {
                transport: entry.transport,
                count: entry.count,
            })
            .collect(),
        by_phase: summary
            .by_phase
            .iter()
            .map(|entry| plugin_admin::AdminPhaseCountView {
                phase: entry.phase,
                count: entry.count,
            })
            .collect(),
        by_generation: summary
            .by_generation
            .iter()
            .map(|entry| plugin_admin::AdminGenerationCountView {
                generation_id: entry.generation_id,
                count: entry.count,
            })
            .collect(),
        by_adapter_id: summary
            .by_adapter_id
            .iter()
            .map(|entry| plugin_admin::AdminNamedCountView {
                value: entry.value.clone(),
                count: entry.count,
            })
            .collect(),
        by_gameplay_profile: summary
            .by_gameplay_profile
            .iter()
            .map(|entry| plugin_admin::AdminNamedCountView {
                value: entry.value.clone(),
                count: entry.count,
            })
            .collect(),
    }
}

fn plugin_sessions_view_from_runtime(
    sessions: &AdminSessionsView,
) -> plugin_admin::AdminSessionsView {
    plugin_admin::AdminSessionsView {
        summary: plugin_summary_view_from_runtime(&sessions.summary),
        sessions: sessions
            .sessions
            .iter()
            .map(|session| plugin_admin::AdminSessionView {
                connection_id: session.connection_id,
                generation_id: session.generation_id,
                transport: session.transport,
                phase: session.phase,
                adapter_id: session.adapter_id.clone(),
                gameplay_profile: session.gameplay_profile.clone(),
                player_id: session.player_id,
                entity_id: session.entity_id,
                protocol_generation: session.protocol_generation,
                gameplay_generation: session.gameplay_generation,
            })
            .collect(),
    }
}

const fn runtime_permission_from_config(
    permission: crate::config::AdminPermission,
) -> AdminPermission {
    match permission {
        crate::config::AdminPermission::Status => AdminPermission::Status,
        crate::config::AdminPermission::Sessions => AdminPermission::Sessions,
        crate::config::AdminPermission::ReloadRuntime => AdminPermission::ReloadRuntime,
        crate::config::AdminPermission::UpgradeRuntime => AdminPermission::UpgradeRuntime,
        crate::config::AdminPermission::Shutdown => AdminPermission::Shutdown,
    }
}

fn admin_topology_reload_view(result: super::TopologyReloadResult) -> AdminTopologyReloadView {
    AdminTopologyReloadView {
        activated_generation_id: result.activated_generation_id.0,
        retired_generation_ids: result
            .retired_generation_ids
            .into_iter()
            .map(|generation_id| generation_id.0)
            .collect(),
        applied_config_change: result.applied_config_change,
        reconfigured_adapter_ids: result.reconfigured_adapter_ids,
    }
}

fn render_builtin_runtime_reload_response(result: &AdminRuntimeReloadView) -> String {
    match &result.detail {
        AdminRuntimeReloadDetail::Artifacts(detail) => format!(
            "reload runtime {}: {}",
            result.mode.as_str(),
            render_csv_or_dash(&detail.reloaded_plugin_ids),
        ),
        AdminRuntimeReloadDetail::Topology(detail) => format!(
            "reload runtime {}: activated_generation={} reconfigured={}",
            result.mode.as_str(),
            detail.activated_generation_id,
            render_csv_or_dash(&detail.reconfigured_adapter_ids),
        ),
        AdminRuntimeReloadDetail::Core(_) => {
            format!("reload runtime {}: completed", result.mode.as_str())
        }
        AdminRuntimeReloadDetail::Full(result_view) => format!(
            "reload runtime {}: plugins={} activated_generation={} reconfigured={}",
            result.mode.as_str(),
            render_csv_or_dash(&result_view.reloaded_plugin_ids),
            result_view.topology.activated_generation_id,
            render_csv_or_dash(&result_view.topology.reconfigured_adapter_ids),
        ),
    }
}

const fn plugin_reload_mode_from_runtime(
    mode: RuntimeReloadMode,
) -> plugin_admin::RuntimeReloadMode {
    match mode {
        RuntimeReloadMode::Artifacts => plugin_admin::RuntimeReloadMode::Artifacts,
        RuntimeReloadMode::Topology => plugin_admin::RuntimeReloadMode::Topology,
        RuntimeReloadMode::Core => plugin_admin::RuntimeReloadMode::Core,
        RuntimeReloadMode::Full => plugin_admin::RuntimeReloadMode::Full,
    }
}

const fn runtime_reload_mode_from_plugin(
    mode: plugin_admin::RuntimeReloadMode,
) -> RuntimeReloadMode {
    match mode {
        plugin_admin::RuntimeReloadMode::Artifacts => RuntimeReloadMode::Artifacts,
        plugin_admin::RuntimeReloadMode::Topology => RuntimeReloadMode::Topology,
        plugin_admin::RuntimeReloadMode::Core => RuntimeReloadMode::Core,
        plugin_admin::RuntimeReloadMode::Full => RuntimeReloadMode::Full,
    }
}

fn plugin_topology_reload_view_from_runtime(
    result: &AdminTopologyReloadView,
) -> plugin_admin::AdminTopologyReloadView {
    plugin_admin::AdminTopologyReloadView {
        activated_generation_id: result.activated_generation_id,
        retired_generation_ids: result.retired_generation_ids.clone(),
        applied_config_change: result.applied_config_change,
        reconfigured_adapter_ids: result.reconfigured_adapter_ids.clone(),
    }
}

fn plugin_runtime_reload_view_from_runtime(
    result: &AdminRuntimeReloadView,
) -> plugin_admin::AdminRuntimeReloadView {
    let detail = match &result.detail {
        AdminRuntimeReloadDetail::Artifacts(result) => {
            plugin_admin::AdminRuntimeReloadDetail::Artifacts(
                plugin_admin::AdminArtifactsReloadView {
                    reloaded_plugin_ids: result.reloaded_plugin_ids.clone(),
                },
            )
        }
        AdminRuntimeReloadDetail::Topology(result) => {
            plugin_admin::AdminRuntimeReloadDetail::Topology(
                plugin_topology_reload_view_from_runtime(result),
            )
        }
        AdminRuntimeReloadDetail::Core(_result) => {
            plugin_admin::AdminRuntimeReloadDetail::Core(plugin_admin::AdminCoreReloadView {})
        }
        AdminRuntimeReloadDetail::Full(result) => {
            plugin_admin::AdminRuntimeReloadDetail::Full(plugin_admin::AdminFullReloadView {
                reloaded_plugin_ids: result.reloaded_plugin_ids.clone(),
                topology: plugin_topology_reload_view_from_runtime(&result.topology),
            })
        }
    };
    plugin_admin::AdminRuntimeReloadView {
        mode: plugin_reload_mode_from_runtime(result.mode),
        detail,
    }
}
