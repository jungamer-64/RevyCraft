use crate::RuntimeError;
use crate::config::ServerConfig;
use mc_plugin_host::registry::ProtocolRegistry;
use mc_proto_common::{Edition, ProtocolAdapter, TransportKind};
use std::sync::Arc;

pub(in crate::runtime) struct ActiveProtocols {
    pub(in crate::runtime) protocols: ProtocolRegistry,
    pub(in crate::runtime) default_adapter: Arc<dyn ProtocolAdapter>,
    pub(in crate::runtime) default_bedrock_adapter: Option<Arc<dyn ProtocolAdapter>>,
}

pub(in crate::runtime) fn activate_protocols(
    config: &ServerConfig,
    protocols: &ProtocolRegistry,
) -> Result<ActiveProtocols, RuntimeError> {
    if protocols
        .resolve_adapter(&config.topology.default_adapter)
        .is_none()
    {
        return Err(RuntimeError::Config(format!(
            "unknown default-adapter `{}`",
            config.topology.default_adapter
        )));
    }
    if config.topology.be_enabled
        && protocols
            .resolve_adapter(&config.topology.default_bedrock_adapter)
            .is_none()
    {
        return Err(RuntimeError::Config(format!(
            "unknown default-bedrock-adapter `{}`",
            config.topology.default_bedrock_adapter
        )));
    }

    let mut enabled_adapter_ids = config.effective_enabled_adapters();
    if !enabled_adapter_ids
        .iter()
        .any(|adapter_id| adapter_id == &config.topology.default_adapter)
    {
        return Err(RuntimeError::Config(format!(
            "default-adapter `{}` must be included in enabled-adapters",
            config.topology.default_adapter
        )));
    }
    let enabled_bedrock_adapter_ids = if config.topology.be_enabled {
        let enabled = config.effective_enabled_bedrock_adapters();
        if !enabled
            .iter()
            .any(|adapter_id| adapter_id == &config.topology.default_bedrock_adapter)
        {
            return Err(RuntimeError::Config(format!(
                "default-bedrock-adapter `{}` must be included in enabled-bedrock-adapters",
                config.topology.default_bedrock_adapter
            )));
        }
        enabled
    } else {
        Vec::new()
    };
    enabled_adapter_ids.extend(enabled_bedrock_adapter_ids.iter().cloned());
    let active_protocols = protocols.filter_enabled(&enabled_adapter_ids)?;
    if !config.topology.be_enabled
        && !active_protocols
            .adapter_ids_for_transport(TransportKind::Udp)
            .is_empty()
    {
        return Err(RuntimeError::Config(
            "enabled-adapters contains udp adapters but be-enabled=false".to_string(),
        ));
    }

    let default_adapter = active_protocols
        .resolve_adapter(&config.topology.default_adapter)
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "default-adapter `{}` is not active",
                config.topology.default_adapter
            ))
        })?;
    if default_adapter.descriptor().transport != TransportKind::Tcp {
        return Err(RuntimeError::Config(format!(
            "default-adapter `{}` must be a tcp adapter",
            config.topology.default_adapter
        )));
    }

    let default_bedrock_adapter = if config.topology.be_enabled {
        let adapter = active_protocols
            .resolve_adapter(&config.topology.default_bedrock_adapter)
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "default-bedrock-adapter `{}` is not active",
                    config.topology.default_bedrock_adapter
                ))
            })?;
        let descriptor = adapter.descriptor();
        if descriptor.transport != TransportKind::Udp || descriptor.edition != Edition::Be {
            return Err(RuntimeError::Config(format!(
                "default-bedrock-adapter `{}` must be a bedrock udp adapter",
                config.topology.default_bedrock_adapter
            )));
        }
        Some(adapter)
    } else {
        None
    };

    Ok(ActiveProtocols {
        protocols: active_protocols,
        default_adapter,
        default_bedrock_adapter,
    })
}
