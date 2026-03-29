use crate::RuntimeError;
use crate::runtime::{
    RuntimeServer, SessionRuntimeContext, SessionState, SessionView, SharedSessionState,
};
use mc_plugin_api::codec::gameplay::GameplaySessionSnapshot;
use mc_proto_common::ProtocolSessionSnapshot;
use revy_voxel_core::{ConnectionId, SessionCapabilitySet};

impl RuntimeServer {
    pub(in crate::runtime) fn refresh_session_capabilities(session: &mut SessionState) {
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

    pub(in crate::runtime) fn session_view(session: &SessionState) -> SessionView {
        SessionView {
            generation_id: session.generation.generation_id,
            transport: session.transport,
            phase: session.phase,
            adapter_id: session
                .adapter
                .as_ref()
                .map(|adapter| adapter.descriptor().adapter_id),
            player_id: session.player_id,
            entity_id: session.entity_id,
            gameplay_profile: session
                .session_capabilities
                .as_ref()
                .map(|capabilities| capabilities.gameplay_profile.clone()),
            protocol_generation: session
                .session_capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.protocol_generation),
            gameplay_generation: session
                .session_capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.gameplay_generation),
        }
    }

    pub(in crate::runtime) fn session_runtime_context(
        session: &SessionState,
    ) -> SessionRuntimeContext {
        SessionRuntimeContext {
            player_id: session.player_id,
            gameplay: session.gameplay.clone(),
            session_capabilities: session.session_capabilities.clone(),
        }
    }

    pub(in crate::runtime) async fn read_session_view(
        shared_state: &SharedSessionState,
    ) -> SessionView {
        let session = shared_state.read().await;
        Self::session_view(&session)
    }

    pub(in crate::runtime) async fn read_session_runtime_context(
        shared_state: &SharedSessionState,
    ) -> SessionRuntimeContext {
        let session = shared_state.read().await;
        Self::session_runtime_context(&session)
    }

    pub(in crate::runtime) fn protocol_session_snapshot(
        connection_id: ConnectionId,
        session: &SessionView,
    ) -> ProtocolSessionSnapshot {
        ProtocolSessionSnapshot {
            connection_id,
            phase: session.phase,
            player_id: session.player_id,
            entity_id: session.entity_id,
        }
    }

    pub(in crate::runtime) fn gameplay_session_snapshot(
        view: &SessionView,
        context: &SessionRuntimeContext,
    ) -> Option<GameplaySessionSnapshot> {
        let player_id = view.player_id?;
        let session_capabilities = context.session_capabilities.as_ref()?;
        Some(GameplaySessionSnapshot {
            phase: view.phase,
            player_id: Some(player_id),
            entity_id: view.entity_id,
            protocol: session_capabilities.protocol.clone(),
            gameplay_profile: session_capabilities.gameplay_profile.clone(),
            protocol_generation: session_capabilities.protocol_generation,
            gameplay_generation: session_capabilities.gameplay_generation,
        })
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
}
