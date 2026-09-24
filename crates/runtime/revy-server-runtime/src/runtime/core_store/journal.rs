use super::SharedCoreEvent;
use crate::RuntimeError;
use revy_voxel_core::{
    CoreRevision, CoreTransferCommit, CoreTransferDelta, CoreTransferDeltaDescriptor,
    CoreTransferError, EncodedCoreTransferCommit,
};
use std::collections::VecDeque;
use std::io::Write;

const RESYNC_EVENT_LIMIT: usize = 65_536;
const RESYNC_BYTE_LIMIT: usize = 32 * 1024 * 1024;
const PROCESS_ENTRY_BYTE_LIMIT: usize = 2 * 1024 * 1024;
pub(super) const PROCESS_BYTE_LIMIT: usize = 8 * 1024 * 1024;

/// Retention belongs to the exclusive cutover owner. Same-process reload retains only events
/// since its candidate snapshot; process pre-copy retains only encoded, adjacent commits.
/// Stopping revokes recording without freeing potentially large buffers inside the freeze.
pub(super) enum CoreJournal {
    Inactive,
    Resync(ResyncJournal),
    Process(ProcessJournal),
    StoppedResync(Vec<SharedCoreEvent>),
    StoppedProcess(VecDeque<EncodedCoreTransferCommit>),
}

pub(super) struct ResyncJournal {
    base_revision: CoreRevision,
    events: Vec<SharedCoreEvent>,
    event_bytes: usize,
    outcome: Result<(), RuntimeError>,
}

pub(super) struct ProcessJournal {
    floor: CoreRevision,
    commits: VecDeque<EncodedCoreTransferCommit>,
    bytes: usize,
    failure: Option<CoreTransferError>,
}

pub(super) enum ProcessDeltaSeal {
    Ready(CoreTransferDeltaDescriptor),
    Outpaced { earliest_revision: CoreRevision },
}

impl CoreJournal {
    pub(super) fn resync(base_revision: CoreRevision) -> Self {
        Self::Resync(ResyncJournal {
            base_revision,
            events: Vec::new(),
            event_bytes: 0,
            outcome: Ok(()),
        })
    }

    pub(super) fn process(base_revision: CoreRevision) -> Self {
        Self::Process(ProcessJournal {
            floor: base_revision,
            commits: VecDeque::new(),
            bytes: 0,
            failure: None,
        })
    }

    /// Journal failure invalidates the cutover, not the already validated gameplay commit.
    /// The cutover owner observes the first failure at seal; recording never retries it.
    pub(super) fn record(
        &mut self,
        revision: CoreRevision,
        events: &[SharedCoreEvent],
        transfer: CoreTransferCommit,
    ) {
        match self {
            Self::Resync(journal) => {
                if journal.outcome.is_ok() {
                    journal.outcome = journal.retain(revision, events);
                }
            }
            Self::Process(journal) => {
                if journal.failure.is_none() {
                    if let Err(error) = journal.retain(transfer) {
                        journal.failure = Some(error);
                    }
                }
            }
            Self::Inactive => {}
            Self::StoppedResync(events) => {
                drop(std::mem::take(events));
                *self = Self::Inactive;
            }
            Self::StoppedProcess(commits) => {
                drop(std::mem::take(commits));
                *self = Self::Inactive;
            }
        }
    }

    /// The single candidate owns all retained events. Sealing moves their buffer in O(1);
    /// revisions without events need no entry because the final core is an Arc capability.
    pub(super) fn seal_resync(
        &mut self,
        revision: CoreRevision,
    ) -> Result<Vec<SharedCoreEvent>, RuntimeError> {
        let journal = std::mem::replace(self, Self::Inactive);
        let Self::Resync(journal) = journal else {
            *self = journal;
            return Err(RuntimeError::Config(
                "core resync retention is not active".to_string(),
            ));
        };
        let ResyncJournal {
            base_revision,
            events,
            outcome,
            ..
        } = journal;
        if revision != base_revision {
            *self = Self::StoppedResync(events);
            return Err(RuntimeError::Config(format!(
                "resync base revision {} does not match candidate revision {}",
                revision.value(),
                base_revision.value(),
            )));
        }
        match outcome {
            Ok(()) => Ok(events),
            Err(error) => {
                *self = Self::StoppedResync(events);
                Err(error)
            }
        }
    }

    pub(super) fn write_process_delta(
        &self,
        revision: CoreRevision,
        writer: &mut impl Write,
    ) -> Result<ProcessDeltaSeal, CoreTransferError> {
        let Self::Process(journal) = self else {
            return Err(CoreTransferError::InvalidState(
                "process core retention is not active".to_string(),
            ));
        };
        if let Some(error) = &journal.failure {
            return Err(error.clone());
        }
        if revision < journal.floor {
            return Ok(ProcessDeltaSeal::Outpaced {
                earliest_revision: journal.floor,
            });
        }
        let commits = journal
            .commits
            .iter()
            .filter(|commit| commit.next_revision() > revision);
        CoreTransferDelta::seal_preencoded_into(revision, commits, writer)
            .map(ProcessDeltaSeal::Ready)
    }

    pub(super) fn stop(&mut self) {
        *self = match std::mem::replace(self, Self::Inactive) {
            Self::Resync(journal) => Self::StoppedResync(journal.events),
            Self::Process(journal) => Self::StoppedProcess(journal.commits),
            stopped => stopped,
        };
    }
}

impl ResyncJournal {
    fn retain(
        &mut self,
        revision: CoreRevision,
        events: &[SharedCoreEvent],
    ) -> Result<(), RuntimeError> {
        if events.is_empty() {
            return Ok(());
        }
        let outpaced = || RuntimeError::CutoverOutpaced {
            resource: "core resync events",
            staged_revision: self.base_revision.value(),
            earliest_revision: revision.value(),
        };
        if events.len() > RESYNC_EVENT_LIMIT - self.events.len() {
            return Err(outpaced());
        }
        // The byte budget uses the semantic event encoding size, without allocating a second
        // serialized copy of each event. Saturation remains larger than the finite budget.
        let mut size = EncodedEventSize(self.event_bytes);
        for event in events {
            serde_json::to_writer(&mut size, event.event.as_ref())
                .map_err(RuntimeError::CutoverEventEncoding)?;
            if size.0 > RESYNC_BYTE_LIMIT {
                return Err(outpaced());
            }
        }
        self.events
            .try_reserve(events.len())
            .map_err(|_error| RuntimeError::Allocation {
                resource: "core resync events",
                requested: (self.events.len() + events.len())
                    * std::mem::size_of::<SharedCoreEvent>(),
            })?;
        self.events.extend_from_slice(events);
        self.event_bytes = size.0;
        Ok(())
    }
}

struct EncodedEventSize(usize);

impl Write for EncodedEventSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl ProcessJournal {
    fn retain(&mut self, transfer: CoreTransferCommit) -> Result<(), CoreTransferError> {
        let revision = transfer.next_revision();
        let transfer = match transfer.encode(PROCESS_ENTRY_BYTE_LIMIT) {
            Ok(transfer) => transfer,
            Err(CoreTransferError::BudgetExceeded { .. }) => {
                self.commits.clear();
                self.bytes = 0;
                self.floor = revision;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let bytes = transfer.encoded_len();
        while self.commits.len() == CoreTransferDelta::MAX_COMMITS
            || bytes > PROCESS_BYTE_LIMIT - self.bytes
        {
            if let Some(expired) = self.commits.pop_front() {
                self.bytes -= expired.encoded_len();
                self.floor = expired.next_revision();
            }
        }
        self.commits
            .try_reserve(1)
            .map_err(|_error| CoreTransferError::Allocation {
                requested: (self.commits.len() + 1)
                    * std::mem::size_of::<EncodedCoreTransferCommit>(),
            })?;
        self.bytes += bytes;
        self.commits.push_back(transfer);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use revy_voxel_core::{ConnectionId, CoreEvent, EventTarget};
    use std::sync::Arc;

    #[test]
    fn resync_event_count_budget_accepts_its_boundary_then_reports_outpace() {
        let base = CoreRevision::initial();
        let mut journal = CoreJournal::resync(base);
        let event = SharedCoreEvent {
            target: EventTarget::Connection(ConnectionId(1)),
            event: Arc::new(CoreEvent::SelectedHotbarSlotChanged { slot: 7 }),
        };
        let CoreJournal::Resync(retention) = &mut journal else {
            unreachable!()
        };
        retention
            .retain(
                CoreRevision::from_value(1),
                &vec![event.clone(); RESYNC_EVENT_LIMIT],
            )
            .unwrap();
        assert!(retention.retain(CoreRevision::from_value(2), &[]).is_ok());
        let sealed = journal.seal_resync(base).unwrap();
        assert_eq!(sealed.len(), RESYNC_EVENT_LIMIT);
        assert!(Arc::ptr_eq(&sealed[0].event, &event.event));

        let mut journal = CoreJournal::resync(base);
        let CoreJournal::Resync(retention) = &mut journal else {
            unreachable!()
        };
        retention.outcome = retention.retain(
            CoreRevision::from_value(3),
            &vec![event; RESYNC_EVENT_LIMIT + 1],
        );
        assert!(matches!(
            journal.seal_resync(base),
            Err(RuntimeError::CutoverOutpaced {
                staged_revision: 0,
                earliest_revision: 3,
                ..
            })
        ));
    }

    #[test]
    fn resync_byte_budget_failure_retains_buffers_until_after_stop() {
        let base = CoreRevision::initial();
        let mut journal = CoreJournal::resync(base);
        let event = SharedCoreEvent {
            target: EventTarget::Connection(ConnectionId(1)),
            event: Arc::new(CoreEvent::SelectedHotbarSlotChanged { slot: 1 }),
        };
        let retained = Arc::downgrade(&event.event);
        let CoreJournal::Resync(retention) = &mut journal else {
            unreachable!()
        };
        retention
            .retain(CoreRevision::from_value(1), &[event])
            .unwrap();
        let oversized = SharedCoreEvent {
            target: EventTarget::Connection(ConnectionId(1)),
            event: Arc::new(CoreEvent::Disconnect {
                reason: "x".repeat(RESYNC_BYTE_LIMIT),
            }),
        };
        retention.outcome = retention.retain(CoreRevision::from_value(2), &[oversized]);
        assert!(matches!(
            journal.seal_resync(base),
            Err(RuntimeError::CutoverOutpaced { .. })
        ));
        journal.stop();
        assert!(
            retained.upgrade().is_some(),
            "abort must not release buffers during freeze"
        );
        drop(journal);
        assert!(retained.upgrade().is_none());
    }
}
