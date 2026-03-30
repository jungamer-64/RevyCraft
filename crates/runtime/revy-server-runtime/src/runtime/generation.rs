use crate::RuntimeError;
use crate::config::ServerConfig;
use crate::runtime::ListenerBinding;
use crate::transport::AcceptedTransportSession;
use mc_plugin_host::registry::ProtocolRegistry;
use mc_proto_common::{ProtocolAdapter, TransportKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GenerationId(pub u64);

#[derive(Clone)]
pub(crate) struct ActiveGeneration {
    pub(crate) generation_id: GenerationId,
    pub(crate) config: ServerConfig,
    pub(crate) protocol_registry: ProtocolRegistry,
    pub(crate) default_adapter: Arc<dyn ProtocolAdapter>,
    pub(crate) default_bedrock_adapter: Option<Arc<dyn ProtocolAdapter>>,
    pub(crate) listener_bindings: Vec<ListenerBinding>,
}

#[derive(Clone)]
pub(crate) struct DrainingGeneration {
    pub(crate) generation: Arc<ActiveGeneration>,
    pub(crate) drain_deadline_ms: u64,
}

pub(crate) enum GenerationAdmission {
    Active(Arc<ActiveGeneration>),
    Draining(Arc<ActiveGeneration>),
    ExpiredDraining,
    Missing,
}

pub(crate) struct TopologyListenerWorker {
    pub(crate) transport: TransportKind,
    pub(crate) generation_tx: watch::Sender<GenerationId>,
    pub(crate) control_tx: mpsc::Sender<ListenerWorkerControl>,
    pub(crate) shutdown_tx: Option<oneshot::Sender<()>>,
    pub(crate) join_handle: Option<JoinHandle<()>>,
}

pub(crate) enum ListenerWorkerControl {
    Export {
        ack_tx: oneshot::Sender<Result<std::net::TcpListener, RuntimeError>>,
    },
}

pub(crate) struct RuntimeGenerationState {
    pub(crate) active: Arc<ActiveGeneration>,
    pub(crate) draining: Vec<DrainingGeneration>,
    pub(crate) listener_workers: HashMap<TransportKind, TopologyListenerWorker>,
    pub(crate) next_generation_id: u64,
}

pub(crate) struct AcceptedGenerationSession {
    pub(crate) generation_id: GenerationId,
    pub(crate) session: AcceptedTransportSession,
    pub(crate) queued_accept: QueuedAcceptGuard,
}

impl AcceptedGenerationSession {
    pub(crate) fn new(
        generation_id: GenerationId,
        session: AcceptedTransportSession,
        queued_accept: QueuedAcceptGuard,
    ) -> Self {
        Self {
            generation_id,
            session,
            queued_accept,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct QueuedAcceptTracker {
    counts: Arc<StdMutex<HashMap<GenerationId, usize>>>,
}

pub(crate) struct QueuedAcceptGuard {
    tracker: QueuedAcceptTracker,
    generation_id: Option<GenerationId>,
}

impl QueuedAcceptTracker {
    pub(crate) fn track(&self, generation_id: GenerationId) -> QueuedAcceptGuard {
        self.increment(generation_id);
        QueuedAcceptGuard {
            tracker: self.clone(),
            generation_id: Some(generation_id),
        }
    }

    pub(crate) fn increment(&self, generation_id: GenerationId) {
        let mut counts = self
            .counts
            .lock()
            .expect("queued accept tracker should not be poisoned");
        let entry = counts.entry(generation_id).or_insert(0);
        *entry = entry.saturating_add(1);
    }

    pub(crate) fn decrement(&self, generation_id: GenerationId) {
        let mut counts = self
            .counts
            .lock()
            .expect("queued accept tracker should not be poisoned");
        let Some(entry) = counts.get_mut(&generation_id) else {
            return;
        };
        *entry = entry.saturating_sub(1);
        if *entry == 0 {
            counts.remove(&generation_id);
        }
    }

    pub(crate) fn generation_ids(&self) -> HashSet<GenerationId> {
        self.counts
            .lock()
            .expect("queued accept tracker should not be poisoned")
            .keys()
            .copied()
            .collect()
    }

    pub(crate) fn total_count(&self) -> usize {
        self.counts
            .lock()
            .expect("queued accept tracker should not be poisoned")
            .values()
            .copied()
            .sum()
    }
}

impl Drop for QueuedAcceptGuard {
    fn drop(&mut self) {
        if let Some(generation_id) = self.generation_id.take() {
            self.tracker.decrement(generation_id);
        }
    }
}

pub(crate) fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .expect("current unix time in milliseconds should fit into u64")
}
