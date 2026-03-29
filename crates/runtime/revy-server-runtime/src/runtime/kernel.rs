use crate::RuntimeError;
use mc_plugin_api::abi::PluginKind;
use mc_plugin_host::host::PluginFailureAction;
use mc_plugin_host::runtime::{GameplayProfileHandle, RuntimePluginHost, StorageProfileHandle};
use revy_voxel_core::{
    ConnectionId, CoreCommand, CoreEvent, CoreRuntimeStateBlob, GameplayJournalApplyResult,
    PlayerId, PlayerSummary, Revisioned, ServerCore, SessionCapabilitySet, TargetedEvent,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
#[cfg(test)]
use tokio::sync::{Mutex as AsyncMutex, oneshot};

struct KernelStateData {
    core: ServerCore,
    dirty: bool,
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
    #[cfg(test)]
    detached_gameplay_pause_hook: AsyncMutex<Option<DetachedGameplayPauseHook>>,
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
            #[cfg(test)]
            detached_gameplay_pause_hook: AsyncMutex::new(None),
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
                    let journal = gameplay
                        .prepare_player_join(
                            snapshot,
                            session_capabilities,
                            connection_id,
                            username,
                            player_id,
                            now_ms,
                        )
                        .map_err(|error| RuntimeError::Config(error.to_string()))?;
                    Ok(self
                        .commit_detached_gameplay_journal(
                            revision,
                            journal,
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
            command => {
                if let Ok(gameplay_command) = command.clone().into_gameplay() {
                    if let (Some(session_capabilities), Some(gameplay)) =
                        (session_capabilities.as_ref(), gameplay.as_ref())
                    {
                        let player_id = gameplay_command.player_id();
                        let (snapshot, revision) = self.snapshot_for_detached_gameplay().await;
                        let journal = gameplay
                            .prepare_command(
                                snapshot,
                                session_capabilities,
                                &gameplay_command,
                                now_ms,
                            )
                            .map_err(|error| RuntimeError::Config(error.to_string()))?;
                        Ok(self
                            .commit_detached_gameplay_journal(
                                revision,
                                journal,
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
                } else {
                    Ok(KernelCommandOutcome::Events(
                        self.apply_direct_command(command, now_ms, should_persist)
                            .await,
                    ))
                }
            }
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
                    let crafting_table_kind = revy_voxel_rules::ContainerKindId::new(
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
        let journal = gameplay
            .prepare_tick(snapshot, &session_capabilities, player_id, now_ms)
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        #[cfg(test)]
        self.maybe_pause_before_detached_gameplay_commit_for_test()
            .await;
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
                    let apply_result = state.core.validate_and_apply_gameplay_journal(journal);
                    let should_increment = match &apply_result {
                        GameplayJournalApplyResult::Applied(events) => {
                            Self::record_commit_side_effects(state, events, false)
                        }
                        GameplayJournalApplyResult::Conflict => false,
                    };
                    (apply_result, should_increment)
                },
                |(_, should_increment)| *should_increment,
            )
            .expect("detached gameplay tick should apply against the current revision");
        match apply_result {
            GameplayJournalApplyResult::Applied(events) => Ok(Some(events)),
            GameplayJournalApplyResult::Conflict => Ok(None),
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

    #[cfg(test)]
    pub(crate) async fn arm_detached_gameplay_pause_for_test(&self) -> DetachedGameplayPauseHandle {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self.detached_gameplay_pause_hook.lock().await = Some(DetachedGameplayPauseHook {
            reached_tx: Some(reached_tx),
            release_rx,
        });
        DetachedGameplayPauseHandle {
            reached_rx,
            release_tx: Some(release_tx),
        }
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
            Err(mc_proto_common::StorageError::Plugin(message)) => {
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
                        mc_proto_common::StorageError::Plugin(message),
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

    async fn commit_detached_gameplay_journal(
        &self,
        snapshot_revision: u64,
        journal: revy_voxel_core::GameplayJournal,
        should_persist: bool,
        stale_outcome: KernelCommandOutcome,
    ) -> Result<KernelCommandOutcome, RuntimeError> {
        #[cfg(test)]
        self.maybe_pause_before_detached_gameplay_commit_for_test()
            .await;
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
                    let apply_result = state.core.validate_and_apply_gameplay_journal(journal);
                    let should_increment = match &apply_result {
                        GameplayJournalApplyResult::Applied(events) => {
                            Self::record_commit_side_effects(state, events, should_persist)
                        }
                        GameplayJournalApplyResult::Conflict => false,
                    };
                    (apply_result, should_increment)
                },
                |(_, should_increment)| *should_increment,
            )
            .expect("detached gameplay journal should apply against the current revision");
        Ok(match apply_result {
            GameplayJournalApplyResult::Applied(events) => KernelCommandOutcome::Events(events),
            GameplayJournalApplyResult::Conflict => stale_outcome,
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

    #[cfg(test)]
    async fn maybe_pause_before_detached_gameplay_commit_for_test(&self) {
        let hook = self.detached_gameplay_pause_hook.lock().await.take();
        let Some(mut hook) = hook else {
            return;
        };
        if let Some(reached_tx) = hook.reached_tx.take() {
            let _ = reached_tx.send(());
        }
        let _ = hook.release_rx.await;
    }
}

#[cfg(test)]
struct DetachedGameplayPauseHook {
    reached_tx: Option<oneshot::Sender<()>>,
    release_rx: oneshot::Receiver<()>,
}

#[cfg(test)]
pub(crate) struct DetachedGameplayPauseHandle {
    reached_rx: oneshot::Receiver<()>,
    release_tx: Option<oneshot::Sender<()>>,
}

#[cfg(test)]
impl DetachedGameplayPauseHandle {
    pub(crate) async fn wait_until_reached(&mut self) {
        let _ = (&mut self.reached_rx).await;
    }

    pub(crate) fn release(mut self) {
        if let Some(release_tx) = self.release_tx.take() {
            let _ = release_tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_plugin_api::codec::gameplay::GameplaySessionSnapshot;
    use mc_plugin_host::PluginHostError;
    use mc_proto_common::StorageError;
    use revy_voxel_core::{
        ConnectionId, CoreConfig, EntityId, EventTarget, GameplayCapabilitySet, GameplayCommand,
        GameplayJournal, GameplayProfileId, GameplayTransaction, PlayerId, ProtocolCapabilitySet,
        SessionCapabilitySet, StorageCapabilitySet,
    };
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
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
            snapshot: ServerCore,
            _session: &SessionCapabilitySet,
            connection_id: ConnectionId,
            username: String,
            player_id: PlayerId,
            now_ms: u64,
        ) -> Result<GameplayJournal, PluginHostError> {
            self.join_invocations.fetch_add(1, Ordering::SeqCst);
            let mut tx = GameplayTransaction::detached(snapshot, now_ms);
            if let Some(rejection) = tx
                .begin_login(connection_id, username, player_id)
                .map_err(PluginHostError::Config)?
            {
                for event in rejection {
                    tx.emit_event(event.target, event.event);
                }
                return Ok(tx.into_journal());
            }
            tx.finalize_login(connection_id, player_id)
                .map_err(PluginHostError::Config)?;
            Ok(tx.into_journal())
        }

        fn prepare_command(
            &self,
            snapshot: ServerCore,
            _session: &SessionCapabilitySet,
            command: &GameplayCommand,
            now_ms: u64,
        ) -> Result<GameplayJournal, PluginHostError> {
            self.command_invocations.fetch_add(1, Ordering::SeqCst);
            let mut tx = GameplayTransaction::detached(snapshot, now_ms);
            match command {
                GameplayCommand::SetHeldSlot { player_id, slot } => {
                    tx.player_snapshot(*player_id).ok_or_else(|| {
                        PluginHostError::Config(
                            "tracking profile expected a live player".to_string(),
                        )
                    })?;
                    let slot = u8::try_from(*slot).map_err(|_| {
                        PluginHostError::Config(
                            "tracking profile expected a non-negative held slot".to_string(),
                        )
                    })?;
                    tx.set_selected_hotbar_slot(*player_id, slot);
                }
                other => {
                    return Err(PluginHostError::Config(format!(
                        "tracking profile only supports SetHeldSlot, got {other:?}"
                    )));
                }
            }
            Ok(tx.into_journal())
        }

        fn prepare_tick(
            &self,
            snapshot: ServerCore,
            _session: &SessionCapabilitySet,
            player_id: PlayerId,
            now_ms: u64,
        ) -> Result<GameplayJournal, PluginHostError> {
            self.tick_invocations.fetch_add(1, Ordering::SeqCst);
            let mut tx = GameplayTransaction::detached(snapshot, now_ms);
            tx.player_snapshot(player_id).ok_or_else(|| {
                PluginHostError::Config("tracking tick expected a live player".to_string())
            })?;
            Ok(tx.into_journal())
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
        let mut pause = kernel.arm_detached_gameplay_pause_for_test().await;
        let task_kernel = Arc::clone(&kernel);
        let task_gameplay = Arc::clone(&gameplay);
        let task_session = session.clone();
        let task = tokio::spawn(async move {
            task_kernel
                .apply_command(
                    CoreCommand::SetHeldSlot { player_id, slot: 5 },
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
        let mut pause = kernel.arm_detached_gameplay_pause_for_test().await;
        let task_kernel = Arc::clone(&kernel);
        let task_gameplay = Arc::clone(&gameplay);
        let task_session = session.clone();
        let task = tokio::spawn(async move {
            task_kernel
                .apply_command(
                    CoreCommand::SetHeldSlot { player_id, slot: 5 },
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
        let mut pause = kernel.arm_detached_gameplay_pause_for_test().await;
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
    async fn detached_gameplay_tick_pause_does_not_block_direct_commands()
    -> Result<(), RuntimeError> {
        let (kernel, player_id, session) = logged_in_kernel("detached-tick");
        let gameplay = Arc::new(TrackingGameplayProfile::default());
        let mut pause = kernel.arm_detached_gameplay_pause_for_test().await;
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
