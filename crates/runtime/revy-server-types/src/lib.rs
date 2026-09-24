use mc_proto_common::{ConnectionPhase, TransportKind};
use revy_voxel_semantic::{
    AdapterId, AdminSurfaceProfileId, AuthProfileId, ConnectionId, EntityId, GameplayProfileId,
    PlayerId, PluginGenerationId, StorageProfileId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenerBinding {
    pub transport: TransportKind,
    pub local_addr: SocketAddr,
    pub adapter_ids: Vec<AdapterId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginFailureAction {
    Quarantine,
    Skip,
    FailFast,
}

impl PluginFailureAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Quarantine => "quarantine",
            Self::Skip => "skip",
            Self::FailFast => "fail-fast",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginFailureMatrix {
    pub protocol: PluginFailureAction,
    pub gameplay: PluginFailureAction,
    pub storage: PluginFailureAction,
    pub auth: PluginFailureAction,
    pub admin_surface: PluginFailureAction,
}

impl Default for PluginFailureMatrix {
    fn default() -> Self {
        Self {
            protocol: PluginFailureAction::Quarantine,
            gameplay: PluginFailureAction::Quarantine,
            storage: PluginFailureAction::FailFast,
            auth: PluginFailureAction::Skip,
            admin_surface: PluginFailureAction::Skip,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHostBufferLimitsView {
    pub protocol_response_bytes: usize,
    pub gameplay_response_bytes: usize,
    pub storage_response_bytes: usize,
    pub auth_response_bytes: usize,
    pub admin_surface_response_bytes: usize,
    pub callback_payload_bytes: usize,
    pub metadata_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAbiVersionView {
    pub major: u16,
    pub minor: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHostBootstrapSelectionView {
    pub storage_profile: StorageProfileId,
    pub plugins_dir: PathBuf,
    pub plugin_abi_min: PluginAbiVersionView,
    pub plugin_abi_max: PluginAbiVersionView,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHostAdminSurfaceSelectionView {
    pub instance_id: String,
    pub profile: AdminSurfaceProfileId,
    pub config_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHostRuntimeSelectionView {
    pub be_enabled: bool,
    pub auth_profile: AuthProfileId,
    pub bedrock_auth_profile: AuthProfileId,
    pub default_gameplay_profile: GameplayProfileId,
    pub gameplay_profile_map: HashMap<AdapterId, GameplayProfileId>,
    pub admin_surfaces: Vec<PluginHostAdminSurfaceSelectionView>,
    pub plugin_allowlist: Option<Vec<String>>,
    pub buffer_limits: PluginHostBufferLimitsView,
    pub failure_matrix: PluginFailureMatrix,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHostStatusSnapshot {
    pub failure_matrix: PluginFailureMatrix,
    pub pending_fatal_error: Option<String>,
    pub protocol_count: usize,
    pub gameplay_count: usize,
    pub storage_count: usize,
    pub auth_count: usize,
    pub admin_surface_count: usize,
    pub active_quarantine_count: usize,
    pub artifact_quarantine_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuntimeReloadMode {
    Artifacts,
    Topology,
    Core,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CutoverOperation {
    Reload,
    ExecutableUpgrade,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CutoverOutcome {
    Committed,
    Aborted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverConnectionMix {
    pub java: usize,
    pub bedrock: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverReport {
    pub operation: CutoverOperation,
    pub mode: Option<RuntimeReloadMode>,
    pub connection_mix: CutoverConnectionMix,
    pub session_count: usize,
    pub stage_us: u64,
    pub prepare_us: u64,
    pub freeze_us: u64,
    pub resume_us: u64,
    pub outcome: CutoverOutcome,
    pub epoch_revision: u64,
}

impl RuntimeReloadMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Artifacts => "artifacts",
            Self::Topology => "topology",
            Self::Core => "core",
            Self::Full => "full",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AdminPermission {
    Status,
    Sessions,
    ReloadRuntime,
    UpgradeRuntime,
    Shutdown,
}

impl AdminPermission {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Sessions => "sessions",
            Self::ReloadRuntime => "reload-runtime",
            Self::UpgradeRuntime => "upgrade-runtime",
            Self::Shutdown => "shutdown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuntimeUpgradeRole {
    Parent,
    Child,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuntimeUpgradePhase {
    ParentStaging,
    ParentPreparing,
    ParentFrozen,
    ParentOutcomeUncertain,
    ParentCommitted,
    ChildBooting,
    ChildPrestaging,
    ChildReady,
    ChildCommitting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeUpgradeStateView {
    pub role: RuntimeUpgradeRole,
    pub phase: RuntimeUpgradePhase,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminRequest {
    Help,
    Status,
    Sessions,
    ReloadRuntime { mode: RuntimeReloadMode },
    UpgradeRuntime { executable_path: String },
    Shutdown,
}

impl AdminRequest {
    #[must_use]
    pub const fn required_permission(&self) -> Option<AdminPermission> {
        match self {
            Self::Help => None,
            Self::Status => Some(AdminPermission::Status),
            Self::Sessions => Some(AdminPermission::Sessions),
            Self::ReloadRuntime { .. } => Some(AdminPermission::ReloadRuntime),
            Self::UpgradeRuntime { .. } => Some(AdminPermission::UpgradeRuntime),
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
pub struct AdminSessionTransportCountView {
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
    pub by_transport: Vec<AdminSessionTransportCountView>,
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
    pub admin_surface_count: usize,
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
    pub max_players: u32,
    pub session_summary: AdminSessionSummaryView,
    pub dirty: bool,
    pub plugin_host: Option<AdminPluginHostView>,
    pub upgrade: Option<RuntimeUpgradeStateView>,
    pub last_cutover: Option<CutoverReport>,
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
pub struct AdminArtifactsReloadView {
    pub reloaded_plugin_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminTopologyReloadView {
    pub activated_generation_id: u64,
    pub retired_generation_ids: Vec<u64>,
    pub applied_config_change: bool,
    pub reconfigured_adapter_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminCoreReloadView {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminFullReloadView {
    pub reloaded_plugin_ids: Vec<String>,
    pub topology: AdminTopologyReloadView,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminRuntimeReloadDetail {
    Artifacts(AdminArtifactsReloadView),
    Topology(AdminTopologyReloadView),
    Core(AdminCoreReloadView),
    Full(AdminFullReloadView),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminRuntimeReloadView {
    pub mode: RuntimeReloadMode,
    pub detail: AdminRuntimeReloadDetail,
    pub cutover: CutoverReport,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminUpgradeRuntimeView {
    pub executable_path: String,
    pub cutover: CutoverReport,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminResponse {
    Help,
    Status(AdminStatusView),
    Sessions(AdminSessionsView),
    ReloadRuntime(AdminRuntimeReloadView),
    UpgradeRuntime(AdminUpgradeRuntimeView),
    ShutdownScheduled,
    PermissionDenied {
        principal_id: String,
        permission: AdminPermission,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{AdminPermission, AdminRequest, PluginFailureAction, PluginFailureMatrix};

    #[test]
    fn admin_request_reports_required_permission() {
        assert_eq!(AdminRequest::Help.required_permission(), None);
        assert_eq!(
            AdminRequest::ReloadRuntime {
                mode: super::RuntimeReloadMode::Full,
            }
            .required_permission(),
            Some(AdminPermission::ReloadRuntime)
        );
    }

    #[test]
    fn plugin_failure_matrix_default_matches_runtime_policy() {
        assert_eq!(
            PluginFailureMatrix::default(),
            PluginFailureMatrix {
                protocol: PluginFailureAction::Quarantine,
                gameplay: PluginFailureAction::Quarantine,
                storage: PluginFailureAction::FailFast,
                auth: PluginFailureAction::Skip,
                admin_surface: PluginFailureAction::Skip,
            }
        );
    }
}
