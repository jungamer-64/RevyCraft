use crate::RuntimeError;
use mc_plugin_contract::plugin::PluginKind;
use mc_plugin_host::host::PluginFailureAction;
use mc_plugin_host::runtime::{GameplayProfileHandle, RuntimePluginHost, StorageProfileHandle};
use revy_server_gameplay_bridge::GameplayReadView;
use revy_voxel_core::{
    ConnectionId, CoreCommand, CoreEvent, CoreRuntimeStateBlob, GameplayEffectApplyResult,
    GameplayEffectBatch, GameplayLoginPreview, GameplayLoginPreviewError, PlayerId, PlayerSummary,
    Revisioned, ServerCore, SessionCapabilitySet, TargetedEvent,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

struct KernelStateData {
    core: ServerCore,
    dirty: bool,
}

struct CoreSnapshotReadView {
    snapshot: ServerCore,
}

impl CoreSnapshotReadView {
    fn boxed(snapshot: ServerCore) -> Box<dyn GameplayReadView> {
        Box::new(Self { snapshot })
    }
}

impl GameplayReadView for CoreSnapshotReadView {
    fn world_meta(&mut self) -> revy_server_gameplay_bridge::WorldMeta {
        self.snapshot.world_meta().clone()
    }

    fn player_snapshot(
        &mut self,
        player_id: PlayerId,
    ) -> Option<revy_server_gameplay_bridge::PlayerSnapshot> {
        self.snapshot.player_snapshot(player_id)
    }

    fn block_state(
        &mut self,
        position: revy_server_gameplay_bridge::BlockPos,
    ) -> Option<revy_server_gameplay_bridge::BlockState> {
        self.snapshot.block_state(position)
    }

    fn block_entity(
        &mut self,
        position: revy_server_gameplay_bridge::BlockPos,
    ) -> Option<revy_server_gameplay_bridge::BlockEntityState> {
        self.snapshot.block_entity(position)
    }

    fn can_edit_block(
        &mut self,
        player_id: PlayerId,
        position: revy_server_gameplay_bridge::BlockPos,
    ) -> bool {
        self.snapshot.can_edit_block(player_id, position)
    }
}

struct LoginPreviewReadView {
    preview: GameplayLoginPreview,
}

impl LoginPreviewReadView {
    fn boxed(preview: GameplayLoginPreview) -> Box<dyn GameplayReadView> {
        Box::new(Self { preview })
    }
}

impl GameplayReadView for LoginPreviewReadView {
    fn world_meta(&mut self) -> revy_server_gameplay_bridge::WorldMeta {
        self.preview.world_meta()
    }

    fn player_snapshot(
        &mut self,
        player_id: PlayerId,
    ) -> Option<revy_server_gameplay_bridge::PlayerSnapshot> {
        self.preview.player_snapshot(player_id)
    }

    fn block_state(
        &mut self,
        position: revy_server_gameplay_bridge::BlockPos,
    ) -> Option<revy_server_gameplay_bridge::BlockState> {
        self.preview.block_state(position)
    }

    fn block_entity(
        &mut self,
        position: revy_server_gameplay_bridge::BlockPos,
    ) -> Option<revy_server_gameplay_bridge::BlockEntityState> {
        self.preview.block_entity(position)
    }

    fn can_edit_block(
        &mut self,
        player_id: PlayerId,
        position: revy_server_gameplay_bridge::BlockPos,
    ) -> bool {
        self.preview.can_edit_block(player_id, position)
    }
}

pub(crate) struct ExportedCoreRuntimeState {
    pub(crate) blob: CoreRuntimeStateBlob,
    pub(crate) dirty: bool,
}

pub(crate) enum KernelCommandOutcome {
    Events(Vec<TargetedEvent>),
    StaleGameplayCommand { player_id: PlayerId },
    StaleLogin { connection_id: ConnectionId },
}

pub(crate) struct RuntimeKernel {
    storage_profile: Arc<dyn StorageProfileHandle>,
    world_dir: PathBuf,
    state: Mutex<Revisioned<KernelStateData>>,
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
            state: Mutex::new(Revisioned::new(KernelStateData { core, dirty: false })),
        }
    }

    pub(crate) async fn apply_command(
        &self,
        command: CoreCommand,
        session_capabilities: Option<SessionCapabilitySet>,
        gameplay: Option<Arc<dyn GameplayProfileHandle>>,
        now_ms: u64,
    ) -> Result<KernelCommandOutcome, RuntimeError> {
        let should_persist = matches!(
            command,
            CoreCommand::LoginStart { .. }
                | CoreCommand::Gameplay(..)
                | CoreCommand::InventoryClick { .. }
                | CoreCommand::CloseContainer { .. }
                | CoreCommand::Disconnect { .. }
        );
        match command {
            CoreCommand::LoginStart {
                connection_id,
                username,
                player_id,
            } => {
                if let (Some(session_capabilities), Some(gameplay)) =
                    (session_capabilities.as_ref(), gameplay.as_ref())
                {
                    let (snapshot, revision) = self.snapshot_for_detached_gameplay().await;
                    let read_view = match GameplayLoginPreview::new(
                        snapshot,
                        connection_id,
                        username.clone(),
                        player_id,
                        now_ms,
                    ) {
                        Ok(preview) => LoginPreviewReadView::boxed(preview),
                        Err(GameplayLoginPreviewError::Rejected(events)) => {
                            return Ok(KernelCommandOutcome::Events(events));
                        }
                        Err(GameplayLoginPreviewError::Invalid(message)) => {
                            return Err(RuntimeError::Config(message));
                        }
                    };
                    let batch = gameplay
                        .prepare_player_join(read_view, session_capabilities, player_id, now_ms)
                        .map_err(|error| RuntimeError::Config(error.to_string()))?;
                    Ok(self
                        .commit_detached_login_batch(
                            revision,
                            connection_id,
                            username,
                            player_id,
                            batch,
                            should_persist,
                            KernelCommandOutcome::StaleLogin { connection_id },
                        )
                        .await?)
                } else {
                    Ok(KernelCommandOutcome::Events(
                        self.apply_direct_command(
                            CoreCommand::LoginStart {
                                connection_id,
                                username,
                                player_id,
                            },
                            now_ms,
                            should_persist,
                        )
                        .await,
                    ))
                }
            }
            CoreCommand::Gameplay(gameplay_command) => {
                if let (Some(session_capabilities), Some(gameplay)) =
                    (session_capabilities.as_ref(), gameplay.as_ref())
                {
                    let player_id = gameplay_command.player_id();
                    let (snapshot, revision) = self.snapshot_for_detached_gameplay().await;
                    let batch = gameplay
                        .prepare_command(
                            CoreSnapshotReadView::boxed(snapshot),
                            session_capabilities,
                            &gameplay_command,
                            now_ms,
                        )
                        .map_err(|error| RuntimeError::Config(error.to_string()))?;
                    Ok(self
                        .commit_detached_gameplay_batch(
                            revision,
                            batch,
                            should_persist,
                            KernelCommandOutcome::StaleGameplayCommand { player_id },
                        )
                        .await?)
                } else {
                    Ok(KernelCommandOutcome::Events(
                        self.apply_builtin_gameplay_command(
                            gameplay_command,
                            now_ms,
                            should_persist,
                        )
                        .await,
                    ))
                }
            }
            command => Ok(KernelCommandOutcome::Events(
                self.apply_direct_command(command, now_ms, should_persist)
                    .await,
            )),
        }
    }

    #[cfg(test)]
    pub(crate) async fn open_crafting_table(
        &self,
        player_id: PlayerId,
        window_id: u8,
        _title: &str,
    ) -> Vec<TargetedEvent> {
        let mut state = self.state.lock().await;
        let current_revision = state.revision();
        let (_, (events, _)) = state
            .try_apply_if(
                current_revision,
                |state| {
                    let crafting_table_kind = revy_voxel_semantic::ContainerKindId::new(
                        mc_content_canonical::ids::CRAFTING_TABLE,
                    );
                    let mut should_increment = false;
                    for _ in 1..window_id {
                        let hidden_open_events = {
                            let mut tx = state.core.begin_gameplay_transaction(0);
                            tx.open_virtual_container(player_id, crafting_table_kind.clone());
                            tx.commit()
                        };
                        let hidden_window_id = hidden_open_events
                            .iter()
                            .find_map(|event| match event.event {
                                CoreEvent::ContainerOpened { window_id, .. } => Some(window_id),
                                _ => None,
                            })
                            .expect("hidden crafting table open should emit a window id");
                        let hidden_close_events = state.core.apply_command(
                            CoreCommand::CloseContainer {
                                player_id,
                                window_id: hidden_window_id,
                            },
                            0,
                        );
                        should_increment |=
                            Self::record_commit_side_effects(state, &hidden_open_events, false);
                        should_increment |=
                            Self::record_commit_side_effects(state, &hidden_close_events, false);
                    }
                    let events = {
                        let mut tx = state.core.begin_gameplay_transaction(0);
                        tx.open_virtual_container(player_id, crafting_table_kind);
                        tx.commit()
                    };
                    should_increment |= Self::record_commit_side_effects(state, &events, false);
                    (events, should_increment)
                },
                |(_, should_increment)| *should_increment,
            )
            .expect("kernel test helper should apply against the current revision");
        events
    }

    pub(crate) async fn apply_builtin_tick(
        &self,
        now_ms: u64,
    ) -> Result<Vec<TargetedEvent>, RuntimeError> {
        let mut state = self.state.lock().await;
        let current_revision = state.revision();
        let (_, (events, _)) = state
            .try_apply_if(
                current_revision,
                |state| {
                    let events = state.core.tick(now_ms);
                    let should_increment = Self::record_commit_side_effects(state, &events, false);
                    (events, should_increment)
                },
                |(_, should_increment)| *should_increment,
            )
            .expect("kernel tick should apply against the current revision");
        Ok(events)
    }

    pub(crate) async fn apply_gameplay_tick(
        &self,
        player_id: PlayerId,
        session_capabilities: SessionCapabilitySet,
        gameplay: Arc<dyn GameplayProfileHandle>,
        now_ms: u64,
    ) -> Result<Option<Vec<TargetedEvent>>, RuntimeError> {
        let (snapshot, revision) = self.snapshot_for_detached_gameplay().await;
        let batch = gameplay
            .prepare_tick(
                CoreSnapshotReadView::boxed(snapshot),
                &session_capabilities,
                player_id,
                now_ms,
            )
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        let mut state = self.state.lock().await;
        let expected_revision = if revision == state.revision() {
            revision
        } else {
            state.revision()
        };
        let (_, (apply_result, _)) = state
            .try_apply_if(
                expected_revision,
                |state| {
                    let apply_result = state.core.validate_and_apply_gameplay_effects(batch);
                    let should_increment = match &apply_result {
                        GameplayEffectApplyResult::Applied(events) => {
                            Self::record_commit_side_effects(state, events, false)
                        }
                        GameplayEffectApplyResult::Conflict => false,
                    };
                    (apply_result, should_increment)
                },
                |(_, should_increment)| *should_increment,
            )
            .expect("detached gameplay tick should apply against the current revision");
        match apply_result {
            GameplayEffectApplyResult::Applied(events) => Ok(Some(events)),
            GameplayEffectApplyResult::Conflict => Ok(None),
        }
    }

    pub(crate) async fn snapshot(&self) -> revy_voxel_core::WorldSnapshot {
        self.state.lock().await.state().core.snapshot()
    }

    pub(crate) async fn export_core_runtime_state(&self) -> ExportedCoreRuntimeState {
        let state = self.state.lock().await;
        ExportedCoreRuntimeState {
            blob: state.state().core.export_runtime_state(),
            dirty: state.state().dirty,
        }
    }

    pub(crate) async fn swap_core(&self, candidate: ServerCore, dirty: bool) {
        let mut state = self.state.lock().await;
        let current_revision = state.revision();
        state
            .try_apply(current_revision, |state| {
                state.core = candidate;
                state.dirty = dirty;
            })
            .expect("core swap should apply against the current revision");
    }

    pub(crate) async fn player_summary(&self) -> PlayerSummary {
        self.state.lock().await.state().core.player_summary()
    }

    pub(crate) async fn dirty(&self) -> bool {
        self.state.lock().await.state().dirty
    }

    pub(crate) async fn set_dirty(&self, dirty: bool) {
        let mut state = self.state.lock().await;
        let current_revision = state.revision();
        let _ = state
            .try_apply_if(current_revision, |state| state.dirty = dirty, |_| false)
            .expect("dirty flag update should apply against the current revision");
    }

    pub(crate) async fn set_max_players(&self, max_players: u8) {
        let mut state = self.state.lock().await;
        let current_revision = state.revision();
        state
            .try_apply(current_revision, |state| {
                state.core.set_max_players(max_players);
            })
            .expect("max-player update should apply against the current revision");
    }

    pub(crate) async fn session_resync_events(&self, player_id: PlayerId) -> Vec<TargetedEvent> {
        self.state
            .lock()
            .await
            .state()
            .core
            .session_resync_events(player_id)
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
            if !state.state().dirty {
                return Ok(());
            }
            state.state().core.snapshot()
        };
        match self
            .storage_profile
            .save_snapshot(&self.world_dir, &snapshot)
        {
            Ok(()) => {
                self.set_dirty(false).await;
                Ok(())
            }
            Err(mc_storage_common::StorageError::Plugin(message)) => {
                let action = reload_host.map_or(PluginFailureAction::FailFast, |reload_host| {
                    reload_host.handle_runtime_failure(
                        PluginKind::Storage,
                        self.storage_profile.plugin_id(),
                        &message,
                    )
                });
                self.set_dirty(true).await;
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
                        mc_storage_common::StorageError::Plugin(message),
                    )),
                }
            }
            Err(error) => Err(RuntimeError::Storage(error)),
        }
    }

    async fn snapshot_for_detached_gameplay(&self) -> (ServerCore, u64) {
        let state = self.state.lock().await;
        (state.state().core.clone(), state.revision())
    }

    async fn apply_direct_command(
        &self,
        command: CoreCommand,
        now_ms: u64,
        should_persist: bool,
    ) -> Vec<TargetedEvent> {
        let mut state = self.state.lock().await;
        let current_revision = state.revision();
        let (_, (events, _)) = state
            .try_apply_if(
                current_revision,
                |state| {
                    let events = state.core.apply_command(command, now_ms);
                    let should_increment =
                        Self::record_commit_side_effects(state, &events, should_persist);
                    (events, should_increment)
                },
                |(_, should_increment)| *should_increment,
            )
            .expect("direct command should apply against the current revision");
        events
    }

    async fn apply_builtin_gameplay_command(
        &self,
        command: revy_voxel_core::GameplayCommand,
        now_ms: u64,
        should_persist: bool,
    ) -> Vec<TargetedEvent> {
        let mut state = self.state.lock().await;
        let current_revision = state.revision();
        let (_, (events, _)) = state
            .try_apply_if(
                current_revision,
                |state| {
                    let events = state.core.apply_builtin_gameplay_command(command, now_ms);
                    let should_increment =
                        Self::record_commit_side_effects(state, &events, should_persist);
                    (events, should_increment)
                },
                |(_, should_increment)| *should_increment,
            )
            .expect("builtin gameplay command should apply against the current revision");
        events
    }

    async fn commit_detached_login_batch(
        &self,
        snapshot_revision: u64,
        connection_id: ConnectionId,
        username: String,
        player_id: PlayerId,
        batch: GameplayEffectBatch,
        should_persist: bool,
        stale_outcome: KernelCommandOutcome,
    ) -> Result<KernelCommandOutcome, RuntimeError> {
        let mut state = self.state.lock().await;
        let expected_revision = if snapshot_revision == state.revision() {
            snapshot_revision
        } else {
            state.revision()
        };
        let (_, (apply_result, _)) = state
            .try_apply_if(
                expected_revision,
                |state| {
                    let apply_result = state.core.validate_and_apply_login_effects(
                        connection_id,
                        username,
                        player_id,
                        batch,
                    );
                    let should_increment = match &apply_result {
                        GameplayEffectApplyResult::Applied(events) => {
                            Self::record_commit_side_effects(state, events, should_persist)
                        }
                        GameplayEffectApplyResult::Conflict => false,
                    };
                    (apply_result, should_increment)
                },
                |(_, should_increment)| *should_increment,
            )
            .expect("detached gameplay login batch should apply against the current revision");
        Ok(match apply_result {
            GameplayEffectApplyResult::Applied(events) => KernelCommandOutcome::Events(events),
            GameplayEffectApplyResult::Conflict => stale_outcome,
        })
    }

    async fn commit_detached_gameplay_batch(
        &self,
        snapshot_revision: u64,
        batch: GameplayEffectBatch,
        should_persist: bool,
        stale_outcome: KernelCommandOutcome,
    ) -> Result<KernelCommandOutcome, RuntimeError> {
        let mut state = self.state.lock().await;
        let expected_revision = if snapshot_revision == state.revision() {
            snapshot_revision
        } else {
            state.revision()
        };
        let (_, (apply_result, _)) = state
            .try_apply_if(
                expected_revision,
                |state| {
                    let apply_result = state.core.validate_and_apply_gameplay_effects(batch);
                    let should_increment = match &apply_result {
                        GameplayEffectApplyResult::Applied(events) => {
                            Self::record_commit_side_effects(state, events, should_persist)
                        }
                        GameplayEffectApplyResult::Conflict => false,
                    };
                    (apply_result, should_increment)
                },
                |(_, should_increment)| *should_increment,
            )
            .expect("detached gameplay effect batch should apply against the current revision");
        Ok(match apply_result {
            GameplayEffectApplyResult::Applied(events) => KernelCommandOutcome::Events(events),
            GameplayEffectApplyResult::Conflict => stale_outcome,
        })
    }

    fn record_commit_side_effects(
        state: &mut KernelStateData,
        events: &[TargetedEvent],
        force_dirty: bool,
    ) -> bool {
        if force_dirty
            || events
                .iter()
                .any(|event| !matches!(event.event, CoreEvent::KeepAliveRequested { .. }))
        {
            state.dirty = true;
        }
        force_dirty || !events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_plugin_contract::codec::gameplay::GameplaySessionSnapshot;
    use mc_plugin_host::PluginHostError;
    use mc_storage_common::StorageError;
    use revy_voxel_core::{
        ConnectionId, CoreConfig, EntityId, EventTarget, GameplayCapabilitySet, GameplayCommand,
        GameplayEffect, GameplayEffectBatch, GameplayProfileId, GameplayReadSet, PlayerId,
        ProtocolCapabilitySet, SessionCapabilitySet, StorageCapabilitySet,
    };
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex as StdMutex, mpsc};
    use std::time::Duration;
    use tokio::sync::oneshot;
    use uuid::Uuid;

    struct NullStorage;

    impl StorageProfileHandle for NullStorage {
        fn plugin_id(&self) -> &str {
            "null-storage"
        }

        fn capability_set(&self) -> StorageCapabilitySet {
            StorageCapabilitySet::new()
        }

        fn plugin_generation_id(&self) -> Option<revy_voxel_core::PluginGenerationId> {
            None
        }

        fn load_snapshot(
            &self,
            _world_dir: &Path,
        ) -> Result<Option<revy_voxel_core::WorldSnapshot>, StorageError> {
            Ok(None)
        }

        fn save_snapshot(
            &self,
            _world_dir: &Path,
            _snapshot: &revy_voxel_core::WorldSnapshot,
        ) -> Result<(), StorageError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct TrackingGameplayProfile {
        command_invocations: AtomicUsize,
        join_invocations: AtomicUsize,
        tick_invocations: AtomicUsize,
        callback_pause: StdMutex<Option<CallbackPauseGate>>,
    }

    struct CallbackPauseGate {
        reached_tx: oneshot::Sender<()>,
        release_rx: mpsc::Receiver<()>,
    }

    struct CallbackPauseHandle {
        reached_rx: oneshot::Receiver<()>,
        release_tx: mpsc::Sender<()>,
    }

    impl TrackingGameplayProfile {
        fn arm_callback_pause(&self) -> CallbackPauseHandle {
            let (reached_tx, reached_rx) = oneshot::channel();
            let (release_tx, release_rx) = mpsc::channel();
            *self
                .callback_pause
                .lock()
                .expect("callback pause mutex should not be poisoned") = Some(CallbackPauseGate {
                reached_tx,
                release_rx,
            });
            CallbackPauseHandle {
                reached_rx,
                release_tx,
            }
        }

        fn pause_callback_if_armed(&self) {
            let pause = self
                .callback_pause
                .lock()
                .expect("callback pause mutex should not be poisoned")
                .take();
            if let Some(pause) = pause {
                let _ = pause.reached_tx.send(());
                let _ = pause.release_rx.recv();
            }
        }
    }

    impl CallbackPauseHandle {
        async fn wait_until_reached(&mut self) {
            let _ = (&mut self.reached_rx).await;
        }

        fn release(self) {
            let _ = self.release_tx.send(());
        }
    }

    impl GameplayProfileHandle for TrackingGameplayProfile {
        fn profile_id(&self) -> GameplayProfileId {
            GameplayProfileId::new("tracking")
        }

        fn capability_set(&self) -> GameplayCapabilitySet {
            GameplayCapabilitySet::new()
        }

        fn plugin_generation_id(&self) -> Option<revy_voxel_core::PluginGenerationId> {
            None
        }

        fn prepare_player_join(
            &self,
            mut read_view: Box<dyn GameplayReadView>,
            _session: &SessionCapabilitySet,
            _player_id: PlayerId,
            now_ms: u64,
        ) -> Result<GameplayEffectBatch, PluginHostError> {
            self.join_invocations.fetch_add(1, Ordering::SeqCst);
            self.pause_callback_if_armed();
            let mut reads = GameplayReadSet::default();
            reads.world_meta = Some(read_view.world_meta());
            Ok(GameplayEffectBatch {
                now_ms,
                reads,
                effects: Vec::new(),
            })
        }

        fn prepare_command(
            &self,
            mut read_view: Box<dyn GameplayReadView>,
            _session: &SessionCapabilitySet,
            command: &GameplayCommand,
            now_ms: u64,
        ) -> Result<GameplayEffectBatch, PluginHostError> {
            self.command_invocations.fetch_add(1, Ordering::SeqCst);
            self.pause_callback_if_armed();
            match command {
                GameplayCommand::SetHeldSlot { player_id, slot } => {
                    let player_snapshot =
                        read_view.player_snapshot(*player_id).ok_or_else(|| {
                            PluginHostError::Config(
                                "tracking profile expected a live player".to_string(),
                            )
                        })?;
                    let slot = u8::try_from(*slot).map_err(|_| {
                        PluginHostError::Config(
                            "tracking profile expected a non-negative held slot".to_string(),
                        )
                    })?;
                    let mut reads = GameplayReadSet::default();
                    reads
                        .player_snapshots
                        .insert(*player_id, Some(player_snapshot));
                    Ok(GameplayEffectBatch {
                        now_ms,
                        reads,
                        effects: vec![GameplayEffect::SetSelectedHotbarSlot {
                            player_id: *player_id,
                            slot,
                        }],
                    })
                }
                other => Err(PluginHostError::Config(format!(
                    "tracking profile only supports SetHeldSlot, got {other:?}"
                ))),
            }
        }

        fn prepare_tick(
            &self,
            mut read_view: Box<dyn GameplayReadView>,
            _session: &SessionCapabilitySet,
            player_id: PlayerId,
            now_ms: u64,
        ) -> Result<GameplayEffectBatch, PluginHostError> {
            self.tick_invocations.fetch_add(1, Ordering::SeqCst);
            self.pause_callback_if_armed();
            let player_snapshot = read_view.player_snapshot(player_id).ok_or_else(|| {
                PluginHostError::Config("tracking tick expected a live player".to_string())
            })?;
            let mut reads = GameplayReadSet::default();
            reads
                .player_snapshots
                .insert(player_id, Some(player_snapshot));
            Ok(GameplayEffectBatch {
                now_ms,
                reads,
                effects: Vec::new(),
            })
        }

        fn session_closed(
            &self,
            _session: &GameplaySessionSnapshot,
        ) -> Result<(), PluginHostError> {
            Ok(())
        }
    }

    fn tracking_player_id(name: &str) -> PlayerId {
        PlayerId(Uuid::new_v3(&Uuid::NAMESPACE_OID, name.as_bytes()))
    }

    fn login_session_capabilities() -> SessionCapabilitySet {
        SessionCapabilitySet {
            protocol: ProtocolCapabilitySet::new(),
            gameplay: GameplayCapabilitySet::new(),
            gameplay_profile: GameplayProfileId::new("tracking"),
            entity_id: None,
            protocol_generation: None,
            gameplay_generation: None,
        }
    }

    fn play_session_capabilities(entity_id: EntityId) -> SessionCapabilitySet {
        SessionCapabilitySet {
            entity_id: Some(entity_id),
            ..login_session_capabilities()
        }
    }

    fn logged_in_kernel(name: &str) -> (Arc<RuntimeKernel>, PlayerId, SessionCapabilitySet) {
        let mut core = ServerCore::new(
            CoreConfig::default(),
            crate::runtime::selection::SelectionResolver::content_behavior(),
        );
        let player_id = tracking_player_id(name);
        let _events = core.apply_command(
            CoreCommand::LoginStart {
                connection_id: ConnectionId(1),
                username: name.to_string(),
                player_id,
            },
            0,
        );
        let runtime_state = core.export_runtime_state();
        let entity_id = runtime_state
            .online_players
            .get(&player_id)
            .expect("logged-in player should have an entity id");
        (
            Arc::new(RuntimeKernel::new(
                core,
                Arc::new(NullStorage),
                PathBuf::from("world"),
            )),
            player_id,
            play_session_capabilities(entity_id.session.entity_id),
        )
    }

    async fn selected_hotbar_slot(kernel: &RuntimeKernel, player_id: PlayerId) -> u8 {
        kernel
            .export_core_runtime_state()
            .await
            .blob
            .online_players
            .get(&player_id)
            .expect("tracking test expected a live player snapshot")
            .player
            .selected_hotbar_slot
    }

    #[tokio::test]
    async fn detached_gameplay_command_pause_does_not_block_direct_commands()
    -> Result<(), RuntimeError> {
        let (kernel, player_id, session) = logged_in_kernel("detached-direct");
        let gameplay = Arc::new(TrackingGameplayProfile::default());
        let mut pause = gameplay.arm_callback_pause();
        let task_kernel = Arc::clone(&kernel);
        let task_gameplay = Arc::clone(&gameplay);
        let task_session = session.clone();
        let task = tokio::spawn(async move {
            task_kernel
                .apply_command(
                    GameplayCommand::SetHeldSlot { player_id, slot: 5 }.into(),
                    Some(task_session),
                    Some(task_gameplay),
                    0,
                )
                .await
        });

        pause.wait_until_reached().await;
        tokio::time::timeout(
            Duration::from_millis(250),
            kernel.apply_direct_command(
                CoreCommand::UpdateClientView {
                    player_id,
                    view_distance: 4,
                },
                0,
                false,
            ),
        )
        .await
        .expect("detached gameplay pause should not hold the kernel lock");
        pause.release();

        let outcome = task.await.expect("detached gameplay task should join")?;
        assert!(matches!(outcome, KernelCommandOutcome::Events(_)));
        assert_eq!(gameplay.command_invocations.load(Ordering::SeqCst), 1);
        assert_eq!(selected_hotbar_slot(kernel.as_ref(), player_id).await, 5);
        Ok(())
    }

    #[tokio::test]
    async fn detached_gameplay_conflict_returns_stale_without_reinvoking_callback()
    -> Result<(), RuntimeError> {
        let (kernel, player_id, session) = logged_in_kernel("detached-stale");
        let gameplay = Arc::new(TrackingGameplayProfile::default());
        let mut pause = gameplay.arm_callback_pause();
        let task_kernel = Arc::clone(&kernel);
        let task_gameplay = Arc::clone(&gameplay);
        let task_session = session.clone();
        let task = tokio::spawn(async move {
            task_kernel
                .apply_command(
                    GameplayCommand::SetHeldSlot { player_id, slot: 5 }.into(),
                    Some(task_session),
                    Some(task_gameplay),
                    0,
                )
                .await
        });

        pause.wait_until_reached().await;
        let _events = kernel
            .apply_builtin_gameplay_command(
                GameplayCommand::SetHeldSlot { player_id, slot: 1 },
                0,
                true,
            )
            .await;
        pause.release();

        let outcome = task.await.expect("stale gameplay task should join")?;
        assert!(matches!(
            outcome,
            KernelCommandOutcome::StaleGameplayCommand { player_id: stale_player_id }
                if stale_player_id == player_id
        ));
        assert_eq!(gameplay.command_invocations.load(Ordering::SeqCst), 1);
        assert_eq!(selected_hotbar_slot(kernel.as_ref(), player_id).await, 1);
        let resync_events = kernel.session_resync_events(player_id).await;
        assert!(resync_events.iter().any(|event| {
            matches!(
                (&event.target, &event.event),
                (
                    EventTarget::Player(event_player_id),
                    CoreEvent::SelectedHotbarSlotChanged { slot: 1 }
                ) if *event_player_id == player_id
            )
        }));
        Ok(())
    }

    #[tokio::test]
    async fn detached_login_conflict_returns_stale_without_half_online_player()
    -> Result<(), RuntimeError> {
        let kernel = Arc::new(RuntimeKernel::new(
            ServerCore::new(
                CoreConfig::default(),
                crate::runtime::selection::SelectionResolver::content_behavior(),
            ),
            Arc::new(NullStorage),
            PathBuf::from("world"),
        ));
        let gameplay = Arc::new(TrackingGameplayProfile::default());
        let player_id = tracking_player_id("detached-login");
        let mut pause = gameplay.arm_callback_pause();
        let task_kernel = Arc::clone(&kernel);
        let task_gameplay = Arc::clone(&gameplay);
        let task = tokio::spawn(async move {
            task_kernel
                .apply_command(
                    CoreCommand::LoginStart {
                        connection_id: ConnectionId(7),
                        username: "detached-login".to_string(),
                        player_id,
                    },
                    Some(login_session_capabilities()),
                    Some(task_gameplay),
                    0,
                )
                .await
        });

        pause.wait_until_reached().await;
        let _events = kernel
            .apply_direct_command(
                CoreCommand::LoginStart {
                    connection_id: ConnectionId(8),
                    username: "detached-login".to_string(),
                    player_id,
                },
                0,
                true,
            )
            .await;
        pause.release();

        let outcome = task.await.expect("detached login task should join")?;
        assert!(matches!(
            outcome,
            KernelCommandOutcome::StaleLogin { connection_id } if connection_id == ConnectionId(7)
        ));
        assert_eq!(gameplay.join_invocations.load(Ordering::SeqCst), 1);
        let state = kernel.export_core_runtime_state().await;
        assert!(state.blob.online_players.contains_key(&player_id));
        assert_eq!(state.blob.online_players.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn rejected_login_skips_gameplay_plugin_invocation() -> Result<(), RuntimeError> {
        let mut config = CoreConfig::default();
        config.max_players = 0;
        let kernel = Arc::new(RuntimeKernel::new(
            ServerCore::new(
                config,
                crate::runtime::selection::SelectionResolver::content_behavior(),
            ),
            Arc::new(NullStorage),
            PathBuf::from("world"),
        ));
        let gameplay = Arc::new(TrackingGameplayProfile::default());
        let outcome = kernel
            .apply_command(
                CoreCommand::LoginStart {
                    connection_id: ConnectionId(9),
                    username: "rejected".to_string(),
                    player_id: tracking_player_id("rejected"),
                },
                Some(login_session_capabilities()),
                Some(gameplay.clone()),
                0,
            )
            .await?;

        assert!(matches!(outcome, KernelCommandOutcome::Events(events) if !events.is_empty()));
        assert_eq!(gameplay.join_invocations.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn detached_gameplay_tick_pause_does_not_block_direct_commands()
    -> Result<(), RuntimeError> {
        let (kernel, player_id, session) = logged_in_kernel("detached-tick");
        let gameplay = Arc::new(TrackingGameplayProfile::default());
        let mut pause = gameplay.arm_callback_pause();
        let task_kernel = Arc::clone(&kernel);
        let task_gameplay = Arc::clone(&gameplay);
        let task_session = session.clone();
        let task = tokio::spawn(async move {
            task_kernel
                .apply_gameplay_tick(player_id, task_session, task_gameplay, 50)
                .await
        });

        pause.wait_until_reached().await;
        tokio::time::timeout(
            Duration::from_millis(250),
            kernel.apply_direct_command(
                CoreCommand::UpdateClientView {
                    player_id,
                    view_distance: 5,
                },
                0,
                false,
            ),
        )
        .await
        .expect("paused gameplay ticks should leave the kernel lock available");
        pause.release();

        let events = task.await.expect("detached tick task should join")?;
        assert_eq!(gameplay.tick_invocations.load(Ordering::SeqCst), 1);
        assert_eq!(events, Some(Vec::new()));
        Ok(())
    }
}
