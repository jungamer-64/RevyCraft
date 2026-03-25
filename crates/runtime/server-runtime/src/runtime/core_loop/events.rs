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
        let _consistency_guard = self.reload.read_consistency().await;
        self.apply_command_guarded(command, session).await
    }

    async fn apply_command_guarded(
        &self,
        command: CoreCommand,
        session: Option<&SessionState>,
    ) -> Result<(), RuntimeError> {
        let session_capabilities = session.and_then(|session| session.session_capabilities.clone());
        let gameplay = session.and_then(|session| session.gameplay.clone());
        if session.is_some() {
            debug_assert!(
                session_capabilities.is_some() == gameplay.is_some()
                    || matches!(
                        &command,
                        CoreCommand::LoginStart { .. } | CoreCommand::Disconnect { .. }
                    ),
                "session-backed command reached core loop without matching session context"
            );
        }
        let events = self
            .kernel
            .apply_command(command, session_capabilities, gameplay, now_ms())
            .await?;
        self.dispatch_events(events).await;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn open_test_crafting_table(
        &self,
        player_id: mc_core::PlayerId,
        window_id: u8,
        title: &str,
    ) -> Result<(), RuntimeError> {
        let _consistency_guard = self.reload.read_consistency().await;
        let events = self
            .kernel
            .open_crafting_table(player_id, window_id, title)
            .await;
        self.dispatch_events(events).await;
        Ok(())
    }

    pub(in crate::runtime) async fn tick(&self) -> Result<(), RuntimeError> {
        let _consistency_guard = self.reload.read_consistency().await;
        self.tick_guarded().await
    }

    async fn tick_guarded(&self) -> Result<(), RuntimeError> {
        let gameplay_sessions = self.sessions.gameplay_sessions_for_tick().await;
        let events = self.kernel.tick(&gameplay_sessions, now_ms()).await?;
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
            {
                self.sessions
                    .set_login_player(*connection_id, *player_id)
                    .await;
            }
            let payload = std::sync::Arc::new(payload);

            let recipients = self.sessions.recipients_for_target(target).await;

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
        let _consistency_guard = self.reload.read_consistency().await;
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
        self.sessions.remove(connection_id).await;
        if let Some(player_id) = session.player_id {
            self.apply_command_guarded(CoreCommand::Disconnect { player_id }, None)
                .await?;
        }
        let _ = self
            .topology
            .retire_drained_generations(&self.sessions)
            .await;
        Ok(())
    }

    pub(in crate::runtime) async fn player_summary(&self) -> PlayerSummary {
        self.kernel.player_summary().await
    }
}
