use mc_core::{ConnectionId, EntityId, PlayerId, PluginGenerationId};
use mc_proto_common::{ConnectionPhase, TransportKind};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenerBinding {
    pub transport: TransportKind,
    pub local_addr: SocketAddr,
    pub adapter_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginFailureAction {
    Quarantine,
    Skip,
    FailFast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginFailureMatrix {
    pub protocol: PluginFailureAction,
    pub gameplay: PluginFailureAction,
    pub storage: PluginFailureAction,
    pub auth: PluginFailureAction,
    pub admin_ui: PluginFailureAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHostStatusSnapshot {
    pub failure_matrix: PluginFailureMatrix,
    pub pending_fatal_error: Option<String>,
    pub protocol_count: usize,
    pub gameplay_count: usize,
    pub storage_count: usize,
    pub auth_count: usize,
    pub admin_ui_count: usize,
    pub active_quarantine_count: usize,
    pub artifact_quarantine_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AdminPrincipal {
    LocalConsole,
}

impl AdminPrincipal {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalConsole => "local-console",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AdminPermission {
    Status,
    Sessions,
    ReloadConfig,
    ReloadPlugins,
    ReloadGeneration,
    Shutdown,
}

impl AdminPermission {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Sessions => "sessions",
            Self::ReloadConfig => "reload-config",
            Self::ReloadPlugins => "reload-plugins",
            Self::ReloadGeneration => "reload-generation",
            Self::Shutdown => "shutdown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminRequest {
    Help,
    Status,
    Sessions,
    ReloadConfig,
    ReloadPlugins,
    ReloadGeneration,
    Shutdown,
}

impl AdminRequest {
    #[must_use]
    pub const fn required_permission(&self) -> Option<AdminPermission> {
        match self {
            Self::Help => None,
            Self::Status => Some(AdminPermission::Status),
            Self::Sessions => Some(AdminPermission::Sessions),
            Self::ReloadConfig => Some(AdminPermission::ReloadConfig),
            Self::ReloadPlugins => Some(AdminPermission::ReloadPlugins),
            Self::ReloadGeneration => Some(AdminPermission::ReloadGeneration),
            Self::Shutdown => Some(AdminPermission::Shutdown),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminListenerBindingView {
    pub transport: TransportKind,
    pub local_addr: String,
    pub adapter_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminTransportCountView {
    pub transport: TransportKind,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminPhaseCountView {
    pub phase: ConnectionPhase,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGenerationCountView {
    pub generation_id: u64,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminNamedCountView {
    pub value: Option<String>,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSessionSummaryView {
    pub total: usize,
    pub by_transport: Vec<AdminTransportCountView>,
    pub by_phase: Vec<AdminPhaseCountView>,
    pub by_generation: Vec<AdminGenerationCountView>,
    pub by_adapter_id: Vec<AdminNamedCountView>,
    pub by_gameplay_profile: Vec<AdminNamedCountView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminPluginHostView {
    pub protocol_count: usize,
    pub gameplay_count: usize,
    pub storage_count: usize,
    pub auth_count: usize,
    pub admin_ui_count: usize,
    pub active_quarantine_count: usize,
    pub artifact_quarantine_count: usize,
    pub pending_fatal_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminStatusView {
    pub active_generation_id: u64,
    pub draining_generation_ids: Vec<u64>,
    pub listener_bindings: Vec<AdminListenerBindingView>,
    pub default_adapter_id: String,
    pub default_bedrock_adapter_id: Option<String>,
    pub enabled_adapter_ids: Vec<String>,
    pub enabled_bedrock_adapter_ids: Vec<String>,
    pub motd: String,
    pub max_players: u8,
    pub session_summary: AdminSessionSummaryView,
    pub dirty: bool,
    pub plugin_host: Option<AdminPluginHostView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSessionView {
    pub connection_id: ConnectionId,
    pub generation_id: u64,
    pub transport: TransportKind,
    pub phase: ConnectionPhase,
    pub adapter_id: Option<String>,
    pub gameplay_profile: Option<String>,
    pub player_id: Option<PlayerId>,
    pub entity_id: Option<EntityId>,
    pub protocol_generation: Option<PluginGenerationId>,
    pub gameplay_generation: Option<PluginGenerationId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSessionsView {
    pub summary: AdminSessionSummaryView,
    pub sessions: Vec<AdminSessionView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminPluginsReloadView {
    pub reloaded_plugin_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGenerationReloadView {
    pub activated_generation_id: u64,
    pub retired_generation_ids: Vec<u64>,
    pub applied_config_change: bool,
    pub reconfigured_adapter_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminConfigReloadView {
    pub reloaded_plugin_ids: Vec<String>,
    pub generation: AdminGenerationReloadView,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminResponse {
    Help,
    Status(AdminStatusView),
    Sessions(AdminSessionsView),
    ReloadConfig(AdminConfigReloadView),
    ReloadPlugins(AdminPluginsReloadView),
    ReloadGeneration(AdminGenerationReloadView),
    ShutdownScheduled,
    PermissionDenied {
        principal: AdminPrincipal,
        permission: AdminPermission,
    },
    Error {
        message: String,
    },
}
