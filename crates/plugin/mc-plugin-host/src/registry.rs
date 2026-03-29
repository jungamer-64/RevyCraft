use crate::PluginHostError as RuntimeError;
use crate::runtime::{
    AdminSurfaceProfileHandle, AuthProfileHandle, GameplayProfileHandle, StorageProfileHandle,
};
use mc_proto_common::{
    Edition, HandshakeIntent, HandshakeProbe, ProtocolAdapter, ProtocolError, TransportKind,
};
use revy_voxel_core::{
    AdapterId, AdminSurfaceProfileId, AuthProfileId, GameplayProfileId, StorageProfileId,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenerBinding {
    pub transport: TransportKind,
    pub local_addr: SocketAddr,
    pub adapter_ids: Vec<AdapterId>,
}

#[derive(Clone)]
pub struct ProtocolRegistry {
    adapters_by_id: HashMap<AdapterId, Arc<dyn ProtocolAdapter>>,
    adapters_by_route: HashMap<(TransportKind, Edition, i32), Arc<dyn ProtocolAdapter>>,
    probes: Vec<Arc<dyn HandshakeProbe>>,
}

impl ProtocolRegistry {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            adapters_by_id: HashMap::new(),
            adapters_by_route: HashMap::new(),
            probes: Vec::new(),
        }
    }

    pub(crate) fn register_adapter(
        &mut self,
        adapter: Arc<dyn ProtocolAdapter>,
    ) -> Result<&mut Self, RuntimeError> {
        let descriptor = adapter.descriptor();
        let adapter_id = AdapterId::new(descriptor.adapter_id.clone());
        if let Some(existing) = self.adapters_by_route.get(&(
            descriptor.transport,
            descriptor.edition,
            descriptor.protocol_number,
        )) {
            let existing_descriptor = existing.descriptor();
            return Err(RuntimeError::Config(format!(
                "protocol route collision: existing adapter `{}` conflicts with candidate `{}` for {:?}/{:?}/{}",
                existing_descriptor.adapter_id,
                descriptor.adapter_id,
                descriptor.transport,
                descriptor.edition,
                descriptor.protocol_number,
            )));
        }
        self.adapters_by_route.insert(
            (
                descriptor.transport,
                descriptor.edition,
                descriptor.protocol_number,
            ),
            Arc::clone(&adapter),
        );
        self.adapters_by_id.insert(adapter_id, adapter);
        Ok(self)
    }

    pub(crate) fn register_probe(&mut self, probe: Arc<dyn HandshakeProbe>) -> &mut Self {
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
    pub fn filter_enabled(&self, enabled_adapters: &[AdapterId]) -> Result<Self, RuntimeError> {
        let mut filtered = Self::new();
        let mut seen = HashSet::new();
        for adapter_id in enabled_adapters {
            if !seen.insert(adapter_id.clone()) {
                return Err(RuntimeError::Config(format!(
                    "enabled-adapters contains duplicate adapter `{adapter_id}`"
                )));
            }
            let Some(adapter) = self.resolve_adapter(adapter_id.as_str()) else {
                return Err(RuntimeError::Config(format!(
                    "enabled-adapters contains unknown adapter `{adapter_id}`"
                )));
            };
            filtered.register_adapter(adapter)?;
        }
        filtered.probes.clone_from(&self.probes);
        Ok(filtered)
    }

    #[must_use]
    pub fn adapter_ids_for_transport(&self, transport_kind: TransportKind) -> Vec<AdapterId> {
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

#[derive(Clone)]
pub struct LoadedPluginSet {
    protocols: ProtocolRegistry,
    gameplay_profiles: HashMap<GameplayProfileId, Arc<dyn GameplayProfileHandle>>,
    storage_profiles: HashMap<StorageProfileId, Arc<dyn StorageProfileHandle>>,
    auth_profiles: HashMap<AuthProfileId, Arc<dyn AuthProfileHandle>>,
    admin_surface_profiles: HashMap<AdminSurfaceProfileId, Arc<dyn AdminSurfaceProfileHandle>>,
}

impl LoadedPluginSet {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            protocols: ProtocolRegistry::new(),
            gameplay_profiles: HashMap::new(),
            storage_profiles: HashMap::new(),
            auth_profiles: HashMap::new(),
            admin_surface_profiles: HashMap::new(),
        }
    }

    pub(crate) fn register_gameplay_profile(
        &mut self,
        profile_id: GameplayProfileId,
        profile: Arc<dyn GameplayProfileHandle>,
    ) -> &mut Self {
        self.gameplay_profiles.insert(profile_id, profile);
        self
    }

    pub(crate) fn register_storage_profile(
        &mut self,
        profile_id: StorageProfileId,
        profile: Arc<dyn StorageProfileHandle>,
    ) -> &mut Self {
        self.storage_profiles.insert(profile_id, profile);
        self
    }

    pub(crate) fn register_auth_profile(
        &mut self,
        profile_id: AuthProfileId,
        profile: Arc<dyn AuthProfileHandle>,
    ) -> &mut Self {
        self.auth_profiles.insert(profile_id, profile);
        self
    }

    pub(crate) fn register_admin_surface_profile(
        &mut self,
        profile_id: AdminSurfaceProfileId,
        profile: Arc<dyn AdminSurfaceProfileHandle>,
    ) -> &mut Self {
        self.admin_surface_profiles.insert(profile_id, profile);
        self
    }

    pub(crate) fn replace_protocols(&mut self, protocols: ProtocolRegistry) -> &mut Self {
        self.protocols = protocols;
        self
    }

    #[must_use]
    pub const fn protocols(&self) -> &ProtocolRegistry {
        &self.protocols
    }

    #[must_use]
    pub fn resolve_gameplay_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn GameplayProfileHandle>> {
        self.gameplay_profiles.get(profile_id).cloned()
    }

    #[must_use]
    pub fn resolve_storage_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn StorageProfileHandle>> {
        self.storage_profiles.get(profile_id).cloned()
    }

    #[must_use]
    pub fn resolve_auth_profile(&self, profile_id: &str) -> Option<Arc<dyn AuthProfileHandle>> {
        self.auth_profiles.get(profile_id).cloned()
    }

    #[must_use]
    pub fn resolve_admin_surface_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<dyn AdminSurfaceProfileHandle>> {
        self.admin_surface_profiles.get(profile_id).cloned()
    }
}
