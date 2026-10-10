use crate::RuntimeError;
use crate::runtime::{
    CoreCommandOutcome, CoreInvocation, CoreStore, PreparedGameplayTick, RuntimeServer,
    SessionCommandContext, SessionControl, SessionMessage, SessionPhase, now_ms,
};
use mc_plugin_contract::codec::gameplay::GameplaySessionSnapshot;
use revy_voxel_core::{
    CoreCommand, CoreEvent, EventTarget, PlayerSummary, RuntimeCommand, SessionCommand,
    TargetedEvent,
};
use std::collections::HashMap;
use std::sync::Arc;

const GAMEPLAY_TICK_PREPARE_FAN_OUT: usize = 16;

impl RuntimeServer {
    pub(in crate::runtime) async fn apply_runtime_command(
        &self,
        command: RuntimeCommand,
        session: Option<SessionCommandContext>,
    ) -> Result<(), RuntimeError> {
        match command {
            RuntimeCommand::Core(command) => self.apply_command(command, session).await,
            RuntimeCommand::Session(command) => self.apply_session_command(command, session),
        }
    }

    pub(in crate::runtime) async fn apply_command(
        &self,
        command: CoreCommand,
        session: Option<SessionCommandContext>,
    ) -> Result<(), RuntimeError> {
        let invocation = match session {
            None => CoreInvocation::Internal,
            Some(SessionCommandContext::Login {
                gameplay,
                capabilities,
            }) => CoreInvocation::Login {
                gameplay,
                capabilities,
            },
            Some(SessionCommandContext::Play {
                player_id,
                gameplay,
                capabilities,
            }) => CoreInvocation::Play {
                player_id,
                gameplay,
                capabilities,
            },
        };
        let core = self.authority.active().core.clone();
        match core.apply_command(command, invocation, now_ms()).await? {
            CoreCommandOutcome::Events(events) => self.dispatch_events(events).await,
            CoreCommandOutcome::StaleGameplayCommand { player_id } => {
                self.dispatch_events(
                    core.session_resync_events(player_id)
                        .await
                        .into_iter()
                        .map(crate::runtime::SharedCoreEvent::from)
                        .collect(),
                )
                .await;
            }
            CoreCommandOutcome::StaleLogin { connection_id } => {
                self.dispatch_events(vec![crate::runtime::SharedCoreEvent::from(TargetedEvent {
                    target: EventTarget::Connection(connection_id),
                    event: CoreEvent::Disconnect {
                        reason:
                            "login state changed while processing your request; please try again"
                                .to_string(),
                    },
                })])
                .await;
            }
        }
        Ok(())
    }

    fn apply_session_command(
        &self,
        command: SessionCommand,
        session: Option<SessionCommandContext>,
    ) -> Result<(), RuntimeError> {
        let Some(SessionCommandContext::Play { player_id, .. }) = session else {
            return Err(RuntimeError::Config(
                "session-only command requires a play-session capability".to_string(),
            ));
        };
        if player_id != command.player_id() {
            return Err(RuntimeError::Config(
                "session-only command player did not match its play-session capability".to_string(),
            ));
        }
        match command {
            SessionCommand::ClientStatus { .. }
            | SessionCommand::InventoryTransactionAck { .. } => Ok(()),
        }
    }

    pub(in crate::runtime) async fn tick(&self) -> Result<(), RuntimeError> {
        let _data_plane = self.authority.enter_data_plane().await;
        let gameplay_sessions = self.sessions.gameplay_tick_subscribers().await;
        let now_ms = now_ms();
        let core = self.authority.active().core.clone();
        self.dispatch_events(core.apply_builtin_tick(now_ms).await?)
            .await;
        let snapshot = core.version();
        let prepared = prepare_gameplay_ticks(snapshot, gameplay_sessions, now_ms).await?;
        let outcome = core.commit_gameplay_ticks(prepared).await?;
        for stale in outcome.stale {
            eprintln!(
                "gameplay tick for player {:?} was stale: source core revision {}, active revision {}",
                stale.player_id,
                stale.source_revision.value(),
                stale.active_revision.value()
            );
        }
        self.dispatch_events(outcome.events).await;
        Ok(())
    }

    pub(in crate::runtime) async fn dispatch_events(
        &self,
        events: Vec<crate::runtime::SharedCoreEvent>,
    ) {
        let mut batches = HashMap::new();
        for event in events {
            let recipients = self.sessions.recipients_for_target(event.target).await;
            let payload = event.event;
            for recipient in recipients {
                let (_, batch): &mut (_, Vec<_>) = batches
                    .entry(recipient.connection_id)
                    .or_insert_with(|| (recipient, Vec::new()));
                batch.push(std::sync::Arc::clone(&payload));
            }
        }
        for (_, (recipient, batch)) in batches {
            if recipient
                .tx
                .try_send(SessionMessage::Events(batch))
                .is_err()
            {
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
        connection_id: revy_voxel_core::ConnectionId,
        session: &SessionPhase,
    ) -> Result<(), RuntimeError> {
        let _data_plane = self.authority.enter_data_plane().await;
        if let Some(binding) = session.binding() {
            binding
                .adapter
                .session_closed(&session.protocol_snapshot(connection_id))
                .map_err(|error| RuntimeError::Config(error.to_string()))?;
        }
        if let SessionPhase::Play(play) = session {
            play.binding
                .gameplay
                .session_closed(&GameplaySessionSnapshot {
                    phase: mc_proto_common::ConnectionPhase::Play,
                    player_id: Some(play.player_id),
                    entity_id: Some(play.entity_id),
                    protocol: play.binding.capabilities.protocol.clone(),
                    gameplay_profile: play.binding.capabilities.gameplay_profile.clone(),
                    protocol_generation: play.binding.capabilities.protocol_generation,
                    gameplay_generation: play.binding.capabilities.gameplay_generation,
                })?;
        }
        self.sessions.remove(connection_id).await;
        if let SessionPhase::Play(play) = session {
            self.apply_command(
                CoreCommand::Disconnect {
                    player_id: play.player_id,
                },
                None,
            )
            .await?;
        }
        let _ = self
            .topology_resources
            .retire_drained_generations(&self.sessions)
            .await;
        Ok(())
    }

    pub(in crate::runtime) async fn player_summary(&self) -> PlayerSummary {
        self.authority.active().core.player_summary().await
    }
}

async fn prepare_gameplay_ticks(
    snapshot: Arc<revy_voxel_core::CoreVersion>,
    sessions: Vec<(
        revy_voxel_core::PlayerId,
        revy_voxel_core::SessionCapabilitySet,
        Arc<dyn mc_plugin_host::runtime::GameplayProfileHandle>,
    )>,
    now_ms: u64,
) -> Result<Vec<PreparedGameplayTick>, RuntimeError> {
    if sessions.is_empty() {
        return Ok(Vec::new());
    }
    let chunk_size = sessions.len().div_ceil(GAMEPLAY_TICK_PREPARE_FAN_OUT);
    let mut sessions = sessions.into_iter();
    let mut tasks = Vec::new();
    loop {
        let chunk = sessions.by_ref().take(chunk_size).collect::<Vec<_>>();
        if chunk.is_empty() {
            break;
        }
        let snapshot = Arc::clone(&snapshot);
        tasks.push(tokio::task::spawn_blocking(move || {
            chunk
                .into_iter()
                .map(|(player_id, capabilities, gameplay)| {
                    CoreStore::prepare_gameplay_tick(
                        Arc::clone(&snapshot),
                        player_id,
                        capabilities,
                        gameplay,
                        now_ms,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
        }));
    }

    let mut prepared = Vec::new();
    let mut failure = None;
    for task in tasks {
        match task.await {
            Ok(Ok(mut chunk)) => prepared.append(&mut chunk),
            Ok(Err(error)) => {
                failure.get_or_insert(error);
            }
            Err(error) => {
                failure.get_or_insert_with(|| {
                    RuntimeError::Config(format!("gameplay tick preparation task failed: {error}"))
                });
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(prepared)
}
