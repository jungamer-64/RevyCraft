#![allow(clippy::multiple_crate_versions)]
use mc_plugin_api::codec::gameplay::GameplaySessionSnapshot;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::gameplay::{self, GameplayHost, RustGameplayPlugin};
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;
use revy_voxel_core::{
    CoreEvent, EventTarget, GameplayCapability, GameplayCommand, PlayerId, TargetedEvent,
};

#[derive(Default)]
pub struct ReadonlyGameplayPlugin;

const HOTBAR_SLOT_COUNT: u8 = 9;

impl RustGameplayPlugin for ReadonlyGameplayPlugin {
    fn descriptor(&self) -> mc_plugin_api::codec::gameplay::GameplayDescriptor {
        gameplay::gameplay_descriptor("readonly")
    }

    fn capability_set(&self) -> revy_voxel_core::GameplayCapabilitySet {
        mc_plugin_sdk_rust::capabilities::gameplay_capabilities(&[
            GameplayCapability::RuntimeReload,
        ])
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
        host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        command: &GameplayCommand,
    ) -> Result<(), String> {
        match command {
            GameplayCommand::MoveIntent {
                player_id,
                position,
                yaw,
                pitch,
                on_ground,
            } => host.set_player_pose(*player_id, *position, *yaw, *pitch, *on_ground),
            GameplayCommand::SetHeldSlot { player_id, slot } => {
                set_held_slot(host, *player_id, *slot)
            }
            GameplayCommand::CreativeInventorySet { .. }
            | GameplayCommand::DigBlock { .. }
            | GameplayCommand::PlaceBlock { .. }
            | GameplayCommand::UseBlock { .. } => Ok(()),
        }
    }

    fn handle_tick(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _now_ms: u64,
    ) -> Result<(), String> {
        Ok(())
    }

    fn export_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
    ) -> Result<Vec<u8>, String> {
        Ok(option_env!("REVY_PLUGIN_BUILD_TAG")
            .unwrap_or("readonly")
            .as_bytes()
            .to_vec())
    }

    fn import_session_state(
        &self,
        _host: &dyn GameplayHost,
        _session: &GameplaySessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), String> {
        if mc_plugin_sdk_rust::capabilities::build_tag_contains("reload-fail") {
            return Err("readonly gameplay plugin refused session import".to_string());
        }
        Ok(())
    }
}

fn set_held_slot(host: &dyn GameplayHost, player_id: PlayerId, slot: i16) -> Result<(), String> {
    let Some(player) = host.read_player_snapshot(player_id)? else {
        return Ok(());
    };
    let Ok(slot) = u8::try_from(slot) else {
        return host.emit_event(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::SelectedHotbarSlotChanged {
                slot: player.selected_hotbar_slot,
            },
        });
    };
    if slot >= HOTBAR_SLOT_COUNT {
        return host.emit_event(TargetedEvent {
            target: EventTarget::Player(player_id),
            event: CoreEvent::SelectedHotbarSlotChanged {
                slot: player.selected_hotbar_slot,
            },
        });
    }
    host.clear_mining(player_id)?;
    host.set_selected_hotbar_slot(player_id, slot)
}

const MANIFEST: StaticPluginManifest =
    StaticPluginManifest::gameplay("gameplay-readonly", "Readonly Gameplay Plugin", "readonly");

export_plugin!(gameplay, ReadonlyGameplayPlugin, MANIFEST);
