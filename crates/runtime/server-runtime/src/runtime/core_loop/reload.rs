use crate::RuntimeError;
use crate::runtime::{RuntimeReloadContext, RuntimeServer};
use mc_plugin_api::codec::gameplay::GameplaySessionSnapshot;
use mc_plugin_api::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_host::runtime::{ProtocolReloadSession, RuntimePluginHost};

impl RuntimeServer {
    pub(in crate::runtime) fn take_pending_plugin_fatal_error(&self) -> Option<RuntimeError> {
        self.reload_host.as_ref().and_then(|reload_host| {
            reload_host
                .take_pending_fatal_error()
                .map(RuntimeError::from)
        })
    }

    pub(in crate::runtime) async fn finish_with_runtime_error(
        &self,
        error: RuntimeError,
    ) -> Result<(), RuntimeError> {
        if matches!(error, RuntimeError::PluginFatal(_)) {
            self.shutdown_listener_workers().await;
            self.terminate_all_sessions("Server stopping due to plugin failure")
                .await;
            if let Err(save_error) = self.maybe_save().await {
                eprintln!("best-effort save during fatal shutdown failed: {save_error}");
            }
        }
        Err(error)
    }

    async fn reload_context(&self) -> RuntimeReloadContext {
        let protocol_sessions = {
            self.sessions
                .lock()
                .await
                .iter()
                .filter_map(|(connection_id, handle)| {
                    let adapter_id = handle.adapter_id.clone()?;
                    if !matches!(
                        handle.phase,
                        mc_proto_common::ConnectionPhase::Status
                            | mc_proto_common::ConnectionPhase::Login
                            | mc_proto_common::ConnectionPhase::Play
                    ) {
                        return None;
                    }
                    Some(ProtocolReloadSession {
                        adapter_id,
                        session: ProtocolSessionSnapshot {
                            connection_id: *connection_id,
                            phase: handle.phase,
                            player_id: handle.player_id,
                            entity_id: handle.entity_id,
                        },
                    })
                })
                .collect::<Vec<_>>()
        };
        let gameplay_sessions = {
            self.sessions
                .lock()
                .await
                .values()
                .filter_map(|handle| {
                    Some(GameplaySessionSnapshot {
                        phase: handle.phase,
                        player_id: Some(handle.player_id?),
                        entity_id: handle.entity_id,
                        gameplay_profile: handle.gameplay_profile.clone()?,
                    })
                })
                .collect::<Vec<_>>()
        };
        let snapshot = { self.state.lock().await.core.snapshot() };
        RuntimeReloadContext {
            protocol_sessions,
            gameplay_sessions,
            snapshot,
            world_dir: self.config.world_dir.clone(),
        }
    }

    pub(in crate::runtime) async fn reload_plugins(
        &self,
        reload_host: &dyn RuntimePluginHost,
    ) -> Result<Vec<String>, RuntimeError> {
        let context = self.reload_context().await;
        Ok(reload_host.reload_modified_with_context(&context)?)
    }
}
