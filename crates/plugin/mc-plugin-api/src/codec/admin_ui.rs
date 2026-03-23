use crate::abi::{CURRENT_PLUGIN_ABI, PluginKind};
use crate::codec::__internal::binary::{
    EnvelopeHeader, PROTOCOL_FLAG_RESPONSE, ProtocolCodecError, decode_envelope, encode_envelope,
};
use mc_core::{AdminUiProfileId, ConnectionId, EntityId, PlayerId, PluginGenerationId};
use mc_proto_common::{ConnectionPhase, TransportKind};
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AdminUiOpCode {
    Describe = 1,
    CapabilitySet = 2,
    ParseLine = 3,
    RenderResponse = 4,
}

impl TryFrom<u8> for AdminUiOpCode {
    type Error = ProtocolCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Describe),
            2 => Ok(Self::CapabilitySet),
            3 => Ok(Self::ParseLine),
            4 => Ok(Self::RenderResponse),
            _ => Err(ProtocolCodecError::InvalidValue("invalid admin-ui op code")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminUiDescriptor {
    pub ui_profile: AdminUiProfileId,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminUiInput {
    Describe,
    CapabilitySet,
    ParseLine { line: String },
    RenderResponse { response: AdminResponse },
}

impl AdminUiInput {
    #[must_use]
    pub const fn op_code(&self) -> AdminUiOpCode {
        match self {
            Self::Describe => AdminUiOpCode::Describe,
            Self::CapabilitySet => AdminUiOpCode::CapabilitySet,
            Self::ParseLine { .. } => AdminUiOpCode::ParseLine,
            Self::RenderResponse { .. } => AdminUiOpCode::RenderResponse,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminUiOutput {
    Descriptor(AdminUiDescriptor),
    CapabilitySet(mc_core::CapabilitySet),
    ParsedRequest(AdminRequest),
    RenderedText(String),
}

fn encode_admin_ui_payload<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolCodecError> {
    serde_json::to_vec(value)
        .map_err(|_| ProtocolCodecError::InvalidValue("failed to encode admin-ui payload"))
}

fn decode_admin_ui_payload<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
) -> Result<T, ProtocolCodecError> {
    serde_json::from_slice(bytes)
        .map_err(|_| ProtocolCodecError::InvalidValue("failed to decode admin-ui payload"))
}

pub fn encode_admin_ui_input(input: &AdminUiInput) -> Result<Vec<u8>, ProtocolCodecError> {
    let payload = encode_admin_ui_payload(input)?;
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::AdminUi,
            op_code: input.op_code() as u8,
            flags: 0,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

pub fn decode_admin_ui_input(bytes: &[u8]) -> Result<AdminUiInput, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::AdminUi {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-ui input had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE != 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-ui input unexpectedly set response flag",
        ));
    }
    let input = decode_admin_ui_payload::<AdminUiInput>(payload)?;
    if input.op_code() != AdminUiOpCode::try_from(header.op_code)? {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-ui input opcode did not match payload",
        ));
    }
    Ok(input)
}

pub fn encode_admin_ui_output(
    input: &AdminUiInput,
    output: &AdminUiOutput,
) -> Result<Vec<u8>, ProtocolCodecError> {
    let payload = encode_admin_ui_payload(output)?;
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::AdminUi,
            op_code: input.op_code() as u8,
            flags: PROTOCOL_FLAG_RESPONSE,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

pub fn decode_admin_ui_output(
    input: &AdminUiInput,
    bytes: &[u8],
) -> Result<AdminUiOutput, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::AdminUi {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-ui output had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE == 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-ui output was missing response flag",
        ));
    }
    if input.op_code() != AdminUiOpCode::try_from(header.op_code)? {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-ui output opcode did not match input",
        ));
    }
    decode_admin_ui_payload(payload)
}
