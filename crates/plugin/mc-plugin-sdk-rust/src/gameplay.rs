use super::*;

pub use crate::export_gameplay_plugin;

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
