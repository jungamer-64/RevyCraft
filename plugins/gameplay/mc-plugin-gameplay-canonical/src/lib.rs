#![allow(clippy::multiple_crate_versions)]
use mc_core::{
    CanonicalGameplayPolicy, CapabilitySet, GameplayPolicyResolver, GameplayProfileId,
    GameplayQuery, PlayerId, PlayerSnapshot, SessionCapabilitySet,
};
use mc_plugin_api::codec::gameplay::{GameplayDescriptor, GameplaySessionSnapshot};
use mc_plugin_sdk_rust::gameplay::{GameplayHost, RustGameplayPlugin, export_gameplay_plugin};
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;

#[derive(Default)]
pub struct CanonicalGameplayPlugin;

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

impl RustGameplayPlugin for CanonicalGameplayPlugin {
    fn descriptor(&self) -> GameplayDescriptor {
        GameplayDescriptor {
            profile: GameplayProfileId::new("canonical"),
        }
    }

    fn capability_set(&self) -> CapabilitySet {
        let mut capabilities = CapabilitySet::new();
        let _ = capabilities.insert("gameplay.profile.canonical");
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
        CanonicalGameplayPolicy.handle_player_join(
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
        CanonicalGameplayPolicy.handle_command(
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
        CanonicalGameplayPolicy.handle_tick(
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
            .unwrap_or("canonical")
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
            return Err("canonical gameplay plugin refused session import".to_string());
        }
        Ok(())
    }
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::gameplay(
    "gameplay-canonical",
    "Canonical Gameplay Plugin",
    &["gameplay.profile:canonical", "runtime.reload.gameplay"],
);

export_gameplay_plugin!(CanonicalGameplayPlugin, MANIFEST);
