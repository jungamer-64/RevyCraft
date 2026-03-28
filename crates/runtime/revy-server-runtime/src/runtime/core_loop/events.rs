use crate::RuntimeError;
use crate::runtime::{
    KernelCommandOutcome, RuntimeServer, SessionControl, SessionMessage, SessionRuntimeContext,
    SharedSessionState, now_ms,
};
use mc_core::{
    CoreCommand, CoreEvent, EventTarget, PlayerSummary, RuntimeCommand, SessionCommand,
    TargetedEvent,
};

impl RuntimeServer {
    pub(in crate::runtime) async fn apply_runtime_command(
        &self,
        command: RuntimeCommand,
        session: Option<SessionRuntimeContext>,
    ) -> Result<(), RuntimeError> {
        let _consistency_guard = self.reload.read_consistency().await;
        self.apply_runtime_command_guarded(command, session).await
    }

    async fn apply_runtime_command_guarded(
        &self,
        command: RuntimeCommand,
        session: Option<SessionRuntimeContext>,
    ) -> Result<(), RuntimeError> {
        match command {
            RuntimeCommand::Core(command) => self.apply_command_guarded(command, session).await,
            RuntimeCommand::Session(command) => {
                self.apply_session_command_guarded(command, session).await
            }
        }
    }

    pub(in crate::runtime) async fn apply_command(
        &self,
        command: CoreCommand,
        session: Option<SessionRuntimeContext>,
    ) -> Result<(), RuntimeError> {
        let _consistency_guard = self.reload.read_consistency().await;
        self.apply_command_guarded(command, session).await
    }

    async fn apply_command_guarded(
        &self,
        command: CoreCommand,
        session: Option<SessionRuntimeContext>,
    ) -> Result<(), RuntimeError> {
        let session_capabilities = session
            .as_ref()
            .and_then(|session| session.session_capabilities.clone());
        let gameplay = session
            .as_ref()
            .and_then(|session| session.gameplay.clone());
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
        match events {
            KernelCommandOutcome::Events(events) => self.dispatch_events(events).await,
            KernelCommandOutcome::StaleGameplayCommand { player_id } => {
                let events = self.kernel.session_resync_events(player_id).await;
                self.dispatch_events(events).await;
            }
            KernelCommandOutcome::StaleLogin { connection_id } => {
                self.dispatch_events(vec![TargetedEvent {
                    target: EventTarget::Connection(connection_id),
                    event: CoreEvent::Disconnect {
                        reason:
                            "login state changed while processing your request; please try again"
                                .to_string(),
                    },
                }])
                .await;
            }
        }
        Ok(())
    }

    async fn apply_session_command_guarded(
        &self,
        command: SessionCommand,
        session: Option<SessionRuntimeContext>,
    ) -> Result<(), RuntimeError> {
        debug_assert!(
            session.is_some(),
            "session-only command reached runtime core loop without session context"
        );
        if let Some(session) = session {
            debug_assert!(
                session.player_id.is_none() || session.player_id == Some(command.player_id()),
                "session-only command player id did not match session player id"
            );
        }
        match command {
            SessionCommand::ClientStatus { .. }
            | SessionCommand::InventoryTransactionAck { .. } => Ok(()),
        }
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
        let now_ms = now_ms();
        let events = self.kernel.apply_builtin_tick(now_ms).await?;
        self.dispatch_events(events).await;
        for (player_id, session_capabilities, gameplay) in gameplay_sessions {
            if let Some(events) = self
                .kernel
                .apply_gameplay_tick(player_id, session_capabilities, gameplay, now_ms)
                .await?
            {
                self.dispatch_events(events).await;
            }
        }
        Ok(())
    }

    pub(in crate::runtime) async fn dispatch_events(&self, events: Vec<TargetedEvent>) {
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
                    .record_pending_login_route(*connection_id, *player_id)
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
                let _ = recipient
                    .control_tx
                    .send(SessionControl::Terminate {
                        reason: "server dropped the session because the outbound queue was full"
                            .to_string(),
                    })
                    .await;
            }
        }
    }

    pub(in crate::runtime) async fn unregister_session(
        &self,
        connection_id: mc_core::ConnectionId,
        shared_state: &SharedSessionState,
    ) -> Result<(), RuntimeError> {
        let _consistency_guard = self.reload.read_consistency().await;
        self.unregister_session_guarded(connection_id, shared_state)
            .await
    }

    async fn unregister_session_guarded(
        &self,
        connection_id: mc_core::ConnectionId,
        shared_state: &SharedSessionState,
    ) -> Result<(), RuntimeError> {
        let (view, context, adapter) = {
            let session = shared_state.read().await;
            (
                Self::session_view(&session),
                Self::session_runtime_context(&session),
                session.adapter.clone(),
            )
        };
        if let Some(adapter) = adapter.as_ref() {
            adapter
                .session_closed(&Self::protocol_session_snapshot(connection_id, &view))
                .map_err(|error| RuntimeError::Config(error.to_string()))?;
        }
        if let (Some(gameplay), Some(snapshot)) = (
            context.gameplay.as_ref(),
            Self::gameplay_session_snapshot(&view, &context),
        ) {
            gameplay.session_closed(&snapshot)?;
        }
        self.sessions.remove(connection_id).await;
        if let Some(player_id) = view.player_id {
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
