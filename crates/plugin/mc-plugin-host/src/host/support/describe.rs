use super::{
    AdminSurfaceCapability, AdminSurfaceDescriptor, AdminSurfaceResponse, AuthCapability,
    AuthDescriptor, AuthResponse, BedrockListenerDescriptor, GameplayCapability,
    GameplayDescriptor, GameplayResponse, ProtocolCapability, ProtocolDescriptor, ProtocolResponse,
    RuntimeError, StorageCapability, StorageDescriptor, StorageResponse,
};
use mc_plugin_api::CapabilityAnnouncement;

pub(crate) fn expect_protocol_descriptor(
    plugin_id: &str,
    response: ProtocolResponse,
) -> Result<ProtocolDescriptor, RuntimeError> {
    match response {
        ProtocolResponse::Descriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected describe payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_protocol_bedrock_listener_descriptor(
    plugin_id: &str,
    response: ProtocolResponse,
) -> Result<Option<BedrockListenerDescriptor>, RuntimeError> {
    match response {
        ProtocolResponse::BedrockListenerDescriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected bedrock listener payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_protocol_capabilities(
    plugin_id: &str,
    response: ProtocolResponse,
) -> Result<CapabilityAnnouncement<ProtocolCapability>, RuntimeError> {
    match response {
        ProtocolResponse::CapabilitySet(capabilities) => Ok(capabilities),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected capability payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_gameplay_descriptor(
    plugin_id: &str,
    response: GameplayResponse,
) -> Result<GameplayDescriptor, RuntimeError> {
    match response {
        GameplayResponse::Descriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected gameplay describe payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_gameplay_capabilities(
    plugin_id: &str,
    response: GameplayResponse,
) -> Result<CapabilityAnnouncement<GameplayCapability>, RuntimeError> {
    match response {
        GameplayResponse::CapabilitySet(capabilities) => Ok(capabilities),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected gameplay capability payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_storage_descriptor(
    plugin_id: &str,
    response: StorageResponse,
) -> Result<StorageDescriptor, RuntimeError> {
    match response {
        StorageResponse::Descriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected storage describe payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_storage_capabilities(
    plugin_id: &str,
    response: StorageResponse,
) -> Result<CapabilityAnnouncement<StorageCapability>, RuntimeError> {
    match response {
        StorageResponse::CapabilitySet(capabilities) => Ok(capabilities),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected storage capability payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_auth_descriptor(
    plugin_id: &str,
    response: AuthResponse,
) -> Result<AuthDescriptor, RuntimeError> {
    match response {
        AuthResponse::Descriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected auth describe payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_auth_capabilities(
    plugin_id: &str,
    response: AuthResponse,
) -> Result<CapabilityAnnouncement<AuthCapability>, RuntimeError> {
    match response {
        AuthResponse::CapabilitySet(capabilities) => Ok(capabilities),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected auth capability payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_admin_surface_descriptor(
    plugin_id: &str,
    response: AdminSurfaceResponse,
) -> Result<AdminSurfaceDescriptor, RuntimeError> {
    match response {
        AdminSurfaceResponse::Descriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected admin-surface describe payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_admin_surface_capabilities(
    plugin_id: &str,
    response: AdminSurfaceResponse,
) -> Result<CapabilityAnnouncement<AdminSurfaceCapability>, RuntimeError> {
    match response {
        AdminSurfaceResponse::CapabilitySet(capabilities) => Ok(capabilities),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected admin-surface capability payload: {other:?}"
        ))),
    }
}
