use crate::RuntimeError;
use crate::host::PluginHost;
use mc_proto_common::{
    Edition, HandshakeIntent, HandshakeProbe, ProtocolAdapter, ProtocolError, StorageAdapter,
    TransportKind,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenerBinding {
    pub transport: TransportKind,
    pub local_addr: SocketAddr,
    pub adapter_ids: Vec<String>,
}

#[derive(Clone, Default)]
pub struct ProtocolRegistry {
    adapters_by_id: HashMap<String, Arc<dyn ProtocolAdapter>>,
    adapters_by_route: HashMap<(TransportKind, Edition, i32), Arc<dyn ProtocolAdapter>>,
    probes: Vec<Arc<dyn HandshakeProbe>>,
}

impl ProtocolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_adapter(&mut self, adapter: Arc<dyn ProtocolAdapter>) -> &mut Self {
        let descriptor = adapter.descriptor();
        let adapter_id = descriptor.adapter_id;
        self.adapters_by_route.insert(
            (
                descriptor.transport,
                descriptor.edition,
                descriptor.protocol_number,
            ),
            Arc::clone(&adapter),
        );
        self.adapters_by_id.insert(adapter_id, adapter);
        self
    }

    pub fn register_probe(&mut self, probe: Arc<dyn HandshakeProbe>) -> &mut Self {
        self.probes.push(probe);
        self
    }

    #[must_use]
    pub fn resolve_adapter(&self, adapter_id: &str) -> Option<Arc<dyn ProtocolAdapter>> {
        self.adapters_by_id.get(adapter_id).cloned()
    }

    #[must_use]
    pub fn resolve_route(
        &self,
        transport_kind: TransportKind,
        edition: Edition,
        protocol_number: i32,
    ) -> Option<Arc<dyn ProtocolAdapter>> {
        self.adapters_by_route
            .get(&(transport_kind, edition, protocol_number))
            .cloned()
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when `enabled_adapters` contains duplicates or
    /// unknown adapter identifiers.
    pub fn filter_enabled(&self, enabled_adapters: &[String]) -> Result<Self, RuntimeError> {
        let mut filtered = Self::new();
        let mut seen = HashSet::new();
        for adapter_id in enabled_adapters {
            if !seen.insert(adapter_id.clone()) {
                return Err(RuntimeError::Config(format!(
                    "enabled-adapters contains duplicate adapter `{adapter_id}`"
                )));
            }
            let Some(adapter) = self.resolve_adapter(adapter_id) else {
                return Err(RuntimeError::Config(format!(
                    "enabled-adapters contains unknown adapter `{adapter_id}`"
                )));
            };
            filtered.register_adapter(adapter);
        }
        filtered.probes.clone_from(&self.probes);
        Ok(filtered)
    }

    #[must_use]
    pub fn adapter_ids_for_transport(&self, transport_kind: TransportKind) -> Vec<String> {
        let mut adapter_ids = self
            .adapters_by_id
            .iter()
            .filter(|(_, adapter)| adapter.descriptor().transport == transport_kind)
            .map(|(adapter_id, _)| adapter_id.clone())
            .collect::<Vec<_>>();
        adapter_ids.sort();
        adapter_ids
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError`] when a registered probe matches the frame's
    /// protocol family but the payload is malformed for that family.
    pub fn route_handshake(
        &self,
        transport_kind: TransportKind,
        frame: &[u8],
    ) -> Result<Option<HandshakeIntent>, ProtocolError> {
        for probe in &self.probes {
            if probe.transport_kind() != transport_kind {
                continue;
            }
            if let Some(intent) = probe.try_route(frame)? {
                return Ok(Some(intent));
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Default)]
pub struct StorageRegistry {
    profiles: HashMap<String, Arc<dyn StorageAdapter>>,
}

impl StorageRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_profile(
        &mut self,
        storage_profile: impl Into<String>,
        adapter: Arc<dyn StorageAdapter>,
    ) -> &mut Self {
        self.profiles.insert(storage_profile.into(), adapter);
        self
    }

    #[must_use]
    pub fn resolve(&self, storage_profile: &str) -> Option<Arc<dyn StorageAdapter>> {
        self.profiles.get(storage_profile).cloned()
    }
}

#[derive(Clone, Default)]
pub struct RuntimeRegistries {
    protocols: ProtocolRegistry,
    storage: StorageRegistry,
    plugin_host: Option<Arc<PluginHost>>,
}

impl RuntimeRegistries {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_adapter(&mut self, adapter: Arc<dyn ProtocolAdapter>) -> &mut Self {
        self.protocols.register_adapter(adapter);
        self
    }

    pub fn register_probe(&mut self, probe: Arc<dyn HandshakeProbe>) -> &mut Self {
        self.protocols.register_probe(probe);
        self
    }

    pub fn register_storage_profile(
        &mut self,
        storage_profile: impl Into<String>,
        adapter: Arc<dyn StorageAdapter>,
    ) -> &mut Self {
        self.storage.register_profile(storage_profile, adapter);
        self
    }

    pub fn attach_plugin_host(&mut self, plugin_host: Arc<PluginHost>) -> &mut Self {
        self.plugin_host = Some(plugin_host);
        self
    }

    pub(crate) fn replace_protocols(&mut self, protocols: ProtocolRegistry) -> &mut Self {
        self.protocols = protocols;
        self
    }

    #[must_use]
    pub fn plugin_host(&self) -> Option<Arc<PluginHost>> {
        self.plugin_host.clone()
    }

    #[must_use]
    pub const fn protocols(&self) -> &ProtocolRegistry {
        &self.protocols
    }

    #[must_use]
    pub const fn storage(&self) -> &StorageRegistry {
        &self.storage
    }
}
