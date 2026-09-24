use super::selection::ResolvedRuntimeSelection;
use super::{ActiveGeneration, CoreStore, OnlineAuthKeys};
use crate::{
    CutoverReport, RuntimeError, RuntimeUpgradePhase, RuntimeUpgradeRole, RuntimeUpgradeStateView,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock as AsyncRwLock, watch};

pub(crate) struct RuntimeEpoch {
    revision: u64,
    pub(crate) selection: ResolvedRuntimeSelection,
    pub(crate) topology: Arc<ActiveGeneration>,
    pub(crate) core: Arc<CoreStore>,
    pub(crate) online_auth_keys: Option<Arc<OnlineAuthKeys>>,
}

impl RuntimeEpoch {
    pub(crate) fn initial(
        selection: ResolvedRuntimeSelection,
        topology: Arc<ActiveGeneration>,
        core: Arc<CoreStore>,
        online_auth_keys: Option<Arc<OnlineAuthKeys>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            revision: 1,
            selection,
            topology,
            core,
            online_auth_keys,
        })
    }

    pub(crate) fn candidate(
        active: &Self,
        selection: ResolvedRuntimeSelection,
        topology: Arc<ActiveGeneration>,
        core: Arc<CoreStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            revision: active.revision.saturating_add(1),
            selection,
            topology,
            core,
            online_auth_keys: active.online_auth_keys.clone(),
        })
    }

    pub(crate) fn transferred(
        revision: u64,
        selection: ResolvedRuntimeSelection,
        topology: Arc<ActiveGeneration>,
        core: Arc<CoreStore>,
        online_auth_keys: Option<Arc<OnlineAuthKeys>>,
    ) -> Result<Arc<Self>, RuntimeError> {
        if revision == 0 {
            return Err(RuntimeError::Config(
                "transferred runtime epoch revision must be non-zero".to_string(),
            ));
        }
        Ok(Arc::new(Self {
            revision,
            selection,
            topology,
            core,
            online_auth_keys,
        }))
    }

    pub(crate) const fn revision(&self) -> u64 {
        self.revision
    }
}

pub(crate) struct RuntimeAuthority {
    active: RwLock<Arc<RuntimeEpoch>>,
    data_plane: Arc<AsyncRwLock<()>>,
    epoch_latch: watch::Sender<u64>,
    last_cutover: RwLock<Option<CutoverReport>>,
    next_upgrade_status_id: AtomicU64,
    upgrade_status: RwLock<Option<(u64, RuntimeUpgradeStateView)>>,
}

impl RuntimeAuthority {
    pub(crate) fn new(initial: Arc<RuntimeEpoch>) -> Self {
        let initial_revision = initial.revision();
        Self {
            active: RwLock::new(initial),
            data_plane: Arc::new(AsyncRwLock::new(())),
            epoch_latch: watch::channel(initial_revision).0,
            last_cutover: RwLock::new(None),
            next_upgrade_status_id: AtomicU64::new(1),
            upgrade_status: RwLock::new(None),
        }
    }

    pub(crate) fn active(&self) -> Arc<RuntimeEpoch> {
        Arc::clone(
            &self
                .active
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub(crate) async fn enter_data_plane(&self) -> OwnedRwLockReadGuard<()> {
        Arc::clone(&self.data_plane).read_owned().await
    }

    pub(crate) fn subscribe_epoch(&self) -> watch::Receiver<u64> {
        self.epoch_latch.subscribe()
    }

    pub(crate) fn activate_epoch(&self, revision: u64) {
        self.epoch_latch.send_replace(revision);
    }

    pub(crate) async fn freeze(&self) -> FrozenDataPlane {
        // A queued writer prevents subsequent admissions while existing readers drain. That
        // interval is already a gameplay stall, not prepare time. Start before polling the write
        // acquisition so scheduling and drain latency cannot disappear from the report.
        let started_at = Instant::now();
        let guard = Arc::clone(&self.data_plane).write_owned().await;
        FrozenDataPlane {
            guard: Some(guard),
            started_at,
        }
    }

    pub(crate) fn last_cutover(&self) -> Option<CutoverReport> {
        self.last_cutover
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn record_cutover(&self, report: CutoverReport) {
        *self
            .last_cutover
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(report);
    }

    pub(crate) fn begin_executable_upgrade(
        &self,
        role: RuntimeUpgradeRole,
        phase: RuntimeUpgradePhase,
    ) -> Result<u64, RuntimeError> {
        let mut status = self
            .upgrade_status
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, active)) = *status {
            return Err(RuntimeError::Config(format!(
                "runtime upgrade is already active: role={:?} phase={:?}",
                active.role, active.phase
            )));
        }
        let id = self.next_upgrade_status_id.fetch_add(1, Ordering::AcqRel);
        *status = Some((id, RuntimeUpgradeStateView { role, phase }));
        Ok(id)
    }

    pub(crate) fn update_executable_upgrade(
        &self,
        id: u64,
        phase: RuntimeUpgradePhase,
    ) -> Result<(), RuntimeError> {
        let mut status = self
            .upgrade_status
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((active_id, active)) = status.as_mut() else {
            return Err(RuntimeError::Config(
                "runtime upgrade status authority is no longer active".to_string(),
            ));
        };
        if *active_id != id {
            return Err(RuntimeError::Config(
                "runtime upgrade status authority was superseded".to_string(),
            ));
        }
        active.phase = phase;
        Ok(())
    }

    pub(crate) fn finish_executable_upgrade(&self, id: u64) {
        let mut status = self
            .upgrade_status
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if status
            .as_ref()
            .is_some_and(|(active_id, _)| *active_id == id)
        {
            *status = None;
        }
    }

    pub(crate) fn executable_upgrade_status(&self) -> Option<RuntimeUpgradeStateView> {
        self.upgrade_status
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|(_, view)| *view)
    }
}

pub(crate) struct FrozenDataPlane {
    guard: Option<OwnedRwLockWriteGuard<()>>,
    started_at: Instant,
}

impl FrozenDataPlane {
    pub(crate) fn publish(&self, authority: &RuntimeAuthority, candidate: Arc<RuntimeEpoch>) {
        *authority
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = candidate;
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub(crate) fn resume(self) -> ResumedDataPlane {
        let mut frozen = self;
        drop(
            frozen
                .guard
                .take()
                .expect("frozen data-plane authority must be present until resume"),
        );
        ResumedDataPlane {
            started_at: frozen.started_at,
        }
    }
}

impl Drop for FrozenDataPlane {
    fn drop(&mut self) {
        let Some(guard) = self.guard.take() else {
            return;
        };
        // Losing a frozen cutover authority cannot prove whether an irreversible publication
        // occurred. Keeping the write guard alive prevents either process from silently restoring
        // parent data-plane authority; an explicit resume transition is the only rollback path.
        std::mem::forget(guard);
    }
}

pub(crate) struct ResumedDataPlane {
    started_at: Instant,
}

impl ResumedDataPlane {
    pub(crate) fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}
