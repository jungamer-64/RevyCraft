use super::{
    AdminArtifactsReloadView, AdminCoreReloadView, AdminFullReloadView, AdminGenerationCountView,
    AdminListenerBindingView, AdminNamedCountView, AdminPermission, AdminPhaseCountView,
    AdminPluginHostView, AdminRequest, AdminRuntimeReloadDetail, AdminRuntimeReloadView,
    AdminSessionSummaryView, AdminSessionTransportCountView, AdminSessionView, AdminSessionsView,
    AdminStatusView, AdminTopologyReloadView, AdminUpgradeRuntimeView, RuntimeReloadMode,
    RuntimeReloadResult, RuntimeServer,
};
use crate::RuntimeError;
use mc_plugin_api::codec::admin as surface_admin;
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use thiserror::Error;

pub type RuntimeUpgradeFuture =
    Pin<Box<dyn Future<Output = Result<AdminUpgradeRuntimeView, RuntimeError>> + Send>>;
pub type RuntimeUpgradeCallback =
    Arc<dyn Fn(AdminSubject, String) -> RuntimeUpgradeFuture + Send + Sync>;

#[derive(Clone, PartialEq, Eq)]
pub struct AdminSubject {
    principal_id: String,
}

impl AdminSubject {
    #[must_use]
    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    #[must_use]
    pub(crate) fn remote(principal_id: impl Into<String>) -> Self {
        Self {
            principal_id: principal_id.into(),
        }
    }
}

impl Debug for AdminSubject {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminSubject")
            .field("principal_id", &self.principal_id)
            .finish()
    }
}

impl Display for AdminSubject {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.principal_id())
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AdminAuthError {
    #[error("missing remote admin principal id")]
    MissingPrincipalId,
    #[error("invalid remote admin principal id")]
    InvalidPrincipalId,
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
}

#[derive(Clone)]
struct AdminService {
    runtime: Arc<RuntimeServer>,
    upgrade_callback: Option<RuntimeUpgradeCallback>,
}

impl AdminControlPlaneHandle {
    pub(crate) fn new(runtime: Arc<RuntimeServer>) -> Self {
        Self {
            service: AdminService {
                runtime: Arc::clone(&runtime),
                upgrade_callback: None,
            },
        }
    }

    #[must_use]
    pub fn with_runtime_upgrader(mut self, upgrade_callback: RuntimeUpgradeCallback) -> Self {
        self.service.upgrade_callback = Some(upgrade_callback);
        self
    }

    pub async fn subject_for_remote_principal(
        &self,
        principal_id: &str,
    ) -> Result<AdminSubject, AdminAuthError> {
        self.service
            .subject_for_remote_principal(principal_id)
            .await
    }

    pub async fn permissions_for_principal(
        &self,
        principal_id: &str,
    ) -> Result<Vec<AdminPermission>, AdminAuthError> {
        self.service.permissions_for_principal(principal_id).await
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

    pub async fn execute_surface(
        &self,
        principal_id: &str,
        request: surface_admin::AdminRequest,
    ) -> surface_admin::AdminResponse {
        let subject = match self.subject_for_remote_principal(principal_id).await {
            Ok(subject) => subject,
            Err(error) => {
                return surface_admin::AdminResponse::Error {
                    message: error.to_string(),
                };
            }
        };
        let response = match runtime_request_from_surface_request(request) {
            AdminRequest::Help => return surface_admin::AdminResponse::Help,
            AdminRequest::Status => self
                .status(&subject)
                .await
                .map(|status| {
                    surface_admin::AdminResponse::Status(surface_status_view_from_runtime(&status))
                })
                .unwrap_or_else(|error| surface_response_from_error(principal_id, error)),
            AdminRequest::Sessions => self
                .sessions(&subject)
                .await
                .map(|sessions| {
                    surface_admin::AdminResponse::Sessions(surface_sessions_view_from_runtime(
                        &sessions,
                    ))
                })
                .unwrap_or_else(|error| surface_response_from_error(principal_id, error)),
            AdminRequest::ReloadRuntime { mode } => self
                .reload_runtime(&subject, mode)
                .await
                .map(|reload| {
                    surface_admin::AdminResponse::ReloadRuntime(
                        surface_runtime_reload_view_from_runtime(&reload),
                    )
                })
                .unwrap_or_else(|error| surface_response_from_error(principal_id, error)),
            AdminRequest::UpgradeRuntime { executable_path } => self
                .upgrade_runtime(&subject, executable_path)
                .await
                .map(|result| {
                    surface_admin::AdminResponse::UpgradeRuntime(
                        surface_admin::AdminUpgradeRuntimeView {
                            executable_path: result.executable_path,
                        },
                    )
                })
                .unwrap_or_else(|error| surface_response_from_error(principal_id, error)),
            AdminRequest::Shutdown => self
                .shutdown(&subject)
                .await
                .map(|()| surface_admin::AdminResponse::ShutdownScheduled)
                .unwrap_or_else(|error| surface_response_from_error(principal_id, error)),
        };
        response
    }

    pub async fn surface_permissions_for_principal(
        &self,
        principal_id: &str,
    ) -> Result<Vec<surface_admin::AdminPermission>, AdminAuthError> {
        self.permissions_for_principal(principal_id)
            .await
            .map(|permissions| {
                permissions
                    .into_iter()
                    .map(surface_permission_from_runtime)
                    .collect()
            })
    }
}

fn surface_response_from_error(
    principal_id: &str,
    error: AdminCommandError,
) -> surface_admin::AdminResponse {
    match error {
        AdminCommandError::PermissionDenied { permission, .. } => {
            surface_admin::AdminResponse::PermissionDenied {
                principal_id: principal_id.to_string(),
                permission: surface_permission_from_runtime(permission),
            }
        }
        AdminCommandError::InvalidSubject { subject } => surface_admin::AdminResponse::Error {
            message: format!("invalid admin subject: subject={subject}"),
        },
        AdminCommandError::Runtime(error) => surface_admin::AdminResponse::Error {
            message: error.to_string(),
        },
    }
}

impl AdminService {
    async fn subject_for_remote_principal(
        &self,
        principal_id: &str,
    ) -> Result<AdminSubject, AdminAuthError> {
        self.runtime
            .subject_for_remote_principal(principal_id)
            .await
    }

    async fn permissions_for_principal(
        &self,
        principal_id: &str,
    ) -> Result<Vec<AdminPermission>, AdminAuthError> {
        self.runtime.permissions_for_principal(principal_id).await
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
            .reject_mutating_admin_action_during_upgrade("reload runtime")?;
        self.runtime
            .admin_reload_runtime_view(mode)
            .await
            .map_err(Into::into)
    }

    async fn shutdown(&self, subject: &AdminSubject) -> Result<(), AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::Shutdown)
            .await?;
        self.runtime
            .reject_mutating_admin_action_during_upgrade("shutdown")?;
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
        self.runtime
            .reject_mutating_admin_action_during_upgrade("upgrade runtime")?;
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
}

impl RuntimeServer {
    pub(crate) fn request_shutdown(&self) -> bool {
        self.reload.request_shutdown()
    }

    pub(crate) fn clear_runtime_upgrade_state(&self) {
        self.reload.clear_upgrade_state();
    }

    pub(crate) fn reject_mutating_admin_action_during_upgrade(
        &self,
        action: &str,
    ) -> Result<(), RuntimeError> {
        self.reload
            .reject_mutating_admin_action_during_upgrade(action)
    }

    pub(crate) async fn subject_for_remote_principal(
        &self,
        principal_id: &str,
    ) -> Result<AdminSubject, AdminAuthError> {
        let principal_id = principal_id.trim();
        if principal_id.is_empty() {
            return Err(AdminAuthError::MissingPrincipalId);
        }
        self.selection_state()
            .await
            .remote_admin_principals
            .get(principal_id)
            .map(|principal| AdminSubject::remote(principal.principal_id.clone()))
            .ok_or(AdminAuthError::InvalidPrincipalId)
    }

    pub(crate) async fn permissions_for_principal(
        &self,
        principal_id: &str,
    ) -> Result<Vec<AdminPermission>, AdminAuthError> {
        let principal_id = principal_id.trim();
        if principal_id.is_empty() {
            return Err(AdminAuthError::MissingPrincipalId);
        }
        self.selection_state()
            .await
            .remote_admin_principals
            .get(principal_id)
            .map(|principal| principal.permissions.clone())
            .ok_or(AdminAuthError::InvalidPrincipalId)
    }

    async fn authorize(
        &self,
        subject: &AdminSubject,
        permission: AdminPermission,
    ) -> Result<(), AdminCommandError> {
        let selection_state = self.selection_state().await;
        let Some(principal) = selection_state
            .remote_admin_principals
            .get(subject.principal_id())
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
                    .map(|count| AdminSessionTransportCountView {
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
                admin_surface_count: status.admin_surface_count,
                active_quarantine_count: status.active_quarantine_count,
                artifact_quarantine_count: status.artifact_quarantine_count,
                pending_fatal_error: status.pending_fatal_error,
            }),
            upgrade: snapshot.upgrade,
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
                    .map(|count| AdminSessionTransportCountView {
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

fn runtime_request_from_surface_request(request: surface_admin::AdminRequest) -> AdminRequest {
    match request {
        surface_admin::AdminRequest::Help => AdminRequest::Help,
        surface_admin::AdminRequest::Status => AdminRequest::Status,
        surface_admin::AdminRequest::Sessions => AdminRequest::Sessions,
        surface_admin::AdminRequest::ReloadRuntime { mode } => AdminRequest::ReloadRuntime {
            mode: runtime_reload_mode_from_surface(mode),
        },
        surface_admin::AdminRequest::UpgradeRuntime { executable_path } => {
            AdminRequest::UpgradeRuntime { executable_path }
        }
        surface_admin::AdminRequest::Shutdown => AdminRequest::Shutdown,
    }
}

fn surface_permission_from_runtime(permission: AdminPermission) -> surface_admin::AdminPermission {
    match permission {
        AdminPermission::Status => surface_admin::AdminPermission::Status,
        AdminPermission::Sessions => surface_admin::AdminPermission::Sessions,
        AdminPermission::ReloadRuntime => surface_admin::AdminPermission::ReloadRuntime,
        AdminPermission::UpgradeRuntime => surface_admin::AdminPermission::UpgradeRuntime,
        AdminPermission::Shutdown => surface_admin::AdminPermission::Shutdown,
    }
}

fn surface_status_view_from_runtime(status: &AdminStatusView) -> surface_admin::AdminStatusView {
    surface_admin::AdminStatusView {
        active_generation_id: status.active_generation_id,
        draining_generation_ids: status.draining_generation_ids.clone(),
        listener_bindings: status
            .listener_bindings
            .iter()
            .map(|binding| surface_admin::AdminListenerBindingView {
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
        session_summary: surface_summary_view_from_runtime(&status.session_summary),
        dirty: status.dirty,
        plugin_host: status.plugin_host.as_ref().map(|plugin_host| {
            surface_admin::AdminPluginHostView {
                protocol_count: plugin_host.protocol_count,
                gameplay_count: plugin_host.gameplay_count,
                storage_count: plugin_host.storage_count,
                auth_count: plugin_host.auth_count,
                admin_surface_count: plugin_host.admin_surface_count,
                active_quarantine_count: plugin_host.active_quarantine_count,
                artifact_quarantine_count: plugin_host.artifact_quarantine_count,
                pending_fatal_error: plugin_host.pending_fatal_error.clone(),
            }
        }),
        upgrade: status
            .upgrade
            .map(|upgrade| surface_admin::RuntimeUpgradeStateView {
                role: match upgrade.role {
                    super::RuntimeUpgradeRole::Parent => surface_admin::RuntimeUpgradeRole::Parent,
                    super::RuntimeUpgradeRole::Child => surface_admin::RuntimeUpgradeRole::Child,
                },
                phase: match upgrade.phase {
                    super::RuntimeUpgradePhase::ParentFreezing => {
                        surface_admin::RuntimeUpgradePhase::ParentFreezing
                    }
                    super::RuntimeUpgradePhase::ParentWaitingChildReady => {
                        surface_admin::RuntimeUpgradePhase::ParentWaitingChildReady
                    }
                    super::RuntimeUpgradePhase::ParentRollingBack => {
                        surface_admin::RuntimeUpgradePhase::ParentRollingBack
                    }
                    super::RuntimeUpgradePhase::ChildWaitingCommit => {
                        surface_admin::RuntimeUpgradePhase::ChildWaitingCommit
                    }
                },
            }),
    }
}

fn surface_summary_view_from_runtime(
    summary: &AdminSessionSummaryView,
) -> surface_admin::AdminSessionSummaryView {
    surface_admin::AdminSessionSummaryView {
        total: summary.total,
        by_transport: summary
            .by_transport
            .iter()
            .map(|entry| surface_admin::AdminSessionTransportCountView {
                transport: entry.transport,
                count: entry.count,
            })
            .collect(),
        by_phase: summary
            .by_phase
            .iter()
            .map(|entry| surface_admin::AdminPhaseCountView {
                phase: entry.phase,
                count: entry.count,
            })
            .collect(),
        by_generation: summary
            .by_generation
            .iter()
            .map(|entry| surface_admin::AdminGenerationCountView {
                generation_id: entry.generation_id,
                count: entry.count,
            })
            .collect(),
        by_adapter_id: summary
            .by_adapter_id
            .iter()
            .map(|entry| surface_admin::AdminNamedCountView {
                value: entry.value.clone(),
                count: entry.count,
            })
            .collect(),
        by_gameplay_profile: summary
            .by_gameplay_profile
            .iter()
            .map(|entry| surface_admin::AdminNamedCountView {
                value: entry.value.clone(),
                count: entry.count,
            })
            .collect(),
    }
}

fn surface_sessions_view_from_runtime(
    sessions: &AdminSessionsView,
) -> surface_admin::AdminSessionsView {
    surface_admin::AdminSessionsView {
        summary: surface_summary_view_from_runtime(&sessions.summary),
        sessions: sessions
            .sessions
            .iter()
            .map(|session| surface_admin::AdminSessionView {
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

const fn surface_reload_mode_from_runtime(
    mode: RuntimeReloadMode,
) -> surface_admin::RuntimeReloadMode {
    match mode {
        RuntimeReloadMode::Artifacts => surface_admin::RuntimeReloadMode::Artifacts,
        RuntimeReloadMode::Topology => surface_admin::RuntimeReloadMode::Topology,
        RuntimeReloadMode::Core => surface_admin::RuntimeReloadMode::Core,
        RuntimeReloadMode::Full => surface_admin::RuntimeReloadMode::Full,
    }
}

const fn runtime_reload_mode_from_surface(
    mode: surface_admin::RuntimeReloadMode,
) -> RuntimeReloadMode {
    match mode {
        surface_admin::RuntimeReloadMode::Artifacts => RuntimeReloadMode::Artifacts,
        surface_admin::RuntimeReloadMode::Topology => RuntimeReloadMode::Topology,
        surface_admin::RuntimeReloadMode::Core => RuntimeReloadMode::Core,
        surface_admin::RuntimeReloadMode::Full => RuntimeReloadMode::Full,
    }
}

fn surface_runtime_reload_view_from_runtime(
    result: &AdminRuntimeReloadView,
) -> surface_admin::AdminRuntimeReloadView {
    surface_admin::AdminRuntimeReloadView {
        mode: surface_reload_mode_from_runtime(result.mode),
        detail: match &result.detail {
            AdminRuntimeReloadDetail::Artifacts(detail) => {
                surface_admin::AdminRuntimeReloadDetail::Artifacts(
                    surface_admin::AdminArtifactsReloadView {
                        reloaded_plugin_ids: detail.reloaded_plugin_ids.clone(),
                    },
                )
            }
            AdminRuntimeReloadDetail::Topology(detail) => {
                surface_admin::AdminRuntimeReloadDetail::Topology(
                    surface_admin::AdminTopologyReloadView {
                        activated_generation_id: detail.activated_generation_id,
                        retired_generation_ids: detail.retired_generation_ids.clone(),
                        applied_config_change: detail.applied_config_change,
                        reconfigured_adapter_ids: detail.reconfigured_adapter_ids.clone(),
                    },
                )
            }
            AdminRuntimeReloadDetail::Core(_) => {
                surface_admin::AdminRuntimeReloadDetail::Core(surface_admin::AdminCoreReloadView {})
            }
            AdminRuntimeReloadDetail::Full(detail) => {
                surface_admin::AdminRuntimeReloadDetail::Full(surface_admin::AdminFullReloadView {
                    reloaded_plugin_ids: detail.reloaded_plugin_ids.clone(),
                    topology: surface_admin::AdminTopologyReloadView {
                        activated_generation_id: detail.topology.activated_generation_id,
                        retired_generation_ids: detail.topology.retired_generation_ids.clone(),
                        applied_config_change: detail.topology.applied_config_change,
                        reconfigured_adapter_ids: detail.topology.reconfigured_adapter_ids.clone(),
                    },
                })
            }
        },
    }
}
