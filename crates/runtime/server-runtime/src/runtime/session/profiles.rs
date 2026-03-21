use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SessionState};
use mc_core::{ConnectionId, SessionCapabilitySet};
use mc_plugin_host::runtime::{AuthProfileHandle, GameplayProfileHandle};
use std::sync::Arc;

impl RuntimeServer {
    pub(in crate::runtime::session) fn refresh_session_capabilities(session: &mut SessionState) {
        let Some(adapter) = session.adapter.as_ref() else {
            session.session_capabilities = None;
            return;
        };
        let Some(gameplay) = session.gameplay.as_ref() else {
            session.session_capabilities = None;
            return;
        };
        session.session_capabilities = Some(SessionCapabilitySet {
            protocol: adapter.capability_set(),
            gameplay: gameplay.capability_set(),
            gameplay_profile: gameplay.profile_id(),
            entity_id: session.entity_id,
            protocol_generation: adapter.plugin_generation_id(),
            gameplay_generation: gameplay.plugin_generation_id(),
        });
    }

    fn gameplay_profile_for_adapter(&self, adapter_id: &str) -> &str {
        self.config
            .gameplay_profile_map
            .get(adapter_id)
            .map_or(&self.config.default_gameplay_profile, String::as_str)
    }

    pub(in crate::runtime::session) fn resolve_gameplay_for_adapter(
        &self,
        adapter_id: &str,
    ) -> Result<Arc<dyn GameplayProfileHandle>, RuntimeError> {
        let profile_id = self.gameplay_profile_for_adapter(adapter_id);
        self.loaded_plugins
            .resolve_gameplay_profile(profile_id)
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "gameplay profile `{profile_id}` for adapter `{adapter_id}` is not active"
                ))
            })
    }

    pub(in crate::runtime::session) fn resolve_bedrock_auth_profile(
        &self,
    ) -> Result<Arc<dyn AuthProfileHandle>, RuntimeError> {
        self.bedrock_auth_profile
            .clone()
            .ok_or_else(|| RuntimeError::Config("bedrock auth profile is not active".to_string()))
    }

    pub(in crate::runtime::session) async fn sync_session_handle(
        &self,
        connection_id: ConnectionId,
        session: &SessionState,
    ) {
        let _consistency_guard = self.consistency_gate.read().await;
        if let Some(handle) = self.sessions.lock().await.get_mut(&connection_id) {
            handle.topology_generation_id = session.topology_generation_id;
            handle.transport = session.transport;
            handle.phase = session.phase;
            handle.adapter_id = session
                .adapter
                .as_ref()
                .map(|adapter| adapter.descriptor().adapter_id);
            handle.player_id = session.player_id;
            handle.entity_id = session.entity_id;
            handle.gameplay_profile = session
                .session_capabilities
                .as_ref()
                .map(|capabilities| capabilities.gameplay_profile.clone());
            handle
                .session_capabilities
                .clone_from(&session.session_capabilities);
        }
    }
}
