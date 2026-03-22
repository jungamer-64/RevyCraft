use super::{
    AdminConfigReloadView, AdminListenerBindingView, AdminNamedCountView, AdminPermission,
    AdminPhaseCountView, AdminPluginHostView, AdminPluginsReloadView, AdminPrincipal, AdminRequest,
    AdminResponse, AdminSessionSummaryView, AdminSessionView, AdminSessionsView, AdminStatusView,
    AdminTopologyGenerationCountView, AdminTopologyReloadView, AdminTransportCountView,
    RuntimeServer,
};
use crate::RuntimeError;
use mc_plugin_host::runtime::AdminUiProfileHandle;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;
use thiserror::Error;

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

#[derive(Clone, Copy, PartialEq, Eq)]
struct AdminCredentialTag([u8; 32]);

impl AdminCredentialTag {
    #[must_use]
    fn from_token(token: &str) -> Self {
        let digest = Sha256::digest(token.as_bytes());
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&digest);
        Self(bytes)
    }
}

impl Debug for AdminCredentialTag {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("***redacted***")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteAdminPrincipal {
    principal_id: String,
    credential_tag: AdminCredentialTag,
    permissions: Vec<AdminPermission>,
}

impl RemoteAdminPrincipal {
    #[must_use]
    fn new(
        principal_id: impl Into<String>,
        token: &str,
        permissions: Vec<AdminPermission>,
    ) -> Self {
        Self {
            principal_id: principal_id.into(),
            credential_tag: AdminCredentialTag::from_token(token),
            permissions,
        }
    }
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
    runtime: Arc<RuntimeServer>,
}

impl AdminControlPlaneHandle {
    pub(crate) const fn new(runtime: Arc<RuntimeServer>) -> Self {
        Self { runtime }
    }

    pub async fn authenticate_remote_token(
        &self,
        token: &str,
    ) -> Result<AdminSubject, AdminAuthError> {
        self.runtime.authenticate_remote_token(token).await
    }

    pub async fn status(
        &self,
        subject: &AdminSubject,
    ) -> Result<AdminStatusView, AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::Status)
            .await?;
        Ok(self.runtime.admin_status_view().await)
    }

    pub async fn sessions(
        &self,
        subject: &AdminSubject,
    ) -> Result<AdminSessionsView, AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::Sessions)
            .await?;
        Ok(self.runtime.admin_sessions_view().await)
    }

    pub async fn reload_config(
        &self,
        subject: &AdminSubject,
    ) -> Result<AdminConfigReloadView, AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::ReloadConfig)
            .await?;
        self.runtime
            .admin_reload_config_view()
            .await
            .map_err(Into::into)
    }

    pub async fn reload_plugins(
        &self,
        subject: &AdminSubject,
    ) -> Result<AdminPluginsReloadView, AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::ReloadPlugins)
            .await?;
        self.runtime
            .admin_reload_plugins_view()
            .await
            .map_err(Into::into)
    }

    pub async fn reload_topology(
        &self,
        subject: &AdminSubject,
    ) -> Result<AdminTopologyReloadView, AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::ReloadTopology)
            .await?;
        self.runtime
            .admin_reload_topology_view()
            .await
            .map_err(Into::into)
    }

    pub async fn shutdown(&self, subject: &AdminSubject) -> Result<(), AdminCommandError> {
        self.runtime
            .authorize(subject, AdminPermission::Shutdown)
            .await?;
        let _ = self.runtime.request_shutdown();
        Ok(())
    }

    pub async fn execute_local_console(&self, request: AdminRequest) -> AdminResponse {
        let subject = AdminSubject::local_console();
        match request {
            AdminRequest::Help => AdminResponse::Help,
            AdminRequest::Status => self
                .status(&subject)
                .await
                .map(AdminResponse::Status)
                .unwrap_or_else(Self::local_response_from_error),
            AdminRequest::Sessions => self
                .sessions(&subject)
                .await
                .map(AdminResponse::Sessions)
                .unwrap_or_else(Self::local_response_from_error),
            AdminRequest::ReloadConfig => self
                .reload_config(&subject)
                .await
                .map(AdminResponse::ReloadConfig)
                .unwrap_or_else(Self::local_response_from_error),
            AdminRequest::ReloadPlugins => self
                .reload_plugins(&subject)
                .await
                .map(AdminResponse::ReloadPlugins)
                .unwrap_or_else(Self::local_response_from_error),
            AdminRequest::ReloadTopology => self
                .reload_topology(&subject)
                .await
                .map(AdminResponse::ReloadTopology)
                .unwrap_or_else(Self::local_response_from_error),
            AdminRequest::Shutdown => self
                .shutdown(&subject)
                .await
                .map(|()| AdminResponse::ShutdownScheduled)
                .unwrap_or_else(Self::local_response_from_error),
        }
    }

    pub async fn execute(&self, principal: AdminPrincipal, request: AdminRequest) -> AdminResponse {
        match principal {
            AdminPrincipal::LocalConsole => self.execute_local_console(request).await,
        }
    }

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

impl RuntimeServer {
    pub(crate) async fn current_admin_ui(&self) -> Option<Arc<dyn AdminUiProfileHandle>> {
        self.live_state.read().await.admin_ui.clone()
    }

    pub(crate) fn request_shutdown(&self) -> bool {
        self.shutdown_tx
            .lock()
            .expect("shutdown mutex should not be poisoned")
            .take()
            .is_some_and(|shutdown_tx| shutdown_tx.send(()).is_ok())
    }

    pub(crate) async fn authenticate_remote_token(
        &self,
        token: &str,
    ) -> Result<AdminSubject, AdminAuthError> {
        let token = token.trim();
        if token.is_empty() {
            return Err(AdminAuthError::InvalidToken);
        }
        self.live_state
            .read()
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
        let live_state = self.live_state.read().await;
        match &subject.kind {
            AdminSubjectKind::LocalConsole => {
                if live_state
                    .config
                    .admin
                    .local_console_permissions
                    .contains(&permission)
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
                let Some(principal) = live_state.remote_admin_subjects.values().find(|principal| {
                    principal.principal_id == remote.principal_id
                        && principal.credential_tag == remote.credential_tag
                }) else {
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
            active_topology_generation_id: snapshot.active_topology.generation_id.0,
            draining_topology_generation_ids: snapshot
                .draining_topologies
                .iter()
                .map(|topology| topology.generation_id.0)
                .collect(),
            listener_bindings: snapshot
                .listener_bindings
                .into_iter()
                .map(|binding| AdminListenerBindingView {
                    transport: binding.transport,
                    local_addr: binding.local_addr.to_string(),
                    adapter_ids: binding.adapter_ids,
                })
                .collect(),
            default_adapter_id: snapshot.active_topology.default_adapter_id,
            default_bedrock_adapter_id: snapshot.active_topology.default_bedrock_adapter_id,
            enabled_adapter_ids: snapshot.active_topology.enabled_adapter_ids,
            enabled_bedrock_adapter_ids: snapshot.active_topology.enabled_bedrock_adapter_ids,
            motd: snapshot.active_topology.motd,
            max_players: snapshot.active_topology.max_players,
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
                by_topology_generation: snapshot
                    .session_summary
                    .by_topology_generation
                    .into_iter()
                    .map(|count| AdminTopologyGenerationCountView {
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
                protocol_count: status.protocols.len(),
                gameplay_count: status.gameplay.len(),
                storage_count: status.storage.len(),
                auth_count: status.auth.len(),
                admin_ui_count: status.admin_ui.len(),
                active_quarantine_count: status.active_quarantine_count(),
                artifact_quarantine_count: status.artifact_quarantine_count(),
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
                by_topology_generation: summary
                    .by_topology_generation
                    .into_iter()
                    .map(|count| AdminTopologyGenerationCountView {
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
                    topology_generation_id: session.topology_generation_id.0,
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

    async fn admin_reload_plugins_view(&self) -> Result<AdminPluginsReloadView, RuntimeError> {
        let Some(reload_host) = self.reload_host.as_ref() else {
            return Err(RuntimeError::Config(
                "plugin reload is unavailable without a reload-capable host".to_string(),
            ));
        };
        self.reload_plugins(reload_host.as_ref())
            .await
            .map(|reloaded_plugin_ids| AdminPluginsReloadView {
                reloaded_plugin_ids,
            })
    }

    async fn admin_reload_topology_view(&self) -> Result<AdminTopologyReloadView, RuntimeError> {
        let Some(reload_host) = self.reload_host.as_ref() else {
            return Err(RuntimeError::Config(
                "topology reload is unavailable without a reload-capable host".to_string(),
            ));
        };
        self.reload_topology(reload_host.as_ref())
            .await
            .map(|result| AdminTopologyReloadView {
                activated_generation_id: result.activated_generation_id.0,
                retired_generation_ids: result
                    .retired_generation_ids
                    .into_iter()
                    .map(|generation_id| generation_id.0)
                    .collect(),
                applied_config_change: result.applied_config_change,
                reconfigured_adapter_ids: result.reconfigured_adapter_ids,
            })
    }

    async fn admin_reload_config_view(&self) -> Result<AdminConfigReloadView, RuntimeError> {
        let Some(reload_host) = self.reload_host.as_ref() else {
            return Err(RuntimeError::Config(
                "config reload is unavailable without a reload-capable host".to_string(),
            ));
        };
        self.reload_config(reload_host.as_ref())
            .await
            .map(|result| AdminConfigReloadView {
                reloaded_plugin_ids: result.reloaded_plugins,
                topology: AdminTopologyReloadView {
                    activated_generation_id: result.topology.activated_generation_id.0,
                    retired_generation_ids: result
                        .topology
                        .retired_generation_ids
                        .into_iter()
                        .map(|generation_id| generation_id.0)
                        .collect(),
                    applied_config_change: result.topology.applied_config_change,
                    reconfigured_adapter_ids: result.topology.reconfigured_adapter_ids,
                },
            })
    }
}

pub(crate) fn remote_admin_subjects_from_config(
    config: &crate::config::ServerConfig,
) -> HashMap<String, RemoteAdminPrincipal> {
    config
        .admin
        .grpc
        .principals
        .iter()
        .map(|(principal_id, principal)| {
            (
                principal.token.trim().to_string(),
                RemoteAdminPrincipal::new(
                    principal_id.clone(),
                    &principal.token,
                    principal.permissions.clone(),
                ),
            )
        })
        .collect()
}
