#![allow(clippy::multiple_crate_versions)]
use mc_core::{
    CapabilitySet, GameplayPolicyResolver, GameplayProfileId, GameplayQuery, PlayerId,
    PlayerSnapshot, ReadonlyGameplayPolicy, SessionCapabilitySet,
};
use mc_plugin_api::{GameplayDescriptor, GameplaySessionSnapshot};
use mc_plugin_sdk_rust::{
    GameplayHost, RustGameplayPlugin, StaticPluginManifest, export_gameplay_plugin,
};

#[derive(Default)]
pub struct ReadonlyGameplayPlugin;

struct HostQuery<'a> {
    host: &'a dyn GameplayHost,
}

impl GameplayQuery for HostQuery<'_> {
    fn world_meta(&self) -> mc_core::WorldMeta {
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

impl RustGameplayPlugin for ReadonlyGameplayPlugin {
    fn descriptor(&self) -> GameplayDescriptor {
        GameplayDescriptor {
            profile: GameplayProfileId::new("readonly"),
        }
    }

    fn capability_set(&self) -> CapabilitySet {
        let mut capabilities = CapabilitySet::new();
        let _ = capabilities.insert("gameplay.profile.readonly");
        let _ = capabilities.insert("runtime.reload.gameplay");
        if let Some(build_tag) = option_env!("REVY_PLUGIN_BUILD_TAG") {
            let _ = capabilities.insert(format!("build-tag:{build_tag}"));
        }
        capabilities
    }

    fn handle_player_join(
        &self,
        host: &dyn GameplayHost,
        session: &GameplaySessionSnapshot,
        player: &PlayerSnapshot,
    ) -> Result<mc_core::GameplayJoinEffect, String> {
        let query = HostQuery { host };
        let capabilities = self.capability_set();
        ReadonlyGameplayPolicy.handle_player_join(
            &query,
            &session_capabilities(&capabilities, session),
            player,
        )
    }

    fn handle_command(
        &self,
        host: &dyn GameplayHost,
        session: &GameplaySessionSnapshot,
        command: &mc_core::CoreCommand,
    ) -> Result<mc_core::GameplayEffect, String> {
        let query = HostQuery { host };
        let capabilities = self.capability_set();
        ReadonlyGameplayPolicy.handle_command(
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
    ) -> Result<mc_core::GameplayEffect, String> {
        let query = HostQuery { host };
        let capabilities = self.capability_set();
        let Some(player_id) = session.player_id else {
            return Ok(mc_core::GameplayEffect::default());
        };
        ReadonlyGameplayPolicy.handle_tick(
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
        if option_env!("REVY_PLUGIN_BUILD_TAG").is_some_and(|tag| tag.contains("reload-fail")) {
            return Err("readonly gameplay plugin refused session import".to_string());
        }
        Ok(())
    }
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::gameplay(
    "gameplay-readonly",
    "Readonly Gameplay Plugin",
    &["gameplay.profile:readonly", "runtime.reload.gameplay"],
);

export_gameplay_plugin!(ReadonlyGameplayPlugin, MANIFEST);
