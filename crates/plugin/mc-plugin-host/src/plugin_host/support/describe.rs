use super::{
    AdminUiDescriptor, AdminUiOutput, AuthDescriptor, AuthResponse, BedrockListenerDescriptor,
    CapabilitySet, GameplayDescriptor, GameplayResponse, ProtocolDescriptor, ProtocolResponse,
    RuntimeError, StorageDescriptor, StorageResponse,
};

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
) -> Result<CapabilitySet, RuntimeError> {
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
) -> Result<CapabilitySet, RuntimeError> {
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
) -> Result<CapabilitySet, RuntimeError> {
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
) -> Result<CapabilitySet, RuntimeError> {
    match response {
        AuthResponse::CapabilitySet(capabilities) => Ok(capabilities),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected auth capability payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_admin_ui_descriptor(
    plugin_id: &str,
    response: AdminUiOutput,
) -> Result<AdminUiDescriptor, RuntimeError> {
    match response {
        AdminUiOutput::Descriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected admin-ui describe payload: {other:?}"
        ))),
    }
}

pub(crate) fn expect_admin_ui_capabilities(
    plugin_id: &str,
    response: AdminUiOutput,
) -> Result<CapabilitySet, RuntimeError> {
    match response {
        AdminUiOutput::CapabilitySet(capabilities) => Ok(capabilities),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected admin-ui capability payload: {other:?}"
        ))),
    }
}
