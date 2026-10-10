use super::selection::ResolvedRuntimeSelection;
use super::{ActiveGeneration, CoreStore, OnlineAuthKeys};
use crate::{
    CutoverReport, RuntimeError, RuntimeUpgradePhase, RuntimeUpgradeRole, RuntimeUpgradeStateView,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore, watch};

const DATA_PLANE_OPERATION_CAPACITY: u32 = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
enum AdmissionState {
    Open,
    Closed,
}

/// Counts operations that actually hold execution authority. Requests waiting for capacity
/// retain their input with the caller and cannot prolong a freeze by executing ahead of it.
struct DataPlaneGate {
    capacity: Arc<Semaphore>,
    admission: watch::Sender<AdmissionState>,
    freeze_serial: Arc<Mutex<()>>,
}

impl DataPlaneGate {
    fn new() -> Self {
        Self {
            capacity: Arc::new(Semaphore::new(
                usize::try_from(DATA_PLANE_OPERATION_CAPACITY)
                    .expect("the fixed operation capacity fits every supported target"),
            )),
            admission: watch::channel(AdmissionState::Open).0,
            freeze_serial: Arc::new(Mutex::new(())),
        }
    }

    async fn enter(&self) -> DataPlaneOperation {
        let mut admission = self.admission.subscribe();
        loop {
            if *admission.borrow_and_update() == AdmissionState::Closed {
                // This gate owns a sender for the complete borrowed acquisition lifetime.
                admission
                    .changed()
                    .await
                    .expect("admission owner remains alive");
                continue;
            }
            let permit = tokio::select! {
                biased;
                changed = admission.changed() => {
                    changed.expect("admission owner remains alive");
                    continue;
                }
                permit = Arc::clone(&self.capacity).acquire_owned() => {
                    // Capacity is private and is never closed; shutdown uses admission state.
                    permit.expect("data-plane capacity is never closed")
                }
            };
            if *admission.borrow_and_update() == AdmissionState::Open {
                return DataPlaneOperation { _permit: permit };
            }
            // A grant racing admission closure has not authorized any work. Return it before
            // waiting for the next open epoch, so quiescence drains only admitted operations.
            drop(permit);
        }
    }

    async fn close(&self) -> QuiescedDataPlane {
        let serial = Arc::clone(&self.freeze_serial).lock_owned().await;
        self.admission.send_replace(AdmissionState::Closed);
        let mut closing = ClosingAdmissions {
            closure: Some(AdmissionClosure {
                admission: self.admission.clone(),
                _serial: serial,
            }),
        };
        let permit = Arc::clone(&self.capacity)
            .acquire_many_owned(DATA_PLANE_OPERATION_CAPACITY)
            .await
            .expect("data-plane capacity is never closed");
        QuiescedDataPlane {
            permit,
            closure: closing.closure.take().expect("closing authority is owned"),
        }
    }
}

pub(crate) struct DataPlaneOperation {
    _permit: OwnedSemaphorePermit,
}

struct AdmissionClosure {
    admission: watch::Sender<AdmissionState>,
    _serial: OwnedMutexGuard<()>,
}

impl AdmissionClosure {
    fn reopen(self) {
        self.admission.send_replace(AdmissionState::Open);
    }
}

/// Cancellation before quiescence cannot have issued publication authority. It restores
/// admission; once quiescence is returned, only an explicit resume can restore admission.
struct ClosingAdmissions {
    closure: Option<AdmissionClosure>,
}

impl Drop for ClosingAdmissions {
    fn drop(&mut self) {
        if let Some(closure) = self.closure.take() {
            closure.reopen();
        }
    }
}

struct QuiescedDataPlane {
    permit: OwnedSemaphorePermit,
    closure: AdmissionClosure,
}

impl QuiescedDataPlane {
    fn resume(self) {
        drop(self.permit);
        self.closure.reopen();
    }
}

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
    active: Arc<RwLock<Arc<RuntimeEpoch>>>,
    data_plane: DataPlaneGate,
    epoch_latch: watch::Sender<u64>,
    last_cutover: RwLock<Option<CutoverReport>>,
    next_upgrade_status_id: AtomicU64,
    upgrade_status: RwLock<Option<(u64, RuntimeUpgradeStateView)>>,
}

impl RuntimeAuthority {
    pub(crate) fn new(initial: Arc<RuntimeEpoch>) -> Self {
        let initial_revision = initial.revision();
        Self {
            active: Arc::new(RwLock::new(initial)),
            data_plane: DataPlaneGate::new(),
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

    pub(crate) async fn enter_data_plane(&self) -> DataPlaneOperation {
        self.data_plane.enter().await
    }

    pub(crate) fn subscribe_epoch(&self) -> watch::Receiver<u64> {
        self.epoch_latch.subscribe()
    }

    pub(crate) fn activate_epoch(&self, revision: u64) {
        self.epoch_latch.send_replace(revision);
    }

    pub(crate) async fn freeze(&self) -> FrozenDataPlane {
        let started_at = Instant::now();
        let guard = self.data_plane.close().await;
        FrozenDataPlane {
            guard: Some(guard),
            publication: Arc::clone(&self.active),
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
    guard: Option<QuiescedDataPlane>,
    publication: Arc<RwLock<Arc<RuntimeEpoch>>>,
    started_at: Instant,
}

impl FrozenDataPlane {
    pub(crate) fn publish(&self, candidate: Arc<RuntimeEpoch>) {
        *self
            .publication
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = candidate;
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub(crate) fn resume(self) -> ResumedDataPlane {
        let mut frozen = self;
        frozen
            .guard
            .take()
            .expect("frozen data-plane authority must be present until resume")
            .resume();
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
        // occurred. Keeping the quiescence authority alive prevents either process from restoring
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

#[cfg(test)]
mod gate_tests {
    use super::*;
    use tokio::sync::mpsc;
    use tokio::task::JoinSet;

    #[tokio::test]
    async fn capacity_waiters_do_not_execute_ahead_of_freeze() {
        let gate = Arc::new(DataPlaneGate::new());
        let mut admitted = Vec::new();
        for _ in 0..DATA_PLANE_OPERATION_CAPACITY {
            admitted.push(gate.enter().await);
        }
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let (entered_tx, mut entered_rx) = mpsc::unbounded_channel();
        let (release_tx, release_rx) = watch::channel(false);
        let mut waiting = JoinSet::new();
        let waiting_count = DATA_PLANE_OPERATION_CAPACITY * 4;
        for _ in 0..waiting_count {
            let gate = Arc::clone(&gate);
            let started_tx = started_tx.clone();
            let entered_tx = entered_tx.clone();
            let mut release = release_rx.clone();
            waiting.spawn(async move {
                started_tx.send(()).expect("start observer is alive");
                let operation = gate.enter().await;
                entered_tx.send(()).expect("admission observer is alive");
                while !*release.borrow_and_update() {
                    release.changed().await.expect("release owner is alive");
                }
                drop(operation);
            });
        }
        for _ in 0..waiting_count {
            started_rx.recv().await.expect("every waiter starts");
        }
        let closing = gate.close();
        tokio::pin!(closing);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut closing)
                .await
                .is_err()
        );
        drop(admitted);
        let quiesced = tokio::time::timeout(Duration::from_secs(1), closing)
            .await
            .expect("only admitted operations need to drain");
        assert!(entered_rx.try_recv().is_err());
        quiesced.resume();
        release_tx.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), async {
            while let Some(joined) = waiting.join_next().await {
                joined.expect("waiting operation finishes");
            }
        })
        .await
        .expect("every retained request resumes");
        for _ in 0..waiting_count {
            entered_rx.recv().await.expect("every request was admitted");
        }
    }

    #[tokio::test]
    async fn cancelling_before_quiescence_restores_admission() {
        let gate = DataPlaneGate::new();
        let operation = gate.enter().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), gate.close())
                .await
                .is_err()
        );
        drop(
            tokio::time::timeout(Duration::from_secs(1), gate.enter())
                .await
                .expect("unpublished cancellation restores admission"),
        );
        drop(operation);
        gate.close().await.resume();
    }
}
