mod journal;

use self::journal::CoreJournal;
use crate::RuntimeError;
use mc_plugin_contract::plugin::PluginKind;
use mc_plugin_host::host::PluginFailureAction;
use mc_plugin_host::runtime::{GameplayProfileHandle, RuntimePluginHost, StorageProfileHandle};
use revy_server_gameplay_bridge::{GameplayEffectBatch, GameplayReadView};
use revy_voxel_core::{
    ConnectionId, CoreCommand, CoreEvent, CoreHandoff, CoreMutation, CoreRevision,
    CoreTransferDeltaDescriptor, CoreTransferError, CoreVersion, EncodedCoreTransferCommit,
    EventTarget, GameplayEffectApplyResult, GameplayLoginPreview, GameplayLoginPreviewError,
    PlayerId, PlayerSummary, PreparedCoreCommit, ServerCore, SessionCapabilitySet, TargetedEvent,
};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

const CORE_JOURNAL_EVENT_LIMIT: usize = 65_536;
const CORE_JOURNAL_EVENT_BYTE_LIMIT: usize = 32 * 1024 * 1024;
const CORE_JOURNAL_TRANSFER_ENTRY_BYTE_LIMIT: usize = 2 * 1024 * 1024;
const CORE_JOURNAL_TRANSFER_BYTE_LIMIT: usize = 8 * 1024 * 1024;
pub(crate) const CORE_PROCESS_DELTA_SLOT_BYTES: usize = CORE_JOURNAL_TRANSFER_BYTE_LIMIT
    + revy_voxel_core::CoreTransferDelta::MAX_COMMITS * std::mem::size_of::<u32>()
    + 8
    + std::mem::size_of::<u16>()
    + std::mem::size_of::<u64>() * 2
    + std::mem::size_of::<u32>();

struct CoreStoreState {
    active: Arc<CoreVersion>,
    persisted_revision: CoreRevision,
    latest_dirty_revision: Option<CoreRevision>,
    journal: CoreJournal,
}

#[derive(Clone)]
pub(crate) struct SharedCoreEvent {
    pub(crate) target: EventTarget,
    pub(crate) event: Arc<CoreEvent>,
}

impl From<TargetedEvent> for SharedCoreEvent {
    fn from(event: TargetedEvent) -> Self {
        Self {
            target: event.target,
            event: Arc::new(event.event),
        }
    }
}

pub(crate) struct CorePrecopy {
    version: Arc<CoreVersion>,
    persisted_revision: CoreRevision,
    latest_dirty_revision: Option<CoreRevision>,
}

pub(crate) struct CoreProcessPrecopy {
    pub(crate) version: Arc<CoreVersion>,
    pub(crate) persisted_revision: CoreRevision,
    pub(crate) latest_dirty_revision: Option<CoreRevision>,
}

pub(crate) struct CoreCandidatePlan {
    base_revision: CoreRevision,
    config: Option<revy_voxel_core::CoreConfig>,
    storage_profile: Arc<dyn StorageProfileHandle>,
    world_dir: PathBuf,
}

pub(crate) enum CoreDeltaSeal {
    Ready {
        precopy: CorePrecopy,
        events: Vec<SharedCoreEvent>,
    },
    Outpaced {
        requested_revision: CoreRevision,
        earliest_revision: CoreRevision,
    },
}

pub(crate) enum CoreProcessDeltaSeal {
    Ready {
        descriptor: CoreTransferDeltaDescriptor,
        persisted_revision: CoreRevision,
        latest_dirty_revision: Option<CoreRevision>,
    },
    Outpaced {
        requested_revision: CoreRevision,
        earliest_revision: CoreRevision,
    },
}

struct VersionReadView {
    version: Arc<CoreVersion>,
}

impl VersionReadView {
    fn boxed(version: Arc<CoreVersion>) -> Box<dyn GameplayReadView> {
        Box::new(Self { version })
    }
}

impl GameplayReadView for VersionReadView {
    fn world_meta(&mut self) -> revy_server_gameplay_bridge::WorldMeta {
        self.version.world_meta().clone()
    }

    fn player_snapshot(
        &mut self,
        player_id: PlayerId,
    ) -> Option<revy_server_gameplay_bridge::PlayerSnapshot> {
        self.version.player_snapshot(player_id)
    }

    fn block_state(
        &mut self,
        position: revy_server_gameplay_bridge::BlockPos,
    ) -> Option<revy_server_gameplay_bridge::BlockState> {
        self.version.block_state(position)
    }

    fn block_entity(
        &mut self,
        position: revy_server_gameplay_bridge::BlockPos,
    ) -> Option<revy_server_gameplay_bridge::BlockEntityState> {
        self.version.block_entity(position)
    }

    fn can_edit_block(
        &mut self,
        player_id: PlayerId,
        position: revy_server_gameplay_bridge::BlockPos,
    ) -> bool {
        self.version.can_edit_block(player_id, position)
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

pub(crate) enum CoreCommandOutcome {
    Events(Vec<SharedCoreEvent>),
    StaleGameplayCommand { player_id: PlayerId },
    StaleLogin { connection_id: ConnectionId },
}

pub(crate) struct PreparedGameplayTick {
    player_id: PlayerId,
    source_revision: CoreRevision,
    batch: GameplayEffectBatch,
}

pub(crate) struct StaleGameplayTick {
    pub(crate) player_id: PlayerId,
    pub(crate) source_revision: CoreRevision,
    pub(crate) active_revision: CoreRevision,
}

pub(crate) struct GameplayTickCommitOutcome {
    pub(crate) events: Vec<SharedCoreEvent>,
    pub(crate) stale: Vec<StaleGameplayTick>,
}

pub(crate) enum CoreInvocation {
    Internal,
    Login {
        capabilities: SessionCapabilitySet,
        gameplay: Arc<dyn GameplayProfileHandle>,
    },
    Play {
        player_id: PlayerId,
        capabilities: SessionCapabilitySet,
        gameplay: Arc<dyn GameplayProfileHandle>,
    },
}

pub(crate) struct CoreStore {
    storage_profile: Arc<dyn StorageProfileHandle>,
    world_dir: PathBuf,
    state: Mutex<CoreStoreState>,
}

impl CoreStore {
    pub(crate) fn new(
        core: ServerCore,
        storage_profile: Arc<dyn StorageProfileHandle>,
        world_dir: PathBuf,
    ) -> Self {
        let active = CoreVersion::initial(core);
        Self {
            storage_profile,
            world_dir,
            state: Mutex::new(CoreStoreState {
                persisted_revision: active.revision(),
                journal_active: false,
                journal_floor: active.revision(),
                journal_event_count: 0,
                journal_event_bytes: 0,
                journal_transfer_bytes: 0,
                journal: VecDeque::new(),
                active,
                latest_dirty_revision: None,
            }),
        }
    }

    pub(crate) fn from_handoff(
        handoff: CoreHandoff,
        storage_profile: Arc<dyn StorageProfileHandle>,
        world_dir: PathBuf,
        persisted_revision: CoreRevision,
        latest_dirty_revision: Option<CoreRevision>,
    ) -> Self {
        Self {
            storage_profile,
            world_dir,
            state: Mutex::new(CoreStoreState {
                active: handoff.version(),
                persisted_revision,
                latest_dirty_revision,
                journal_active: false,
                journal_floor: handoff.version().revision(),
                journal_event_count: 0,
                journal_event_bytes: 0,
                journal_transfer_bytes: 0,
                journal: VecDeque::new(),
            }),
        }
    }

    pub(crate) async fn version(&self) -> Arc<CoreVersion> {
        Arc::clone(&self.state.lock().await.active)
    }

    pub(crate) async fn process_precopy(&self) -> CoreProcessPrecopy {
        let mut state = self.state.lock().await;
        Self::begin_journal(&mut state);
        CoreProcessPrecopy {
            version: Arc::clone(&state.active),
            persisted_revision: state.persisted_revision,
            latest_dirty_revision: state.latest_dirty_revision,
        }
    }

    pub(crate) async fn plan_candidate(
        &self,
        config: Option<revy_voxel_core::CoreConfig>,
        storage_profile: Arc<dyn StorageProfileHandle>,
    ) -> (CoreCandidatePlan, Arc<Self>) {
        let mut state = self.state.lock().await;
        Self::begin_journal(&mut state);
        let precopy = CorePrecopy {
            version: Arc::clone(&state.active),
            persisted_revision: state.persisted_revision,
            latest_dirty_revision: state.latest_dirty_revision,
        };
        let plan = CoreCandidatePlan {
            base_revision: precopy.version.revision(),
            config,
            storage_profile,
            world_dir: self.world_dir.clone(),
        };
        let candidate = plan.materialize(precopy);
        (plan, candidate)
    }

    pub(crate) async fn seal_delta_since(&self, revision: CoreRevision) -> CoreDeltaSeal {
        let mut state = self.state.lock().await;
        state.journal_active = false;
        if revision < state.journal_floor {
            return CoreDeltaSeal::Outpaced {
                requested_revision: revision,
                earliest_revision: state.journal_floor,
            };
        }
        let events = state
            .journal
            .iter()
            .filter(|entry| entry.revision > revision)
            .flat_map(|entry| entry.events.iter().cloned())
            .collect();
        CoreDeltaSeal::Ready {
            precopy: CorePrecopy {
                version: Arc::clone(&state.active),
                persisted_revision: state.persisted_revision,
                latest_dirty_revision: state.latest_dirty_revision,
            },
            events,
        }
    }

    pub(crate) async fn write_process_delta_since(
        &self,
        revision: CoreRevision,
        writer: &mut (impl Write + Send),
    ) -> Result<CoreProcessDeltaSeal, CoreTransferError> {
        // Pre-copy can advance while mutations continue. Sealing a delta does not revoke the
        // journal; the cutover owner ends retention after commit or abort.
        let state = self.state.lock().await;
        if revision > state.active.revision() {
            return Err(CoreTransferError::InvalidState(format!(
                "core delta base revision {} exceeds active revision {}",
                revision.value(),
                state.active.revision().value(),
            )));
        }
        if revision < state.journal_floor {
            return Ok(CoreProcessDeltaSeal::Outpaced {
                requested_revision: revision,
                earliest_revision: state.journal_floor,
            });
        }
        let commits = state
            .journal
            .iter()
            .filter(|entry| entry.revision > revision)
            .map(|entry| &entry.transfer);
        Ok(CoreProcessDeltaSeal::Ready {
            descriptor: revy_voxel_core::CoreTransferDelta::seal_preencoded_into(
                revision, commits, writer,
            )?,
            persisted_revision: state.persisted_revision,
            latest_dirty_revision: state.latest_dirty_revision,
        })
    }

    /// Stops retaining semantic commits after a pre-copy that will not reach its seal boundary.
    ///
    /// Retained entries are deliberately not released here: this operation can run while
    /// recovering a freeze, and dropping the bounded journal must not extend the data-plane
    /// pause. The next mutation or pre-copy reclaims the retained storage outside that boundary.
    pub(crate) async fn end_precopy(&self) {
        self.state.lock().await.journal_active = false;
    }

    pub(crate) async fn apply_command(
        &self,
        command: CoreCommand,
        invocation: CoreInvocation,
        now_ms: u64,
    ) -> Result<CoreCommandOutcome, RuntimeError> {
        let requires_persistence = matches!(
            command,
            CoreCommand::LoginStart { .. }
                | CoreCommand::Gameplay(..)
                | CoreCommand::InventoryClick { .. }
                | CoreCommand::CloseContainer { .. }
                | CoreCommand::Disconnect { .. }
        );
        match (command, invocation) {
            (
                CoreCommand::LoginStart {
                    connection_id,
                    username,
                    player_id,
                },
                CoreInvocation::Login {
                    capabilities,
                    gameplay,
                },
            ) => {
                let snapshot = self.version().await;
                let preview = match snapshot.login_preview(
                    connection_id,
                    username.clone(),
                    player_id,
                    now_ms,
                ) {
                    Ok(preview) => preview,
                    Err(GameplayLoginPreviewError::Rejected(events)) => {
                        return Ok(CoreCommandOutcome::Events(
                            events.into_iter().map(SharedCoreEvent::from).collect(),
                        ));
                    }
                    Err(GameplayLoginPreviewError::Invalid(message)) => {
                        return Err(RuntimeError::Config(message));
                    }
                };
                let batch = gameplay
                    .prepare_player_join(
                        LoginPreviewReadView::boxed(preview),
                        &capabilities,
                        player_id,
                        now_ms,
                    )
                    .map_err(|error| RuntimeError::Config(error.to_string()))?;
                let mut state = self.state.lock().await;
                let mut mutation = CoreMutation::from_version(&state.active);
                match mutation.validate_and_apply_login_effects(
                    connection_id,
                    username,
                    player_id,
                    batch,
                ) {
                    GameplayEffectApplyResult::Applied(events) => {
                        let prepared = mutation.prepare(events, requires_persistence);
                        Ok(CoreCommandOutcome::Events(Self::commit_locked(
                            &mut state, prepared,
                        )?))
                    }
                    GameplayEffectApplyResult::Conflict => {
                        Ok(CoreCommandOutcome::StaleLogin { connection_id })
                    }
                }
            }
            (
                CoreCommand::Gameplay(gameplay_command),
                CoreInvocation::Play {
                    player_id,
                    capabilities,
                    gameplay,
                },
            ) => {
                if gameplay_command.player_id() != player_id {
                    return Ok(CoreCommandOutcome::StaleGameplayCommand { player_id });
                }
                let snapshot = self.version().await;
                let batch = gameplay
                    .prepare_command(
                        VersionReadView::boxed(Arc::clone(&snapshot)),
                        &capabilities,
                        &gameplay_command,
                        now_ms,
                    )
                    .map_err(|error| RuntimeError::Config(error.to_string()))?;
                let mut state = self.state.lock().await;
                let mut mutation = CoreMutation::from_version(&state.active);
                match mutation.validate_and_apply_gameplay_effects(batch) {
                    GameplayEffectApplyResult::Applied(events) => {
                        let prepared = mutation.prepare(events, requires_persistence);
                        Ok(CoreCommandOutcome::Events(Self::commit_locked(
                            &mut state, prepared,
                        )?))
                    }
                    GameplayEffectApplyResult::Conflict => {
                        Ok(CoreCommandOutcome::StaleGameplayCommand { player_id })
                    }
                }
            }
            (command, CoreInvocation::Internal)
            | (command, CoreInvocation::Login { .. })
            | (command, CoreInvocation::Play { .. }) => {
                let mut state = self.state.lock().await;
                let mut mutation = CoreMutation::from_version(&state.active);
                let events = mutation.apply_command(command, now_ms);
                let prepared = mutation.prepare(events, requires_persistence);
                Ok(CoreCommandOutcome::Events(Self::commit_locked(
                    &mut state, prepared,
                )?))
            }
        }
    }

    pub(crate) async fn apply_builtin_tick(
        &self,
        now_ms: u64,
    ) -> Result<Vec<SharedCoreEvent>, RuntimeError> {
        let mut state = self.state.lock().await;
        let mut mutation = CoreMutation::from_version(&state.active);
        let events = mutation.tick(now_ms);
        let prepared = mutation.prepare(events, false);
        Self::commit_locked(&mut state, prepared)
    }

    pub(crate) fn prepare_gameplay_tick(
        snapshot: Arc<CoreVersion>,
        player_id: PlayerId,
        session_capabilities: SessionCapabilitySet,
        gameplay: Arc<dyn GameplayProfileHandle>,
        now_ms: u64,
    ) -> Result<PreparedGameplayTick, RuntimeError> {
        let batch = gameplay
            .prepare_tick(
                VersionReadView::boxed(Arc::clone(&snapshot)),
                &session_capabilities,
                player_id,
                now_ms,
            )
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        Ok(PreparedGameplayTick {
            player_id,
            source_revision: snapshot.revision(),
            batch,
        })
    }

    pub(crate) async fn commit_gameplay_ticks(
        &self,
        prepared_ticks: Vec<PreparedGameplayTick>,
    ) -> Result<GameplayTickCommitOutcome, RuntimeError> {
        if prepared_ticks.is_empty() {
            return Ok(GameplayTickCommitOutcome {
                events: Vec::new(),
                stale: Vec::new(),
            });
        }
        let mut state = self.state.lock().await;
        let active_revision = state.active.revision();
        let mut mutation = CoreMutation::from_version(&state.active);
        let mut events = Vec::new();
        let mut stale = Vec::new();
        for prepared in prepared_ticks {
            let PreparedGameplayTick {
                player_id,
                source_revision,
                batch,
            } = prepared;
            match mutation.validate_and_apply_gameplay_effects(batch) {
                GameplayEffectApplyResult::Applied(batch_events) => {
                    events.extend(batch_events);
                }
                GameplayEffectApplyResult::Conflict => stale.push(StaleGameplayTick {
                    player_id,
                    source_revision,
                    active_revision,
                }),
            }
        }
        let prepared = mutation.prepare(events, false);
        let events = Self::commit_locked(&mut state, prepared)?;
        Ok(GameplayTickCommitOutcome { events, stale })
    }

    fn commit_locked(
        state: &mut CoreStoreState,
        prepared: PreparedCoreCommit,
    ) -> Result<Vec<SharedCoreEvent>, RuntimeError> {
        if state.active.revision() != prepared.base_revision() {
            return Err(RuntimeError::Config(format!(
                "stale core commit: expected revision {}, active revision {}",
                prepared.base_revision().value(),
                state.active.revision().value()
            )));
        }
        let (next_version, events, requires_persistence, transfer) = prepared.into_parts();
        let events = events
            .into_iter()
            .map(SharedCoreEvent::from)
            .collect::<Vec<_>>();
        if requires_persistence {
            state.latest_dirty_revision = Some(next_version.revision());
        }
        if next_version.revision() > state.active.revision() {
            if !state.journal_active {
                Self::reset_journal(state, next_version.revision());
                state.active = next_version;
                return Ok(events);
            }
            let transfer = transfer.encode().ok();
            let transfer_bytes = transfer
                .as_ref()
                .map(EncodedCoreTransferCommit::encoded_len);
            let event_bytes = events.iter().try_fold(0_usize, |total, event| {
                serde_json::to_vec(event.event.as_ref())
                    .ok()
                    .and_then(|encoded| total.checked_add(encoded.len()))
            });
            if transfer_bytes.is_none_or(|bytes| bytes > CORE_JOURNAL_TRANSFER_ENTRY_BYTE_LIMIT)
                || event_bytes.is_none_or(|bytes| bytes > CORE_JOURNAL_EVENT_BYTE_LIMIT)
            {
                Self::reset_journal(state, next_version.revision());
            } else if let (Some(transfer), Some(transfer_bytes), Some(event_bytes)) =
                (transfer, transfer_bytes, event_bytes)
            {
                state.journal_event_count = state.journal_event_count.saturating_add(events.len());
                state.journal_event_bytes = state.journal_event_bytes.saturating_add(event_bytes);
                state.journal_transfer_bytes =
                    state.journal_transfer_bytes.saturating_add(transfer_bytes);
                state.journal.push_back(CoreJournalEntry {
                    revision: next_version.revision(),
                    events: events.clone(),
                    transfer,
                    event_bytes,
                    transfer_bytes,
                });
                while state.journal.len() > revy_voxel_core::CoreTransferDelta::MAX_COMMITS
                    || state.journal_event_count > CORE_JOURNAL_EVENT_LIMIT
                    || state.journal_event_bytes > CORE_JOURNAL_EVENT_BYTE_LIMIT
                    || state.journal_transfer_bytes > CORE_JOURNAL_TRANSFER_BYTE_LIMIT
                {
                    let Some(expired) = state.journal.pop_front() else {
                        break;
                    };
                    state.journal_event_count = state
                        .journal_event_count
                        .saturating_sub(expired.events.len());
                    state.journal_event_bytes = state
                        .journal_event_bytes
                        .saturating_sub(expired.event_bytes);
                    state.journal_transfer_bytes = state
                        .journal_transfer_bytes
                        .saturating_sub(expired.transfer_bytes);
                    state.journal_floor = expired.revision;
                }
            }
        }
        state.active = next_version;
        Ok(events)
    }

    fn begin_journal(state: &mut CoreStoreState) {
        Self::reset_journal(state, state.active.revision());
        state.journal_active = true;
    }

    fn reset_journal(state: &mut CoreStoreState, floor: CoreRevision) {
        state.journal.clear();
        state.journal_event_count = 0;
        state.journal_event_bytes = 0;
        state.journal_transfer_bytes = 0;
        state.journal_floor = floor;
    }

    pub(crate) async fn player_summary(&self) -> PlayerSummary {
        self.version().await.player_summary()
    }

    pub(crate) async fn session_resync_events(&self, player_id: PlayerId) -> Vec<TargetedEvent> {
        self.version().await.session_resync_events(player_id)
    }

    pub(crate) async fn dirty(&self) -> bool {
        let state = self.state.lock().await;
        state
            .latest_dirty_revision
            .is_some_and(|revision| revision > state.persisted_revision)
    }

    pub(crate) fn world_dir(&self) -> &std::path::Path {
        &self.world_dir
    }

    pub(crate) async fn maybe_save(
        &self,
        reload_host: Option<&dyn RuntimePluginHost>,
    ) -> Result<(), RuntimeError> {
        let version = {
            let state = self.state.lock().await;
            let dirty = state
                .latest_dirty_revision
                .is_some_and(|revision| revision > state.persisted_revision);
            if !dirty {
                return Ok(());
            }
            Arc::clone(&state.active)
        };
        let saved_revision = version.revision();
        let storage_profile = Arc::clone(&self.storage_profile);
        let world_dir = self.world_dir.clone();
        let save_result = tokio::task::spawn_blocking(move || {
            let snapshot = version.snapshot();
            storage_profile.save_snapshot(&world_dir, &snapshot)
        })
        .await?;
        match save_result {
            Ok(()) => {
                let mut state = self.state.lock().await;
                if saved_revision > state.persisted_revision {
                    state.persisted_revision = saved_revision;
                }
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
}

impl CoreCandidatePlan {
    pub(crate) const fn base_revision(&self) -> CoreRevision {
        self.base_revision
    }

    pub(crate) fn materialize(&self, precopy: CorePrecopy) -> Arc<CoreStore> {
        let active = if let Some(config) = self.config.clone() {
            let mut mutation = CoreMutation::from_version(&precopy.version);
            mutation.reconfigure(config);
            mutation.prepare(Vec::new(), true).into_parts().0
        } else {
            precopy.version
        };
        let latest_dirty_revision = if self.config.is_some() {
            Some(active.revision())
        } else {
            precopy.latest_dirty_revision
        };
        Arc::new(CoreStore::from_handoff(
            CoreHandoff::from_committed(active),
            Arc::clone(&self.storage_profile),
            self.world_dir.clone(),
            precopy.persisted_revision,
            latest_dirty_revision,
        ))
    }
}
