use super::*;
use mc_core::{GameplayPolicyResolver, GameplayProfileId, GameplayQuery, SessionCapabilitySet};

struct HostQuery<'a> {
    host: &'a dyn GameplayHost,
}

impl GameplayQuery for HostQuery<'_> {
    fn world_meta(&self) -> WorldMeta {
        self.host
            .read_world_meta()
            .expect("gameplay host should provide world meta")
    }

    fn player_snapshot(&self, player_id: PlayerId) -> Option<PlayerSnapshot> {
        self.host
            .read_player_snapshot(player_id)
            .expect("gameplay host should provide player snapshots")
    }

    fn block_state(&self, position: mc_core::BlockPos) -> mc_core::BlockState {
        self.host
            .read_block_state(position)
            .expect("gameplay host should provide block states")
    }

    fn can_edit_block(&self, player_id: PlayerId, position: mc_core::BlockPos) -> bool {
        self.host
            .can_edit_block(player_id, position)
            .expect("gameplay host should provide can_edit_block")
    }
}

fn session_capabilities(
    plugin_capabilities: &CapabilitySet,
    session: &GameplaySessionSnapshot,
) -> SessionCapabilitySet {
    SessionCapabilitySet {
        protocol: CapabilitySet::new(),
        gameplay: plugin_capabilities.clone(),
        gameplay_profile: session.gameplay_profile.clone(),
        entity_id: session.entity_id,
        protocol_generation: None,
        gameplay_generation: None,
    }
}

pub trait GameplayHost {
    /// Writes a diagnostic message through the host runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the host rejects or cannot persist the log entry.
    fn log(&self, level: u32, message: &str) -> Result<(), String>;

    /// Reads the latest snapshot for the given player.
    ///
    /// # Errors
    ///
    /// Returns an error when the host query fails.
    fn read_player_snapshot(&self, player_id: PlayerId) -> Result<Option<PlayerSnapshot>, String>;

    /// Reads world metadata from the host runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the host query fails.
    fn read_world_meta(&self) -> Result<WorldMeta, String>;

    /// Reads the current block state at the given position.
    ///
    /// # Errors
    ///
    /// Returns an error when the host query fails.
    fn read_block_state(&self, position: mc_core::BlockPos) -> Result<mc_core::BlockState, String>;

    /// Checks whether the given player is allowed to edit the given block.
    ///
    /// # Errors
    ///
    /// Returns an error when the host query fails.
    fn can_edit_block(
        &self,
        player_id: PlayerId,
        position: mc_core::BlockPos,
    ) -> Result<bool, String>;
}

pub trait RustGameplayPlugin: Send + Sync + 'static {
    fn descriptor(&self) -> GameplayDescriptor;

    fn capability_set(&self) -> CapabilitySet {
        CapabilitySet::new()
    }

    /// Handles a player joining the gameplay session.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot produce join-side effects.
    fn handle_player_join(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _player: &PlayerSnapshot,
    ) -> Result<GameplayJoinEffect, String> {
        Ok(GameplayJoinEffect::default())
    }

    /// Handles a gameplay command emitted by the runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot produce command-side effects.
    fn handle_command(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _command: &CoreCommand,
    ) -> Result<GameplayEffect, String> {
        Ok(GameplayEffect::default())
    }

    /// Handles a gameplay tick for the current session.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot produce tick-side effects.
    fn handle_tick(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _now_ms: u64,
    ) -> Result<GameplayEffect, String> {
        Ok(GameplayEffect::default())
    }

    /// Notifies the plugin that the gameplay session has been closed.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot clean up its session state.
    fn session_closed(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Exports plugin-specific gameplay session state into an opaque blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot serialize its session state.
    fn export_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
    ) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    /// Imports plugin-specific gameplay session state from an opaque blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the provided blob is invalid for the current plugin.
    fn import_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), String> {
        Ok(())
    }
}

pub trait PolicyGameplayPlugin: Send + Sync + 'static {
    type Policy: GameplayPolicyResolver + Default;

    const PROFILE_ID: &'static str;
    const EXPORT_TAG: &'static str;
    const IMPORT_REJECT_MESSAGE: &'static str;

    fn capability_names() -> &'static [&'static str];
}

impl<T> RustGameplayPlugin for T
where
    T: PolicyGameplayPlugin,
{
    fn descriptor(&self) -> GameplayDescriptor {
        GameplayDescriptor {
            profile: GameplayProfileId::new(T::PROFILE_ID),
        }
    }

    fn capability_set(&self) -> CapabilitySet {
        capabilities::capability_set(T::capability_names())
    }

    fn handle_player_join(
        &self,
        host: &dyn GameplayHost,
        session: &GameplaySessionSnapshot,
        player: &PlayerSnapshot,
    ) -> Result<GameplayJoinEffect, String> {
        let query = HostQuery { host };
        let capabilities = capabilities::capability_set(T::capability_names());
        T::Policy::default().handle_player_join(
            &query,
            &session_capabilities(&capabilities, session),
            player,
        )
    }

    fn handle_command(
        &self,
        host: &dyn GameplayHost,
        session: &GameplaySessionSnapshot,
        command: &CoreCommand,
    ) -> Result<GameplayEffect, String> {
        let query = HostQuery { host };
        let capabilities = capabilities::capability_set(T::capability_names());
        T::Policy::default().handle_command(
            &query,
            &session_capabilities(&capabilities, session),
            command,
        )
    }

    fn handle_tick(
        &self,
        host: &dyn GameplayHost,
        session: &GameplaySessionSnapshot,
        now_ms: u64,
    ) -> Result<GameplayEffect, String> {
        let query = HostQuery { host };
        let capabilities = capabilities::capability_set(T::capability_names());
        let Some(player_id) = session.player_id else {
            return Ok(GameplayEffect::default());
        };
        T::Policy::default().handle_tick(
            &query,
            &session_capabilities(&capabilities, session),
            player_id,
            now_ms,
        )
    }

    fn export_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
    ) -> Result<Vec<u8>, String> {
        Ok(option_env!("REVY_PLUGIN_BUILD_TAG")
            .unwrap_or(T::EXPORT_TAG)
            .as_bytes()
            .to_vec())
    }

    fn import_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), String> {
        if capabilities::build_tag_contains("reload-fail") {
            return Err(T::IMPORT_REJECT_MESSAGE.to_string());
        }
        Ok(())
    }
}
