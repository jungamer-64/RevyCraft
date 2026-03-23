use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SessionMessage, SessionState, now_ms};
use mc_core::{CoreCommand, CoreEvent, EventTarget, PlayerSummary, TargetedEvent};
use mc_plugin_api::codec::gameplay::GameplaySessionSnapshot;

impl RuntimeServer {
    pub(in crate::runtime) async fn apply_command(
        &self,
        command: CoreCommand,
        session: Option<&SessionState>,
    ) -> Result<(), RuntimeError> {
        let _consistency_guard = self.consistency_gate.read().await;
        self.apply_command_guarded(command, session).await
    }

    async fn apply_command_guarded(
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
            let now = now_ms();
            let events = if let (Some(session_capabilities), Some(gameplay)) =
                (session_capabilities.as_ref(), gameplay.as_ref())
            {
                state
                    .core
                    .apply_command_with_policy(
                        command,
                        now,
                        Some(session_capabilities),
                        gameplay.as_ref(),
                    )
                    .map_err(RuntimeError::Config)?
            } else {
                debug_assert!(
                    session.is_none()
                        || matches!(
                            &command,
                            CoreCommand::LoginStart { .. } | CoreCommand::Disconnect { .. }
                        ),
                    "session-backed command reached core loop without session capabilities"
                );
                state.core.apply_command(command, now)
            };
            if should_persist {
                state.dirty = true;
            }
            events
        };
        self.dispatch_events(events).await;
        Ok(())
    }

    pub(in crate::runtime) async fn tick(&self) -> Result<(), RuntimeError> {
        let _consistency_guard = self.consistency_gate.read().await;
        self.tick_guarded().await
    }

    async fn tick_guarded(&self) -> Result<(), RuntimeError> {
        let gameplay_sessions = {
            self.sessions
                .lock()
                .await
                .values()
                .filter_map(|handle| {
                    let player_id = handle.player_id?;
                    let session_capabilities = handle.session_capabilities.clone()?;
                    let gameplay = handle.gameplay.clone()?;
                    Some((player_id, session_capabilities, gameplay))
                })
                .collect::<Vec<_>>()
        };
        let events = {
            let mut state = self.state.lock().await;
            let now = now_ms();
            let mut events = state.core.tick(now);
            for (player_id, session_capabilities, gameplay) in &gameplay_sessions {
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
            let payload = std::sync::Arc::new(payload);

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

            let mut backpressured_sessions = Vec::new();
            for recipient in recipients {
                if recipient
                    .tx
                    .try_send(SessionMessage::Event(std::sync::Arc::clone(&payload)))
                    .is_err()
                {
                    backpressured_sessions.push(recipient);
                }
            }
            for recipient in backpressured_sessions {
                let _ = recipient.control_tx.send(Some(
                    "server dropped the session because the outbound queue was full".to_string(),
                ));
            }
        }
    }

    pub(in crate::runtime) async fn unregister_session(
        &self,
        connection_id: mc_core::ConnectionId,
        session: &SessionState,
    ) -> Result<(), RuntimeError> {
        let _consistency_guard = self.consistency_gate.read().await;
        self.unregister_session_guarded(connection_id, session)
            .await
    }

    async fn unregister_session_guarded(
        &self,
        connection_id: mc_core::ConnectionId,
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
            self.apply_command_guarded(CoreCommand::Disconnect { player_id }, None)
                .await?;
        }
        let _ = self.retire_drained_generations().await;
        Ok(())
    }

    pub(in crate::runtime) async fn player_summary(&self) -> PlayerSummary {
        self.state.lock().await.core.player_summary()
    }
}
