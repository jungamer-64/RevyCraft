use super::{RuntimeReloadContext, RuntimeServer, SessionMessage, SessionState, now_ms};
use crate::RuntimeError;
use crate::host::PluginHost;
use mc_core::{ConnectionId, CoreCommand, CoreEvent, EventTarget, PlayerSummary, TargetedEvent};
use mc_plugin_api::GameplaySessionSnapshot;
use std::sync::Arc;

impl RuntimeServer {
    pub(super) async fn apply_command(
        &self,
        command: CoreCommand,
        session: Option<&SessionState>,
    ) -> Result<(), RuntimeError> {
        let should_persist = matches!(
            command,
            CoreCommand::LoginStart { .. }
                | CoreCommand::MoveIntent { .. }
                | CoreCommand::SetHeldSlot { .. }
                | CoreCommand::CreativeInventorySet { .. }
                | CoreCommand::DigBlock { .. }
                | CoreCommand::PlaceBlock { .. }
                | CoreCommand::Disconnect { .. }
        );
        let session_capabilities = session.and_then(|session| session.session_capabilities.clone());
        let gameplay = session.and_then(|session| session.gameplay.clone());
        let events = {
            let mut state = self.state.lock().await;
            let events = if let (Some(session_capabilities), Some(gameplay)) =
                (session_capabilities.as_ref(), gameplay.as_ref())
            {
                state
                    .core
                    .apply_command_with_policy(
                        command,
                        now_ms(),
                        Some(session_capabilities),
                        gameplay.as_ref(),
                    )
                    .map_err(RuntimeError::Config)?
            } else {
                state.core.apply_command(command, now_ms())
            };
            if should_persist {
                state.dirty = true;
            }
            events
        };
        self.dispatch_events(events).await;
        Ok(())
    }

    pub(super) async fn tick(&self) -> Result<(), RuntimeError> {
        let gameplay_sessions = {
            self.sessions
                .lock()
                .await
                .values()
                .filter_map(|handle| {
                    let player_id = handle.player_id?;
                    let session_capabilities = handle.session_capabilities.clone()?;
                    let gameplay_profile = handle.gameplay_profile.clone()?;
                    Some((player_id, session_capabilities, gameplay_profile))
                })
                .collect::<Vec<_>>()
        };
        let events = {
            let mut state = self.state.lock().await;
            let now = now_ms();
            let mut events = state.core.tick(now);
            for (player_id, session_capabilities, gameplay_profile) in &gameplay_sessions {
                let Some(gameplay) = self.plugin_host.as_ref().and_then(|plugin_host| {
                    plugin_host.resolve_gameplay_profile(gameplay_profile.as_str())
                }) else {
                    continue;
                };
                events.extend(
                    state
                        .core
                        .tick_player_with_policy(
                            *player_id,
                            now,
                            session_capabilities,
                            gameplay.as_ref(),
                        )
                        .map_err(RuntimeError::Config)?,
                );
            }
            events
        };
        self.dispatch_events(events).await;
        Ok(())
    }

    pub(super) async fn maybe_save(&self) -> Result<(), RuntimeError> {
        let snapshot = {
            let mut state = self.state.lock().await;
            if !state.dirty {
                return Ok(());
            }
            state.dirty = false;
            state.core.snapshot()
        };
        self.storage_profile
            .save_snapshot(&self.config.world_dir, &snapshot)?;
        Ok(())
    }

    async fn dispatch_events(&self, events: Vec<TargetedEvent>) {
        for event in events {
            let TargetedEvent {
                target,
                event: payload,
            } = event;
            if let (
                EventTarget::Connection(connection_id),
                CoreEvent::LoginAccepted { player_id, .. },
            ) = (&target, &payload)
                && let Some(session) = self.sessions.lock().await.get_mut(connection_id)
            {
                session.player_id = Some(*player_id);
            }
            let payload = Arc::new(payload);

            let recipients = {
                let sessions = self.sessions.lock().await;
                match target {
                    EventTarget::Connection(connection_id) => sessions
                        .get(&connection_id)
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                    EventTarget::Player(target_player_id) => sessions
                        .values()
                        .filter(|session| session.player_id == Some(target_player_id))
                        .cloned()
                        .collect::<Vec<_>>(),
                    EventTarget::EveryoneExcept(excluded_player_id) => sessions
                        .values()
                        .filter(|session| {
                            session.player_id.is_some()
                                && session.player_id != Some(excluded_player_id)
                        })
                        .cloned()
                        .collect::<Vec<_>>(),
                }
            };

            for recipient in recipients {
                let _ = recipient
                    .tx
                    .send(SessionMessage::Event(Arc::clone(&payload)));
            }
        }
    }

    pub(super) async fn unregister_session(
        &self,
        connection_id: ConnectionId,
        session: &SessionState,
    ) -> Result<(), RuntimeError> {
        if let (Some(gameplay), Some(gameplay_profile), Some(player_id)) = (
            session.gameplay.as_ref(),
            session
                .session_capabilities
                .as_ref()
                .map(|capabilities| capabilities.gameplay_profile.clone()),
            session.player_id,
        ) {
            gameplay.session_closed(&GameplaySessionSnapshot {
                phase: session.phase,
                player_id: Some(player_id),
                entity_id: session.entity_id,
                gameplay_profile,
            })?;
        }
        self.sessions.lock().await.remove(&connection_id);
        if let Some(player_id) = session.player_id {
            self.apply_command(CoreCommand::Disconnect { player_id }, None)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn player_summary(&self) -> PlayerSummary {
        self.state.lock().await.core.player_summary()
    }

    async fn reload_context(&self) -> RuntimeReloadContext {
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
            gameplay_sessions,
            snapshot,
            world_dir: self.config.world_dir.clone(),
        }
    }

    pub(super) async fn reload_plugins(
        &self,
        plugin_host: &PluginHost,
    ) -> Result<Vec<String>, RuntimeError> {
        let context = self.reload_context().await;
        plugin_host.reload_modified_with_context(&context).await
    }
}
