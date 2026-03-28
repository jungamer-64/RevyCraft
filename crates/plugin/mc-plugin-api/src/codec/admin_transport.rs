use crate::abi::{CURRENT_PLUGIN_ABI, PluginKind};
use crate::codec::__internal::binary::{
    EnvelopeHeader, PROTOCOL_FLAG_RESPONSE, ProtocolCodecError, decode_envelope, encode_envelope,
};
use crate::codec::admin_ui::{
    AdminRuntimeReloadView, AdminSessionsView, AdminStatusView, AdminUpgradeRuntimeView,
    RuntimeReloadMode,
};
use mc_core::{AdminTransportCapability, AdminTransportProfileId, CapabilityAnnouncement};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AdminTransportOpCode {
    Describe = 1,
    CapabilitySet = 2,
    Start = 3,
    PauseForUpgrade = 4,
    ResumeFromUpgrade = 5,
    ResumeAfterUpgradeRollback = 6,
    Shutdown = 7,
}

impl TryFrom<u8> for AdminTransportOpCode {
    type Error = ProtocolCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Describe),
            2 => Ok(Self::CapabilitySet),
            3 => Ok(Self::Start),
            4 => Ok(Self::PauseForUpgrade),
            5 => Ok(Self::ResumeFromUpgrade),
            6 => Ok(Self::ResumeAfterUpgradeRollback),
            7 => Ok(Self::Shutdown),
            _ => Err(ProtocolCodecError::InvalidValue(
                "invalid admin-transport op code",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminTransportDescriptor {
    pub transport_profile: AdminTransportProfileId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminTransportEndpointView {
    pub transport: String,
    pub local_addr: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminTransportStatusView {
    pub endpoints: Vec<AdminTransportEndpointView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminTransportPauseView {
    pub resume_payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminTransportRequest {
    Describe,
    CapabilitySet,
    Start {
        transport_config_path: String,
    },
    PauseForUpgrade,
    ResumeFromUpgrade {
        transport_config_path: String,
        resume_payload: Vec<u8>,
    },
    ResumeAfterUpgradeRollback,
    Shutdown,
}

impl AdminTransportRequest {
    #[must_use]
    pub const fn op_code(&self) -> AdminTransportOpCode {
        match self {
            Self::Describe => AdminTransportOpCode::Describe,
            Self::CapabilitySet => AdminTransportOpCode::CapabilitySet,
            Self::Start { .. } => AdminTransportOpCode::Start,
            Self::PauseForUpgrade => AdminTransportOpCode::PauseForUpgrade,
            Self::ResumeFromUpgrade { .. } => AdminTransportOpCode::ResumeFromUpgrade,
            Self::ResumeAfterUpgradeRollback => AdminTransportOpCode::ResumeAfterUpgradeRollback,
            Self::Shutdown => AdminTransportOpCode::Shutdown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminTransportResponse {
    Descriptor(AdminTransportDescriptor),
    CapabilitySet(CapabilityAnnouncement<AdminTransportCapability>),
    Started(AdminTransportStatusView),
    Paused(AdminTransportPauseView),
    Resumed(AdminTransportStatusView),
    ResumedAfterRollback(AdminTransportStatusView),
    ShutdownComplete,
    Status(AdminStatusView),
    Sessions(AdminSessionsView),
    ReloadRuntime(AdminRuntimeReloadView),
    UpgradeRuntime(AdminUpgradeRuntimeView),
}

fn encode_payload<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolCodecError> {
    serde_json::to_vec(value)
        .map_err(|_| ProtocolCodecError::InvalidValue("failed to encode admin-transport payload"))
}

fn decode_payload<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ProtocolCodecError> {
    serde_json::from_slice(bytes)
        .map_err(|_| ProtocolCodecError::InvalidValue("failed to decode admin-transport payload"))
}

pub fn encode_admin_transport_request(
    request: &AdminTransportRequest,
) -> Result<Vec<u8>, ProtocolCodecError> {
    let payload = encode_payload(request)?;
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::AdminTransport,
            op_code: request.op_code() as u8,
            flags: 0,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

pub fn decode_admin_transport_request(
    bytes: &[u8],
) -> Result<AdminTransportRequest, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::AdminTransport {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-transport request had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE != 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-transport request unexpectedly set response flag",
        ));
    }
    let request = decode_payload::<AdminTransportRequest>(payload)?;
    if request.op_code() != AdminTransportOpCode::try_from(header.op_code)? {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-transport request opcode did not match payload",
        ));
    }
    Ok(request)
}

pub fn encode_admin_transport_response(
    request: &AdminTransportRequest,
    response: &AdminTransportResponse,
) -> Result<Vec<u8>, ProtocolCodecError> {
    let payload = encode_payload(response)?;
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::AdminTransport,
            op_code: request.op_code() as u8,
            flags: PROTOCOL_FLAG_RESPONSE,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

pub fn decode_admin_transport_response(
    request: &AdminTransportRequest,
    bytes: &[u8],
) -> Result<AdminTransportResponse, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::AdminTransport {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-transport response had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE == 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-transport response was missing response flag",
        ));
    }
    if request.op_code() != AdminTransportOpCode::try_from(header.op_code)? {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-transport response opcode did not match request",
        ));
    }
    decode_payload(payload)
}

#[must_use]
pub const fn encode_reload_mode(mode: RuntimeReloadMode) -> u8 {
    match mode {
        RuntimeReloadMode::Artifacts => 1,
        RuntimeReloadMode::Topology => 2,
        RuntimeReloadMode::Core => 3,
        RuntimeReloadMode::Full => 4,
    }
}

pub fn decode_reload_mode(mode: u8) -> Result<RuntimeReloadMode, ProtocolCodecError> {
    match mode {
        1 => Ok(RuntimeReloadMode::Artifacts),
        2 => Ok(RuntimeReloadMode::Topology),
        3 => Ok(RuntimeReloadMode::Core),
        4 => Ok(RuntimeReloadMode::Full),
        _ => Err(ProtocolCodecError::InvalidValue(
            "invalid admin-transport reload mode",
        )),
    }
}
