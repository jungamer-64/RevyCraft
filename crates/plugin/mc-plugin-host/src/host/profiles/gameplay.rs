use super::{
    Arc, ConnectionPhase, GameplayCapabilitySet, GameplayCommand, GameplayProfileHandle,
    GameplayProfileId, GameplayRequest, GameplayResponse, GameplaySessionSnapshot, PlayerId,
    PluginFailureAction, PluginFailureDispatch, PluginGenerationId, PluginKind,
    ReloadableGenerationSlot, ServerCore, SessionCapabilitySet,
    with_gameplay_transaction_and_limits,
};
use crate::PluginHostError;
use mc_core::{ConnectionId, GameplayJournal, GameplayTransaction};

pub(crate) struct HotSwappableGameplayProfile {
    plugin_id: String,
    profile_id: GameplayProfileId,
    generation: ReloadableGenerationSlot<super::GameplayGeneration>,
    failures: Arc<PluginFailureDispatch>,
}

impl HotSwappableGameplayProfile {
    pub(crate) const fn new(
        plugin_id: String,
        profile_id: GameplayProfileId,
        generation: Arc<super::GameplayGeneration>,
        failures: Arc<PluginFailureDispatch>,
    ) -> Self {
        Self {
            plugin_id,
            profile_id,
            generation: ReloadableGenerationSlot::new(
                generation,
                "gameplay generation lock should not be poisoned",
                "gameplay reload gate should not be poisoned",
            ),
            failures,
        }
    }

    pub(crate) fn current_generation(&self) -> Arc<super::GameplayGeneration> {
        self.generation.current()
    }

    pub(crate) fn swap_generation(&self, generation: Arc<super::GameplayGeneration>) {
        self.generation.swap(generation);
    }

    pub(crate) fn with_reload_write<T>(
        &self,
        f: impl FnOnce(Arc<super::GameplayGeneration>) -> T,
    ) -> T {
        self.generation.with_reload_write(f)
    }

    fn session_snapshot(
        &self,
        phase: ConnectionPhase,
        session: &SessionCapabilitySet,
        player_id: Option<PlayerId>,
    ) -> GameplaySessionSnapshot {
        GameplaySessionSnapshot {
            phase,
            player_id,
            entity_id: session.entity_id,
            protocol: session.protocol.clone(),
            gameplay_profile: session.gameplay_profile.clone(),
            protocol_generation: session.protocol_generation,
            gameplay_generation: session.gameplay_generation,
        }
    }

    fn handle_runtime_failure<T: Default>(&self, message: String) -> Result<T, PluginHostError> {
        match self
            .failures
            .handle_runtime_failure(PluginKind::Gameplay, &self.plugin_id, &message)
        {
            PluginFailureAction::Skip | PluginFailureAction::Quarantine => Ok(T::default()),
            PluginFailureAction::FailFast => Err(PluginHostError::Config(message)),
        }
    }

    fn prepare_request(
        &self,
        core: ServerCore,
        now_ms: u64,
        request: GameplayRequest,
    ) -> Result<GameplayJournal, PluginHostError> {
        self.generation.with_reload_read(|generation| {
            if self.failures.is_active_quarantined(&self.plugin_id) {
                return Ok(GameplayJournal::empty(now_ms));
            }

            let mut tx = GameplayTransaction::detached(core, now_ms);
            let response =
                with_gameplay_transaction_and_limits(&mut tx, generation.buffer_limits, || {
                    generation.invoke(&request)
                });
            match response {
                Ok(GameplayResponse::Empty) => Ok(tx.into_journal()),
                Ok(other) => {
                    self.handle_runtime_failure::<()>(format!(
                        "unexpected gameplay response payload: {other:?}"
                    ))?;
                    Ok(GameplayJournal::empty(now_ms))
                }
                Err(error) => {
                    self.handle_runtime_failure::<()>(error.to_string())?;
                    Ok(GameplayJournal::empty(now_ms))
                }
            }
        })
    }

    fn session_closed(&self, session: &GameplaySessionSnapshot) -> Result<(), PluginHostError> {
        self.generation.with_reload_read(|generation| {
            match generation
                .invoke(&GameplayRequest::SessionClosed {
                    session: session.clone(),
                })
                .map_err(PluginHostError::Config)?
            {
                GameplayResponse::Empty => Ok(()),
                other => Err(PluginHostError::Config(format!(
                    "unexpected gameplay session_closed payload: {other:?}"
                ))),
            }
        })
    }
}

impl GameplayProfileHandle for HotSwappableGameplayProfile {
    fn profile_id(&self) -> GameplayProfileId {
        self.profile_id.clone()
    }

    fn capability_set(&self) -> GameplayCapabilitySet {
        self.generation.capability_set()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Some(self.generation.generation_id())
    }

    fn prepare_player_join(
        &self,
        core: ServerCore,
        session: &SessionCapabilitySet,
        connection_id: ConnectionId,
        username: String,
        player_id: PlayerId,
        now_ms: u64,
    ) -> Result<GameplayJournal, PluginHostError> {
        let mut tx = GameplayTransaction::detached(core, now_ms);
        if let Some(rejection) = tx
            .begin_login(connection_id, username, player_id)
            .map_err(PluginHostError::Config)?
        {
            for event in rejection {
                tx.emit_event(event.target, event.event);
            }
            return Ok(tx.into_journal());
        }
        let request = GameplayRequest::HandlePlayerJoin {
            session: self.session_snapshot(ConnectionPhase::Login, session, Some(player_id)),
            player_id,
        };
        self.generation.with_reload_read(|generation| {
            if self.failures.is_active_quarantined(&self.plugin_id) {
                tx.finalize_login(connection_id, player_id)
                    .map_err(PluginHostError::Config)?;
                return Ok(tx.into_journal());
            }
            let response =
                with_gameplay_transaction_and_limits(&mut tx, generation.buffer_limits, || {
                    generation.invoke(&request)
                });
            match response {
                Ok(GameplayResponse::Empty) => {
                    tx.finalize_login(connection_id, player_id)
                        .map_err(PluginHostError::Config)?;
                    Ok(tx.into_journal())
                }
                Ok(other) => {
                    self.handle_runtime_failure::<()>(format!(
                        "unexpected gameplay join payload: {other:?}"
                    ))?;
                    tx.finalize_login(connection_id, player_id)
                        .map_err(PluginHostError::Config)?;
                    Ok(tx.into_journal())
                }
                Err(error) => {
                    self.handle_runtime_failure::<()>(error.to_string())?;
                    tx.finalize_login(connection_id, player_id)
                        .map_err(PluginHostError::Config)?;
                    Ok(tx.into_journal())
                }
            }
        })
    }

    fn prepare_command(
        &self,
        core: ServerCore,
        session: &SessionCapabilitySet,
        command: &GameplayCommand,
        now_ms: u64,
    ) -> Result<GameplayJournal, PluginHostError> {
        let request = GameplayRequest::HandleCommand {
            session: self.session_snapshot(
                ConnectionPhase::Play,
                session,
                Some(command.player_id()),
            ),
            command: command.clone(),
        };
        self.prepare_request(core, now_ms, request)
    }

    fn prepare_tick(
        &self,
        core: ServerCore,
        session: &SessionCapabilitySet,
        player_id: PlayerId,
        now_ms: u64,
    ) -> Result<GameplayJournal, PluginHostError> {
        let request = GameplayRequest::HandleTick {
            session: self.session_snapshot(ConnectionPhase::Play, session, Some(player_id)),
            now_ms,
        };
        self.prepare_request(core, now_ms, request)
    }

    fn session_closed(&self, session: &GameplaySessionSnapshot) -> Result<(), PluginHostError> {
        Self::session_closed(self, session)
    }

    fn export_session_state(
        &self,
        session: &GameplaySessionSnapshot,
    ) -> Result<Vec<u8>, PluginHostError> {
        self.generation.with_reload_read(|generation| {
            match generation
                .invoke(&GameplayRequest::ExportSessionState {
                    session: session.clone(),
                })
                .map_err(PluginHostError::Config)?
            {
                GameplayResponse::SessionTransferBlob(blob) => Ok(blob),
                other => Err(PluginHostError::Config(format!(
                    "unexpected gameplay export_session_state payload: {other:?}"
                ))),
            }
        })
    }

    fn import_session_state(
        &self,
        session: &GameplaySessionSnapshot,
        blob: &[u8],
    ) -> Result<(), PluginHostError> {
        self.generation.with_reload_read(|generation| {
            match generation
                .invoke(&GameplayRequest::ImportSessionState {
                    session: session.clone(),
                    blob: blob.to_vec(),
                })
                .map_err(PluginHostError::Config)?
            {
                GameplayResponse::Empty => Ok(()),
                other => Err(PluginHostError::Config(format!(
                    "unexpected gameplay import_session_state payload: {other:?}"
                ))),
            }
        })
    }
}
