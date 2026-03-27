use super::*;
use mc_core::{GameplayCapabilitySet, GameplayCommand, GameplayProfileId};

pub trait GameplayHost {
    fn log(&self, level: u32, message: &str) -> Result<(), String>;

    fn read_player_snapshot(&self, player_id: PlayerId) -> Result<Option<PlayerSnapshot>, String>;

    fn read_world_meta(&self) -> Result<WorldMeta, String>;

    fn read_block_state(&self, position: mc_core::BlockPos) -> Result<mc_core::BlockState, String>;

    fn read_block_entity(
        &self,
        position: mc_core::BlockPos,
    ) -> Result<Option<mc_core::BlockEntityState>, String>;

    fn can_edit_block(
        &self,
        player_id: PlayerId,
        position: mc_core::BlockPos,
    ) -> Result<bool, String>;

    fn set_player_pose(
        &self,
        player_id: PlayerId,
        position: Option<mc_core::Vec3>,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    ) -> Result<(), String>;

    fn set_selected_hotbar_slot(&self, player_id: PlayerId, slot: u8) -> Result<(), String>;

    fn set_inventory_slot(
        &self,
        player_id: PlayerId,
        slot: mc_core::InventorySlot,
        stack: Option<mc_core::ItemStack>,
    ) -> Result<(), String>;

    fn clear_mining(&self, player_id: PlayerId) -> Result<(), String>;

    fn begin_mining(
        &self,
        player_id: PlayerId,
        position: mc_core::BlockPos,
        duration_ms: u64,
    ) -> Result<(), String>;

    fn open_chest(&self, player_id: PlayerId, position: mc_core::BlockPos) -> Result<(), String>;

    fn open_furnace(&self, player_id: PlayerId, position: mc_core::BlockPos) -> Result<(), String>;

    fn open_crafting_table(&self, player_id: PlayerId) -> Result<(), String>;

    fn set_block(
        &self,
        position: mc_core::BlockPos,
        block: mc_core::BlockState,
    ) -> Result<(), String>;

    fn spawn_dropped_item(
        &self,
        position: mc_core::Vec3,
        item: mc_core::ItemStack,
    ) -> Result<(), String>;

    fn emit_event(&self, event: mc_core::TargetedEvent) -> Result<(), String>;
}

pub trait RustGameplayPlugin: Send + Sync + 'static {
    fn descriptor(&self) -> GameplayDescriptor;

    fn capability_set(&self) -> GameplayCapabilitySet {
        GameplayCapabilitySet::default()
    }

    fn handle_player_join(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _player_id: PlayerId,
    ) -> Result<(), String> {
        Ok(())
    }

    fn handle_command(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _command: &GameplayCommand,
    ) -> Result<(), String> {
        Ok(())
    }

    fn handle_tick(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _now_ms: u64,
    ) -> Result<(), String> {
        Ok(())
    }

    fn session_closed(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
    ) -> Result<(), String> {
        Ok(())
    }

    fn export_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
    ) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    fn import_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), String> {
        Ok(())
    }
}

#[must_use]
pub fn gameplay_descriptor(profile: impl Into<String>) -> GameplayDescriptor {
    GameplayDescriptor {
        profile: GameplayProfileId::new(profile.into()),
    }
}
