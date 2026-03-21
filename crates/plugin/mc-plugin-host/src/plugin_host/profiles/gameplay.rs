use super::{
    Arc, CapabilitySet, ConnectionPhase, GameplayEffect, GameplayJoinEffect,
    GameplayPolicyResolver, GameplayProfileHandle, GameplayProfileId, GameplayQuery,
    GameplayRequest, GameplayResponse, GameplaySessionSnapshot, PlayerId, PlayerSnapshot,
    PluginFailureAction, PluginFailureDispatch, PluginGenerationId, PluginKind,
    ReloadableGenerationSlot, RuntimeError, SessionCapabilitySet, with_gameplay_query,
};

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

    fn profile_id(&self) -> GameplayProfileId {
        self.profile_id.clone()
    }

    fn capability_set(&self) -> CapabilitySet {
        self.generation.capability_set()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Some(self.generation.generation_id())
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
            gameplay_profile: session.gameplay_profile.clone(),
        }
    }

    fn handle_runtime_failure<T: Default>(&self, message: String) -> Result<T, String> {
        match self
            .failures
            .handle_runtime_failure(PluginKind::Gameplay, &self.plugin_id, &message)
        {
            PluginFailureAction::Skip | PluginFailureAction::Quarantine => Ok(T::default()),
            PluginFailureAction::FailFast => Err(message),
        }
    }

    fn handle_hook<T>(
        &self,
        query: &dyn GameplayQuery,
        request: GameplayRequest,
        unexpected_payload: &'static str,
        map_response: impl FnOnce(GameplayResponse) -> Result<T, GameplayResponse>,
    ) -> Result<T, String>
    where
        T: Default,
    {
        self.generation.with_reload_read(|generation| {
            if self.failures.is_active_quarantined(&self.plugin_id) {
                return Ok(T::default());
            }
            with_gameplay_query(query, || match generation.invoke(&request) {
                Ok(response) => match map_response(response) {
                    Ok(value) => Ok(value),
                    Err(other) => {
                        self.handle_runtime_failure(format!("{unexpected_payload}: {other:?}"))
                    }
                },
                Err(error) => self.handle_runtime_failure(error),
            })
        })
    }

    fn session_closed(&self, session: &GameplaySessionSnapshot) -> Result<(), RuntimeError> {
        self.generation.with_reload_read(|generation| {
            match generation
                .invoke(&GameplayRequest::SessionClosed {
                    session: session.clone(),
                })
                .map_err(RuntimeError::Config)?
            {
                GameplayResponse::Empty => Ok(()),
                other => Err(RuntimeError::Config(format!(
                    "unexpected gameplay session_closed payload: {other:?}"
                ))),
            }
        })
    }
}

impl GameplayPolicyResolver for HotSwappableGameplayProfile {
    fn handle_player_join(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        player: &PlayerSnapshot,
    ) -> Result<GameplayJoinEffect, String> {
        self.handle_hook(
            query,
            GameplayRequest::HandlePlayerJoin {
                session: self.session_snapshot(ConnectionPhase::Login, session, Some(player.id)),
                player: player.clone(),
            },
            "unexpected gameplay join payload",
            |response| match response {
                GameplayResponse::JoinEffect(effect) => Ok(effect),
                other => Err(other),
            },
        )
    }

    fn handle_command(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        command: &mc_core::CoreCommand,
    ) -> Result<GameplayEffect, String> {
        self.handle_hook(
            query,
            GameplayRequest::HandleCommand {
                session: self.session_snapshot(ConnectionPhase::Play, session, command.player_id()),
                command: command.clone(),
            },
            "unexpected gameplay command payload",
            |response| match response {
                GameplayResponse::Effect(effect) => Ok(effect),
                other => Err(other),
            },
        )
    }

    fn handle_tick(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        player_id: PlayerId,
        now_ms: u64,
    ) -> Result<GameplayEffect, String> {
        self.handle_hook(
            query,
            GameplayRequest::HandleTick {
                session: self.session_snapshot(ConnectionPhase::Play, session, Some(player_id)),
                now_ms,
            },
            "unexpected gameplay tick payload",
            |response| match response {
                GameplayResponse::Effect(effect) => Ok(effect),
                other => Err(other),
            },
        )
    }
}

impl GameplayProfileHandle for HotSwappableGameplayProfile {
    fn profile_id(&self) -> GameplayProfileId {
        Self::profile_id(self)
    }

    fn capability_set(&self) -> CapabilitySet {
        Self::capability_set(self)
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Self::plugin_generation_id(self)
    }

    fn session_closed(&self, session: &GameplaySessionSnapshot) -> Result<(), RuntimeError> {
        Self::session_closed(self, session)
    }
}
