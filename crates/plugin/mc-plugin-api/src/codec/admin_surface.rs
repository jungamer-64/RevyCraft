use crate::abi::{CURRENT_PLUGIN_ABI, PluginKind};
use crate::codec::__internal::binary::{
    EnvelopeHeader, PROTOCOL_FLAG_RESPONSE, ProtocolCodecError, decode_envelope, encode_envelope,
};
use crate::codec::admin::{AdminPermission, RuntimeReloadMode};
use revy_voxel_core::{AdminSurfaceCapability, AdminSurfaceProfileId, CapabilityAnnouncement};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AdminSurfaceOpCode {
    Describe = 1,
    CapabilitySet = 2,
    DeclareInstance = 3,
    Start = 4,
    PauseForUpgrade = 5,
    ResumeFromUpgrade = 6,
    ActivateAfterUpgradeCommit = 7,
    ResumeAfterUpgradeRollback = 8,
    Shutdown = 9,
}

impl TryFrom<u8> for AdminSurfaceOpCode {
    type Error = ProtocolCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Describe),
            2 => Ok(Self::CapabilitySet),
            3 => Ok(Self::DeclareInstance),
            4 => Ok(Self::Start),
            5 => Ok(Self::PauseForUpgrade),
            6 => Ok(Self::ResumeFromUpgrade),
            7 => Ok(Self::ActivateAfterUpgradeCommit),
            8 => Ok(Self::ResumeAfterUpgradeRollback),
            9 => Ok(Self::Shutdown),
            _ => Err(ProtocolCodecError::InvalidValue(
                "invalid admin-surface op code",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSurfaceDescriptor {
    pub surface_profile: AdminSurfaceProfileId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSurfaceEndpointView {
    pub surface: String,
    pub local_addr: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSurfaceStatusView {
    pub endpoints: Vec<AdminSurfaceEndpointView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSurfacePauseView {
    pub resume_payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminSurfaceResource {
    Bytes(Vec<u8>),
    NativeHandle {
        handle_kind: String,
        raw_handle: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSurfacePrincipalDeclaration {
    pub principal_id: String,
    pub permissions: Vec<AdminPermission>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSurfaceInstanceDeclaration {
    pub principals: Vec<AdminSurfacePrincipalDeclaration>,
    pub required_process_resources: Vec<String>,
    pub supports_upgrade_handoff: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminSurfaceRequest {
    Describe,
    CapabilitySet,
    DeclareInstance {
        instance_id: String,
        surface_config_path: Option<String>,
    },
    Start {
        instance_id: String,
        surface_config_path: Option<String>,
    },
    PauseForUpgrade {
        instance_id: String,
    },
    ResumeFromUpgrade {
        instance_id: String,
        surface_config_path: Option<String>,
        resume_payload: Vec<u8>,
    },
    ActivateAfterUpgradeCommit {
        instance_id: String,
    },
    ResumeAfterUpgradeRollback {
        instance_id: String,
    },
    Shutdown {
        instance_id: String,
    },
}

impl AdminSurfaceRequest {
    #[must_use]
    pub const fn op_code(&self) -> AdminSurfaceOpCode {
        match self {
            Self::Describe => AdminSurfaceOpCode::Describe,
            Self::CapabilitySet => AdminSurfaceOpCode::CapabilitySet,
            Self::DeclareInstance { .. } => AdminSurfaceOpCode::DeclareInstance,
            Self::Start { .. } => AdminSurfaceOpCode::Start,
            Self::PauseForUpgrade { .. } => AdminSurfaceOpCode::PauseForUpgrade,
            Self::ResumeFromUpgrade { .. } => AdminSurfaceOpCode::ResumeFromUpgrade,
            Self::ActivateAfterUpgradeCommit { .. } => {
                AdminSurfaceOpCode::ActivateAfterUpgradeCommit
            }
            Self::ResumeAfterUpgradeRollback { .. } => {
                AdminSurfaceOpCode::ResumeAfterUpgradeRollback
            }
            Self::Shutdown { .. } => AdminSurfaceOpCode::Shutdown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminSurfaceResponse {
    Descriptor(AdminSurfaceDescriptor),
    CapabilitySet(CapabilityAnnouncement<AdminSurfaceCapability>),
    Declared(AdminSurfaceInstanceDeclaration),
    Started(AdminSurfaceStatusView),
    Paused(AdminSurfacePauseView),
    Resumed(AdminSurfaceStatusView),
    Activated,
    ResumedAfterRollback(AdminSurfaceStatusView),
    ShutdownComplete,
}

fn encode_payload<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolCodecError> {
    serde_json::to_vec(value)
        .map_err(|_| ProtocolCodecError::InvalidValue("failed to encode admin-surface payload"))
}

fn decode_payload<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ProtocolCodecError> {
    serde_json::from_slice(bytes)
        .map_err(|_| ProtocolCodecError::InvalidValue("failed to decode admin-surface payload"))
}

pub fn encode_admin_surface_request(
    request: &AdminSurfaceRequest,
) -> Result<Vec<u8>, ProtocolCodecError> {
    let payload = encode_payload(request)?;
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::AdminSurface,
            op_code: request.op_code() as u8,
            flags: 0,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

pub fn decode_admin_surface_request(
    bytes: &[u8],
) -> Result<AdminSurfaceRequest, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::AdminSurface {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-surface request had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE != 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-surface request unexpectedly set response flag",
        ));
    }
    let request = decode_payload::<AdminSurfaceRequest>(payload)?;
    if request.op_code() != AdminSurfaceOpCode::try_from(header.op_code)? {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-surface request opcode did not match payload",
        ));
    }
    Ok(request)
}

pub fn encode_admin_surface_response(
    request: &AdminSurfaceRequest,
    response: &AdminSurfaceResponse,
) -> Result<Vec<u8>, ProtocolCodecError> {
    let payload = encode_payload(response)?;
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::AdminSurface,
            op_code: request.op_code() as u8,
            flags: PROTOCOL_FLAG_RESPONSE,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

pub fn decode_admin_surface_response(
    request: &AdminSurfaceRequest,
    bytes: &[u8],
) -> Result<AdminSurfaceResponse, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::AdminSurface {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-surface response had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE == 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-surface response was missing response flag",
        ));
    }
    if request.op_code() != AdminSurfaceOpCode::try_from(header.op_code)? {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "admin-surface response opcode did not match request",
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
            "invalid admin-surface reload mode",
        )),
    }
}
