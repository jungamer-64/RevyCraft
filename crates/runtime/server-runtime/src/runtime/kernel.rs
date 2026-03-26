use crate::RuntimeError;
use mc_core::{
    CoreCommand, CoreEvent, CoreRuntimeStateBlob, PlayerId, PlayerSummary, ServerCore,
    SessionCapabilitySet, TargetedEvent,
};
use mc_plugin_api::abi::PluginKind;
use mc_plugin_host::host::PluginFailureAction;
use mc_plugin_host::runtime::{GameplayProfileHandle, RuntimePluginHost, StorageProfileHandle};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

struct KernelState {
    core: ServerCore,
    dirty: bool,
}

pub(crate) struct ExportedCoreRuntimeState {
    pub(crate) blob: CoreRuntimeStateBlob,
    pub(crate) dirty: bool,
}

pub(crate) struct RuntimeKernel {
    storage_profile: Arc<dyn StorageProfileHandle>,
    world_dir: PathBuf,
    state: Mutex<KernelState>,
}

impl RuntimeKernel {
    pub(crate) fn new(
        core: ServerCore,
        storage_profile: Arc<dyn StorageProfileHandle>,
        world_dir: PathBuf,
    ) -> Self {
        Self {
            storage_profile,
            world_dir,
            state: Mutex::new(KernelState { core, dirty: false }),
        }
    }

    pub(crate) async fn apply_command(
        &self,
        command: CoreCommand,
        session_capabilities: Option<SessionCapabilitySet>,
        gameplay: Option<Arc<dyn GameplayProfileHandle>>,
        now_ms: u64,
    ) -> Result<Vec<TargetedEvent>, RuntimeError> {
        let should_persist = matches!(
            command,
            CoreCommand::LoginStart { .. }
                | CoreCommand::MoveIntent { .. }
                | CoreCommand::SetHeldSlot { .. }
                | CoreCommand::CreativeInventorySet { .. }
                | CoreCommand::InventoryClick { .. }
                | CoreCommand::CloseContainer { .. }
                | CoreCommand::DigBlock { .. }
                | CoreCommand::PlaceBlock { .. }
                | CoreCommand::UseBlock { .. }
                | CoreCommand::Disconnect { .. }
        );
        let mut state = self.state.lock().await;
        let events = match command {
            CoreCommand::LoginStart {
                connection_id,
                username,
                player_id,
            } => {
                if let (Some(session_capabilities), Some(gameplay)) =
                    (session_capabilities.as_ref(), gameplay.as_ref())
                {
                    gameplay
                        .handle_player_join(
                            &mut state.core,
                            session_capabilities,
                            connection_id,
                            username,
                            player_id,
                            now_ms,
                        )
                        .map_err(|error| RuntimeError::Config(error.to_string()))?
                } else {
                    state.core.apply_command(
                        CoreCommand::LoginStart {
                            connection_id,
                            username,
                            player_id,
                        },
                        now_ms,
                    )
                }
            }
            command => {
                if let Ok(gameplay_command) = command.clone().into_gameplay() {
                    if let (Some(session_capabilities), Some(gameplay)) =
                        (session_capabilities.as_ref(), gameplay.as_ref())
                    {
                        gameplay
                            .handle_command(
                                &mut state.core,
                                session_capabilities,
                                &gameplay_command,
                                now_ms,
                            )
                            .map_err(|error| RuntimeError::Config(error.to_string()))?
                    } else {
                        state
                            .core
                            .apply_builtin_gameplay_command(gameplay_command, now_ms)
                    }
                } else {
                    state.core.apply_command(command, now_ms)
                }
            }
        };
        if should_persist {
            state.dirty = true;
        }
        Ok(events)
    }

    #[cfg(test)]
    pub(crate) async fn open_crafting_table(
        &self,
        player_id: PlayerId,
        window_id: u8,
        title: &str,
    ) -> Vec<TargetedEvent> {
        self.state
            .lock()
            .await
            .core
            .open_crafting_table(player_id, window_id, title)
    }

    pub(crate) async fn tick(
        &self,
        gameplay_sessions: &[(
            PlayerId,
            SessionCapabilitySet,
            Arc<dyn GameplayProfileHandle>,
        )],
        now_ms: u64,
    ) -> Result<Vec<TargetedEvent>, RuntimeError> {
        let mut state = self.state.lock().await;
        let mut events = state.core.tick(now_ms);
        for (player_id, session_capabilities, gameplay) in gameplay_sessions {
            events.extend(
                gameplay
                    .handle_tick(&mut state.core, session_capabilities, *player_id, now_ms)
                    .map_err(|error| RuntimeError::Config(error.to_string()))?,
            );
        }
        if events
            .iter()
            .any(|event| !matches!(event.event, CoreEvent::KeepAliveRequested { .. }))
        {
            state.dirty = true;
        }
        Ok(events)
    }

    pub(crate) async fn snapshot(&self) -> mc_core::WorldSnapshot {
        self.state.lock().await.core.snapshot()
    }

    pub(crate) async fn export_core_runtime_state(&self) -> ExportedCoreRuntimeState {
        let state = self.state.lock().await;
        ExportedCoreRuntimeState {
            blob: state.core.export_runtime_state(),
            dirty: state.dirty,
        }
    }

    pub(crate) async fn swap_core(&self, candidate: ServerCore, dirty: bool) {
        let mut state = self.state.lock().await;
        state.core = candidate;
        state.dirty = dirty;
    }

    pub(crate) async fn player_summary(&self) -> PlayerSummary {
        self.state.lock().await.core.player_summary()
    }

    pub(crate) async fn dirty(&self) -> bool {
        self.state.lock().await.dirty
    }

    #[cfg(test)]
    pub(crate) async fn set_dirty(&self, dirty: bool) {
        self.state.lock().await.dirty = dirty;
    }

    pub(crate) async fn set_max_players(&self, max_players: u8) {
        self.state.lock().await.core.set_max_players(max_players);
    }

    pub(crate) fn world_dir(&self) -> &std::path::Path {
        &self.world_dir
    }

    pub(crate) async fn maybe_save(
        &self,
        reload_host: Option<&dyn RuntimePluginHost>,
    ) -> Result<(), RuntimeError> {
        let snapshot = {
            let state = self.state.lock().await;
            if !state.dirty {
                return Ok(());
            }
            state.core.snapshot()
        };
        match self
            .storage_profile
            .save_snapshot(&self.world_dir, &snapshot)
        {
            Ok(()) => {
                let mut state = self.state.lock().await;
                state.dirty = false;
                Ok(())
            }
            Err(mc_proto_common::StorageError::Plugin(message)) => {
                let action = reload_host.map_or(PluginFailureAction::FailFast, |reload_host| {
                    reload_host.handle_runtime_failure(
                        PluginKind::Storage,
                        self.storage_profile.plugin_id(),
                        &message,
                    )
                });
                let mut state = self.state.lock().await;
                state.dirty = true;
                match action {
                    PluginFailureAction::Skip => {
                        eprintln!(
                            "storage runtime failure for `{}` skipped: {message}",
                            self.storage_profile.plugin_id()
                        );
                        Ok(())
                    }
                    PluginFailureAction::FailFast => Err(RuntimeError::PluginFatal(format!(
                        "storage plugin `{}` failed during runtime: {message}",
                        self.storage_profile.plugin_id()
                    ))),
                    PluginFailureAction::Quarantine => Err(RuntimeError::Storage(
                        mc_proto_common::StorageError::Plugin(message),
                    )),
                }
            }
            Err(error) => Err(RuntimeError::Storage(error)),
        }
    }
}
