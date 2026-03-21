use super::{
    Arc, CapabilitySet, ConnectionPhase, GameplayEffect, GameplayJoinEffect,
    GameplayPolicyResolver, GameplayProfileHandle, GameplayProfileId, GameplayQuery,
    GameplayRequest, GameplayResponse, GameplaySessionSnapshot, PlayerId, PlayerSnapshot,
    PluginFailureAction, PluginFailureDispatch, PluginGenerationId, PluginKind, RuntimeError,
    RwLock, SessionCapabilitySet, with_gameplay_query,
};

pub(crate) struct HotSwappableGameplayProfile {
    plugin_id: String,
    profile_id: GameplayProfileId,
    pub(crate) generation: RwLock<Arc<super::GameplayGeneration>>,
    failures: Arc<PluginFailureDispatch>,
    pub(crate) reload_gate: RwLock<()>,
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
            generation: RwLock::new(generation),
            failures,
            reload_gate: RwLock::new(()),
        }
    }

    pub(crate) fn current_generation(&self) -> Arc<super::GameplayGeneration> {
        self.generation
            .read()
            .expect("gameplay generation lock should not be poisoned")
            .clone()
    }

    pub(crate) fn swap_generation(&self, generation: Arc<super::GameplayGeneration>) {
        *self
            .generation
            .write()
            .expect("gameplay generation lock should not be poisoned") = generation;
    }

    fn profile_id(&self) -> GameplayProfileId {
        self.profile_id.clone()
    }

    fn capability_set(&self) -> CapabilitySet {
        self.current_generation().capabilities.clone()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Some(self.current_generation().generation_id)
    }

    fn session_closed(&self, session: &GameplaySessionSnapshot) -> Result<(), RuntimeError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        let generation = self.current_generation();
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
    }
}

impl GameplayPolicyResolver for HotSwappableGameplayProfile {
    fn handle_player_join(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        player: &PlayerSnapshot,
    ) -> Result<GameplayJoinEffect, String> {
        let session = GameplaySessionSnapshot {
            phase: ConnectionPhase::Login,
            player_id: Some(player.id),
            entity_id: session.entity_id,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        if self.failures.is_active_quarantined(&self.plugin_id) {
            return Ok(GameplayJoinEffect::default());
        }
        let generation = self.current_generation();
        with_gameplay_query(query, || {
            match generation.invoke(&GameplayRequest::HandlePlayerJoin {
                session,
                player: player.clone(),
            }) {
                Ok(GameplayResponse::JoinEffect(effect)) => Ok(effect),
                Ok(other) => {
                    let message = format!("unexpected gameplay join payload: {other:?}");
                    match self.failures.handle_runtime_failure(
                        PluginKind::Gameplay,
                        &self.plugin_id,
                        &message,
                    ) {
                        PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                            Ok(GameplayJoinEffect::default())
                        }
                        PluginFailureAction::FailFast => Err(message),
                    }
                }
                Err(error) => match self.failures.handle_runtime_failure(
                    PluginKind::Gameplay,
                    &self.plugin_id,
                    &error,
                ) {
                    PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                        Ok(GameplayJoinEffect::default())
                    }
                    PluginFailureAction::FailFast => Err(error.to_string()),
                },
            }
        })
    }

    fn handle_command(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        command: &mc_core::CoreCommand,
    ) -> Result<GameplayEffect, String> {
        let session = GameplaySessionSnapshot {
            phase: ConnectionPhase::Play,
            player_id: command.player_id(),
            entity_id: session.entity_id,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        if self.failures.is_active_quarantined(&self.plugin_id) {
            return Ok(GameplayEffect::default());
        }
        let generation = self.current_generation();
        with_gameplay_query(query, || {
            match generation.invoke(&GameplayRequest::HandleCommand {
                session,
                command: command.clone(),
            }) {
                Ok(GameplayResponse::Effect(effect)) => Ok(effect),
                Ok(other) => {
                    let message = format!("unexpected gameplay command payload: {other:?}");
                    match self.failures.handle_runtime_failure(
                        PluginKind::Gameplay,
                        &self.plugin_id,
                        &message,
                    ) {
                        PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                            Ok(GameplayEffect::default())
                        }
                        PluginFailureAction::FailFast => Err(message),
                    }
                }
                Err(error) => match self.failures.handle_runtime_failure(
                    PluginKind::Gameplay,
                    &self.plugin_id,
                    &error,
                ) {
                    PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                        Ok(GameplayEffect::default())
                    }
                    PluginFailureAction::FailFast => Err(error.to_string()),
                },
            }
        })
    }

    fn handle_tick(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        player_id: PlayerId,
        now_ms: u64,
    ) -> Result<GameplayEffect, String> {
        let session = GameplaySessionSnapshot {
            phase: ConnectionPhase::Play,
            player_id: Some(player_id),
            entity_id: session.entity_id,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        if self.failures.is_active_quarantined(&self.plugin_id) {
            return Ok(GameplayEffect::default());
        }
        let generation = self.current_generation();
        with_gameplay_query(query, || {
            match generation.invoke(&GameplayRequest::HandleTick { session, now_ms }) {
                Ok(GameplayResponse::Effect(effect)) => Ok(effect),
                Ok(other) => {
                    let message = format!("unexpected gameplay tick payload: {other:?}");
                    match self.failures.handle_runtime_failure(
                        PluginKind::Gameplay,
                        &self.plugin_id,
                        &message,
                    ) {
                        PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                            Ok(GameplayEffect::default())
                        }
                        PluginFailureAction::FailFast => Err(message),
                    }
                }
                Err(error) => match self.failures.handle_runtime_failure(
                    PluginKind::Gameplay,
                    &self.plugin_id,
                    &error,
                ) {
                    PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                        Ok(GameplayEffect::default())
                    }
                    PluginFailureAction::FailFast => Err(error.to_string()),
                },
            }
        })
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
