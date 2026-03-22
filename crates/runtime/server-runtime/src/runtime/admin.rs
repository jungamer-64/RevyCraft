use super::{
    AdminConfigReloadView, AdminListenerBindingView, AdminNamedCountView, AdminPermission,
    AdminPhaseCountView, AdminPluginHostView, AdminPluginsReloadView, AdminPrincipal, AdminRequest,
    AdminResponse, AdminSessionSummaryView, AdminSessionView, AdminSessionsView, AdminStatusView,
    AdminTopologyGenerationCountView, AdminTopologyReloadView, AdminTransportCountView,
    RuntimeServer,
};
use mc_plugin_host::runtime::AdminUiProfileHandle;
use std::sync::Arc;

#[derive(Clone)]
pub struct AdminControlPlaneHandle {
    runtime: Arc<RuntimeServer>,
}

impl AdminControlPlaneHandle {
    pub(crate) const fn new(runtime: Arc<RuntimeServer>) -> Self {
        Self { runtime }
    }

    pub async fn execute(&self, principal: AdminPrincipal, request: AdminRequest) -> AdminResponse {
        if let Some(permission) = request.required_permission()
            && !self
                .runtime
                .permissions_for(principal)
                .await
                .contains(&permission)
        {
            return AdminResponse::PermissionDenied {
                principal,
                permission,
            };
        }

        match request {
            AdminRequest::Help => AdminResponse::Help,
            AdminRequest::Status => AdminResponse::Status(self.runtime.admin_status_view().await),
            AdminRequest::Sessions => {
                AdminResponse::Sessions(self.runtime.admin_sessions_view().await)
            }
            AdminRequest::ReloadConfig => self.runtime.admin_reload_config().await,
            AdminRequest::ReloadPlugins => self.runtime.admin_reload_plugins().await,
            AdminRequest::ReloadTopology => self.runtime.admin_reload_topology().await,
            AdminRequest::Shutdown => {
                let _ = self.runtime.request_shutdown();
                AdminResponse::ShutdownScheduled
            }
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

    async fn permissions_for(&self, principal: AdminPrincipal) -> Vec<AdminPermission> {
        match principal {
            AdminPrincipal::LocalConsole => self
                .live_state
                .read()
                .await
                .config
                .admin
                .local_console_permissions
                .clone(),
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

    async fn admin_reload_plugins(&self) -> AdminResponse {
        let Some(reload_host) = self.reload_host.as_ref() else {
            return AdminResponse::Error {
                message: "plugin reload is unavailable without a reload-capable host".to_string(),
            };
        };
        match self.reload_plugins(reload_host.as_ref()).await {
            Ok(reloaded_plugin_ids) => AdminResponse::ReloadPlugins(AdminPluginsReloadView {
                reloaded_plugin_ids,
            }),
            Err(error) => AdminResponse::Error {
                message: error.to_string(),
            },
        }
    }

    async fn admin_reload_topology(&self) -> AdminResponse {
        let Some(reload_host) = self.reload_host.as_ref() else {
            return AdminResponse::Error {
                message: "topology reload is unavailable without a reload-capable host".to_string(),
            };
        };
        match self.reload_topology(reload_host.as_ref()).await {
            Ok(result) => AdminResponse::ReloadTopology(AdminTopologyReloadView {
                activated_generation_id: result.activated_generation_id.0,
                retired_generation_ids: result
                    .retired_generation_ids
                    .into_iter()
                    .map(|generation_id| generation_id.0)
                    .collect(),
                applied_config_change: result.applied_config_change,
                reconfigured_adapter_ids: result.reconfigured_adapter_ids,
            }),
            Err(error) => AdminResponse::Error {
                message: error.to_string(),
            },
        }
    }

    async fn admin_reload_config(&self) -> AdminResponse {
        let Some(reload_host) = self.reload_host.as_ref() else {
            return AdminResponse::Error {
                message: "config reload is unavailable without a reload-capable host".to_string(),
            };
        };
        match self.reload_config(reload_host.as_ref()).await {
            Ok(result) => AdminResponse::ReloadConfig(AdminConfigReloadView {
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
            }),
            Err(error) => AdminResponse::Error {
                message: error.to_string(),
            },
        }
    }
}
