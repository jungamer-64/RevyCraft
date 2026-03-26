use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SessionState};
use mc_core::{ConnectionId, SessionCapabilitySet};
use mc_proto_common::ProtocolSessionSnapshot;

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

    pub(in crate::runtime) fn protocol_session_snapshot(
        connection_id: ConnectionId,
        session: &SessionState,
    ) -> ProtocolSessionSnapshot {
        ProtocolSessionSnapshot {
            connection_id,
            phase: session.phase,
            player_id: session.player_id,
            entity_id: session.entity_id,
        }
    }

    pub(in crate::runtime::session) async fn resolve_gameplay_for_adapter(
        &self,
        adapter_id: &str,
    ) -> Result<std::sync::Arc<dyn mc_plugin_host::runtime::GameplayProfileHandle>, RuntimeError>
    {
        self.selection
            .resolve_gameplay_for_adapter(adapter_id)
            .await
    }

    pub(in crate::runtime::session) async fn resolve_bedrock_auth_profile(
        &self,
    ) -> Result<std::sync::Arc<dyn mc_plugin_host::runtime::AuthProfileHandle>, RuntimeError> {
        self.selection
            .bedrock_auth_profile()
            .await
            .clone()
            .ok_or_else(|| RuntimeError::Config("bedrock auth profile is not active".to_string()))
    }

    pub(in crate::runtime::session) async fn sync_session_handle(
        &self,
        connection_id: ConnectionId,
        session: &SessionState,
    ) {
        let _consistency_guard = self.reload.read_consistency().await;
        self.sync_session_handle_direct(connection_id, session)
            .await;
    }

    pub(in crate::runtime::session) async fn sync_session_handle_direct(
        &self,
        connection_id: ConnectionId,
        session: &SessionState,
    ) {
        self.sessions
            .sync_from_session(connection_id, session)
            .await;
    }
}
