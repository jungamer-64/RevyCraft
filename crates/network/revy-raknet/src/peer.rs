use crate::RakNetError;
use crate::budget::{RakNetBudgets, ensure_budget};
use crate::sequence::{DatagramSequence, OrderSequence, ReliableSequence};
use crate::wire::{
    Acknowledgement, Frame, Reliability, SplitHeader, decode_acknowledgement,
    decode_connected_ping, decode_connection_request, decode_datagram, encode_ack,
    encode_connected_pong, encode_connection_accept, encode_datagram, encode_nack,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::Instant;

const RETRANSMIT_BASE_DELAY: Duration = Duration::from_millis(200);
const RETRANSMIT_MAX_BACKOFF_SHIFT: u32 = 3;
const PEER_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_RETRANSMIT_ATTEMPTS: u16 = 60;
const DATAGRAM_HEADER_BUDGET: usize = 64;
const SNAPSHOT_VALIDATION_WORKER_LIMIT: usize = 16;

pub struct Running;
pub struct Frozen;
pub struct Transferred;

pub struct RakNetPeer<State> {
    remote_addr: SocketAddr,
    command_tx: mpsc::Sender<PeerCommand>,
    inbox: Arc<PeerInbox>,
    transfer_snapshot: Option<Arc<PeerSnapshot>>,
    router_owned: Arc<AtomicBool>,
    state: PhantomData<State>,
}

impl<State> RakNetPeer<State> {
    #[must_use]
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    pub(crate) fn claim_session_authority(&self) {
        self.router_owned.store(false, Ordering::Release);
    }
}

#[derive(Clone)]
pub struct RakNetSender {
    command_tx: mpsc::Sender<PeerCommand>,
}

impl RakNetSender {
    pub async fn send_raw(&self, payload: &[u8]) -> Result<(), RakNetError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(PeerCommand::Send {
                payload: payload.to_vec(),
                reply_tx,
            })
            .await
            .map_err(|_| RakNetError::Closed)?;
        reply_rx.await.map_err(|_| RakNetError::Closed)?
    }
}

impl RakNetPeer<Running> {
    pub async fn recv_raw(&mut self) -> Result<Vec<u8>, RakNetError> {
        self.inbox.receive().await
    }

    pub async fn send_raw(&mut self, payload: &[u8]) -> Result<(), RakNetError> {
        self.sender().send_raw(payload).await
    }

    #[must_use]
    pub fn sender(&self) -> RakNetSender {
        RakNetSender {
            command_tx: self.command_tx.clone(),
        }
    }

    pub async fn freeze(self) -> Result<RakNetPeer<Frozen>, RakNetError> {
        freeze_control(&self.command_tx).await?;
        Ok(RakNetPeer {
            remote_addr: self.remote_addr,
            command_tx: self.command_tx,
            inbox: self.inbox,
            transfer_snapshot: None,
            router_owned: self.router_owned,
            state: PhantomData,
        })
    }
}

impl RakNetPeer<Frozen> {
    #[must_use]
    pub fn snapshot(&self) -> &PeerSnapshot {
        self.transfer_snapshot
            .as_ref()
            .expect("transfer-sealed peer should own its snapshot")
    }

    #[must_use]
    pub fn sender(&self) -> RakNetSender {
        RakNetSender {
            command_tx: self.command_tx.clone(),
        }
    }

    #[must_use]
    pub async fn seal_for_transfer(mut self) -> Result<Self, RakNetError> {
        if self.transfer_snapshot.is_none() {
            self.transfer_snapshot = Some(Arc::new(seal_transfer_control(&self.command_tx).await?));
        }
        Ok(self)
    }

    pub fn resume(self) -> Result<RakNetPeer<Running>, RakNetError> {
        let RakNetPeer {
            remote_addr,
            command_tx,
            inbox,
            transfer_snapshot,
            router_owned,
            state: _,
        } = self;
        drop(transfer_snapshot);
        resume_control(&command_tx)?;
        Ok(RakNetPeer {
            remote_addr,
            command_tx,
            inbox,
            transfer_snapshot: None,
            router_owned,
            state: PhantomData,
        })
    }

    pub async fn mark_transferred(mut self) -> Result<RakNetPeer<Transferred>, RakNetError> {
        if self.transfer_snapshot.is_none() {
            return Err(RakNetError::Wire(
                "RakNet peer transfer was not sealed".to_string(),
            ));
        }
        transfer_control(&self.command_tx).await?;
        Ok(RakNetPeer {
            remote_addr: self.remote_addr,
            command_tx: self.command_tx,
            inbox: self.inbox,
            transfer_snapshot: self.transfer_snapshot.take(),
            router_owned: self.router_owned,
            state: PhantomData,
        })
    }
}

impl RakNetPeer<Transferred> {
    #[must_use]
    pub fn snapshot(&self) -> &PeerSnapshot {
        self.transfer_snapshot
            .as_ref()
            .expect("transferred peer should retain its sealed snapshot")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerSnapshot {
    pub remote_addr: SocketAddr,
    pub mtu: u16,
    pub accepted: bool,
    pub next_datagram: DatagramSequence,
    pub next_reliable: ReliableSequence,
    pub next_ordered: OrderSequence,
    pub next_split_id: u16,
    pub last_datagram: Option<DatagramSequence>,
    pub seen_datagram_order: Vec<DatagramSequence>,
    pub seen_reliable_order: Vec<ReliableSequence>,
    pub ordered_expected: Vec<(u8, OrderSequence)>,
    pub sequenced_latest: Vec<(u8, OrderSequence)>,
    pub queued_payloads: Vec<Vec<u8>>,
    pub unacked_datagrams: Vec<UnackedDatagramSnapshot>,
    pub reassembly: Vec<ReassemblySnapshot>,
    pub ordered_holdback: Vec<OrderedPayloadSnapshot>,
    pub timeout_remaining: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnackedDatagramSnapshot {
    pub sequence: DatagramSequence,
    pub bytes: Vec<u8>,
    pub retransmit_remaining: Duration,
    pub attempts: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReassemblySnapshot {
    pub split_id: u16,
    pub parts: Vec<Option<Vec<u8>>>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OrderedPayloadSnapshot {
    pub channel: u8,
    pub sequence: OrderSequence,
    pub payload: Vec<u8>,
}

impl PeerSnapshot {
    /// Validates a process-transfer checkpoint against the child runtime's resource policy.
    ///
    /// # Errors
    ///
    /// Returns [`RakNetError`] when the checkpoint is structurally inconsistent or exceeds a
    /// configured peer, queue, datagram, reassembly, holdback, or timer budget.
    fn validate_structure(
        &self,
        budgets: RakNetBudgets,
    ) -> Result<PeerDuplicateIndexes, RakNetError> {
        let maximum_mtu = u16::try_from(budgets.max_datagram_bytes)
            .unwrap_or(u16::MAX)
            .max(576);
        if !(576..=maximum_mtu).contains(&self.mtu) {
            return Err(RakNetError::Wire(format!(
                "transferred peer MTU {} is outside 576..={maximum_mtu}",
                self.mtu
            )));
        }
        ensure_budget(
            "application queue entries",
            self.queued_payloads.len(),
            budgets.application_queue,
        )?;
        for payload in &self.queued_payloads {
            ensure_budget(
                "application payload bytes",
                payload.len(),
                budgets.max_payload_bytes,
            )?;
        }
        ensure_budget(
            "unacknowledged datagrams",
            self.unacked_datagrams.len(),
            budgets.max_unacked_datagrams,
        )?;
        let mut unacked = HashSet::with_capacity(self.unacked_datagrams.len());
        for datagram in &self.unacked_datagrams {
            if !unacked.insert(datagram.sequence) {
                return Err(RakNetError::Wire(
                    "transferred peer repeats an unacknowledged datagram sequence".to_string(),
                ));
            }
            ensure_budget(
                "datagram bytes",
                datagram.bytes.len(),
                budgets.max_datagram_bytes,
            )?;
            if datagram.retransmit_remaining > retransmit_delay(datagram.attempts)
                || datagram.attempts == 0
                || datagram.attempts > MAX_RETRANSMIT_ATTEMPTS
            {
                return Err(RakNetError::Wire(
                    "transferred peer has an invalid retransmit timer or attempt count".to_string(),
                ));
            }
        }
        let mut split_ids = HashSet::with_capacity(self.reassembly.len());
        let mut reassembly_parts = 0_usize;
        let mut reassembly_bytes = 0_usize;
        for split in &self.reassembly {
            if !split_ids.insert(split.split_id) || split.parts.is_empty() {
                return Err(RakNetError::Wire(
                    "transferred peer has an invalid or duplicate fragment assembly".to_string(),
                ));
            }
            reassembly_parts = reassembly_parts
                .checked_add(split.parts.len())
                .ok_or_else(|| RakNetError::Wire("fragment part count overflow".to_string()))?;
            for part in split.parts.iter().flatten() {
                ensure_budget(
                    "application payload bytes",
                    part.len(),
                    budgets.max_payload_bytes,
                )?;
                reassembly_bytes = reassembly_bytes
                    .checked_add(part.len())
                    .ok_or_else(|| RakNetError::Wire("fragment byte count overflow".to_string()))?;
            }
        }
        ensure_budget(
            "reassembly parts per peer",
            reassembly_parts,
            budgets.max_reassembly_parts_per_peer,
        )?;
        ensure_budget(
            "reassembly bytes per peer",
            reassembly_bytes,
            budgets.max_reassembly_bytes_per_peer,
        )?;
        ensure_budget(
            "ordered holdback entries",
            self.ordered_holdback.len(),
            budgets.max_ordered_holdback,
        )?;
        let mut held_sequences = HashSet::with_capacity(self.ordered_holdback.len());
        for held in &self.ordered_holdback {
            if !held_sequences.insert((held.channel, held.sequence)) {
                return Err(RakNetError::Wire(
                    "transferred peer repeats an ordered holdback sequence".to_string(),
                ));
            }
            ensure_budget(
                "application payload bytes",
                held.payload.len(),
                budgets.max_payload_bytes,
            )?;
        }
        ensure_budget(
            "datagram duplicate window",
            self.seen_datagram_order.len(),
            duplicate_window_limit(budgets),
        )?;
        let seen_datagrams = self
            .seen_datagram_order
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        if seen_datagrams.len() != self.seen_datagram_order.len() {
            return Err(RakNetError::Wire(
                "transferred peer repeats a datagram duplicate-window sequence".to_string(),
            ));
        }
        ensure_budget(
            "reliable duplicate window",
            self.seen_reliable_order.len(),
            duplicate_window_limit(budgets),
        )?;
        let seen = self
            .seen_reliable_order
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        if seen.len() != self.seen_reliable_order.len() {
            return Err(RakNetError::Wire(
                "transferred peer repeats a reliable duplicate-window sequence".to_string(),
            ));
        }
        validate_channel_sequences("ordered expected", &self.ordered_expected)?;
        validate_channel_sequences("sequenced latest", &self.sequenced_latest)?;
        if self.timeout_remaining > PEER_TIMEOUT {
            return Err(RakNetError::Wire(
                "transferred peer timeout exceeds the protocol timeout".to_string(),
            ));
        }
        Ok(PeerDuplicateIndexes {
            datagrams: seen_datagrams,
            reliable: seen,
        })
    }

    /// Consumes a process-transfer checkpoint after validating it against a concrete resource
    /// policy. The returned capability retains the policy identity so a listener cannot import a
    /// checkpoint admitted under weaker budgets.
    ///
    /// # Errors
    ///
    /// Returns [`RakNetError`] when the checkpoint is structurally inconsistent or exceeds any
    /// configured peer-state budget.
    pub fn into_validated(
        self,
        budgets: RakNetBudgets,
    ) -> Result<ValidatedPeerSnapshot, RakNetError> {
        let duplicate_indexes = self.validate_structure(budgets)?;
        Ok(ValidatedPeerSnapshot {
            checkpoint: self,
            duplicate_indexes,
            budgets,
        })
    }
}

/// A checkpoint validated for one resource policy. Validation builds the duplicate indexes that
/// the receiving actor will own; importing consumes both the checkpoint and those indexes.
pub struct ValidatedPeerSnapshot {
    checkpoint: PeerSnapshot,
    duplicate_indexes: PeerDuplicateIndexes,
    budgets: RakNetBudgets,
}

struct PeerDuplicateIndexes {
    datagrams: HashSet<DatagramSequence>,
    reliable: HashSet<ReliableSequence>,
}

impl ValidatedPeerSnapshot {
    #[must_use]
    pub fn remote_addr(&self) -> SocketAddr {
        self.checkpoint.remote_addr
    }

    pub(crate) const fn budgets(&self) -> RakNetBudgets {
        self.budgets
    }
}

/// Validates a batch with bounded parallelism, retaining input order. No actor or socket authority
/// is created by validation; the listener separately admits the resulting checkpoints.
///
/// # Errors
///
/// Returns the checkpoint error or worker failure without a partially validated batch.
pub fn validate_peer_snapshots(
    snapshots: Vec<PeerSnapshot>,
    budgets: RakNetBudgets,
) -> Result<Vec<ValidatedPeerSnapshot>, RakNetError> {
    if snapshots.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(SNAPSHOT_VALIDATION_WORKER_LIMIT)
        .min(snapshots.len());
    let chunk_size = snapshots.len().div_ceil(worker_count);
    let mut snapshots = snapshots.into_iter();
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(worker_count);
        loop {
            let chunk = snapshots.by_ref().take(chunk_size).collect::<Vec<_>>();
            if chunk.is_empty() {
                break;
            }
            workers.push(scope.spawn(move || {
                chunk
                    .into_iter()
                    .map(|snapshot| snapshot.into_validated(budgets))
                    .collect::<Result<Vec<_>, _>>()
            }));
        }
        let mut validated = Vec::new();
        for worker in workers {
            let mut chunk = worker.join().map_err(|_| {
                RakNetError::Wire("RakNet snapshot validation worker panicked".to_string())
            })??;
            validated.append(&mut chunk);
        }
        Ok(validated)
    })
}

fn validate_channel_sequences(
    name: &str,
    sequences: &[(u8, OrderSequence)],
) -> Result<(), RakNetError> {
    let mut channels = HashSet::with_capacity(sequences.len());
    if sequences
        .iter()
        .any(|(channel, _)| !channels.insert(*channel))
    {
        return Err(RakNetError::Wire(format!(
            "transferred peer repeats a {name} channel"
        )));
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) struct PeerControl {
    ingress_tx: mpsc::Sender<Vec<u8>>,
    command_tx: mpsc::Sender<PeerCommand>,
    activity: Arc<PeerActivity>,
    router_owned: Arc<AtomicBool>,
}

impl PeerControl {
    pub(crate) fn is_router_owned(&self) -> bool {
        self.router_owned.load(Ordering::Acquire)
    }

    pub(crate) fn deliver(&self, bytes: Vec<u8>) -> Result<(), RakNetError> {
        match self.ingress_tx.try_send(bytes) {
            Ok(()) => {
                self.activity.observe(Instant::now());
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => Err(RakNetError::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(RakNetError::Closed),
        }
    }

    pub(crate) async fn freeze(&self) -> Result<(), RakNetError> {
        freeze_control(&self.command_tx).await
    }

    pub(crate) async fn freeze_for_transfer(&self) -> Result<PeerSnapshot, RakNetError> {
        self.freeze().await?;
        seal_transfer_control(&self.command_tx).await
    }

    pub(crate) fn resume(&self) -> Result<(), RakNetError> {
        resume_control(&self.command_tx)
    }

    pub(crate) async fn mark_transferred(&self) -> Result<(), RakNetError> {
        transfer_control(&self.command_tx).await
    }
}

async fn freeze_control(command_tx: &mpsc::Sender<PeerCommand>) -> Result<(), RakNetError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    command_tx
        .send(PeerCommand::Freeze { reply_tx })
        .await
        .map_err(|_| RakNetError::Closed)?;
    reply_rx.await.map_err(|_| RakNetError::Closed)
}

async fn seal_transfer_control(
    command_tx: &mpsc::Sender<PeerCommand>,
) -> Result<PeerSnapshot, RakNetError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    command_tx
        .send(PeerCommand::SealTransfer { reply_tx })
        .await
        .map_err(|_| RakNetError::Closed)?;
    reply_rx.await.map_err(|_| RakNetError::Closed)
}

fn resume_control(command_tx: &mpsc::Sender<PeerCommand>) -> Result<(), RakNetError> {
    command_tx
        .try_send(PeerCommand::Resume)
        .map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => RakNetError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => RakNetError::Closed,
        })
}

async fn transfer_control(command_tx: &mpsc::Sender<PeerCommand>) -> Result<(), RakNetError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    command_tx
        .send(PeerCommand::Transfer { reply_tx })
        .await
        .map_err(|_| RakNetError::Closed)?;
    reply_rx.await.map_err(|_| RakNetError::Closed)
}

pub(crate) struct PeerSpawn {
    pub control: PeerControl,
    pub imported: Option<RakNetPeer<Frozen>>,
}

pub(crate) fn spawn_peer(
    socket: Arc<UdpSocket>,
    remote_addr: SocketAddr,
    mtu: u16,
    budgets: RakNetBudgets,
    accepted_tx: mpsc::Sender<RakNetPeer<Running>>,
    closed_tx: mpsc::UnboundedSender<SocketAddr>,
) -> PeerSpawn {
    let (ingress_tx, ingress_rx) = mpsc::channel(budgets.peer_ingress_queue);
    let (command_tx, command_rx) = mpsc::channel(budgets.peer_command_queue);
    let inbox = Arc::new(PeerInbox::new(budgets.application_queue));
    let activity = Arc::new(PeerActivity::new(Instant::now()));
    let router_owned = Arc::new(AtomicBool::new(true));
    let control = PeerControl {
        ingress_tx,
        command_tx: command_tx.clone(),
        activity: Arc::clone(&activity),
        router_owned: Arc::clone(&router_owned),
    };
    let actor = PeerActor::new(
        socket,
        remote_addr,
        mtu,
        budgets,
        accepted_tx,
        command_tx,
        inbox,
        activity,
        router_owned,
    );
    tokio::spawn(async move {
        actor.run(ingress_rx, command_rx).await;
        let _ = closed_tx.send(remote_addr);
    });
    PeerSpawn {
        control,
        imported: None,
    }
}

pub(crate) struct PreparedPeerSpawn {
    remote_addr: SocketAddr,
    control: PeerControl,
    imported: Option<RakNetPeer<Frozen>>,
    actor: PeerActor,
    frozen_at: Instant,
    ingress_rx: mpsc::Receiver<Vec<u8>>,
    command_rx: mpsc::Receiver<PeerCommand>,
}

impl PreparedPeerSpawn {
    pub(crate) const fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    pub(crate) fn spawn(self, closed_tx: mpsc::UnboundedSender<SocketAddr>) -> PeerSpawn {
        let Self {
            remote_addr,
            control,
            imported,
            actor,
            frozen_at,
            ingress_rx,
            command_rx,
        } = self;
        tokio::spawn(async move {
            actor.run_frozen(ingress_rx, command_rx, frozen_at).await;
            let _ = closed_tx.send(remote_addr);
        });
        PeerSpawn { control, imported }
    }
}

pub(crate) fn prepare_peer_snapshots(
    socket: Arc<UdpSocket>,
    snapshots: Vec<ValidatedPeerSnapshot>,
    budgets: RakNetBudgets,
    accepted_tx: mpsc::Sender<RakNetPeer<Running>>,
) -> Result<Vec<PreparedPeerSpawn>, RakNetError> {
    if snapshots.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(SNAPSHOT_VALIDATION_WORKER_LIMIT)
        .min(snapshots.len());
    let chunk_size = snapshots.len().div_ceil(worker_count);
    let mut chunks = Vec::with_capacity(worker_count);
    let mut snapshots = snapshots.into_iter();
    loop {
        let chunk = snapshots.by_ref().take(chunk_size).collect::<Vec<_>>();
        if chunk.is_empty() {
            break;
        }
        chunks.push(chunk);
    }
    std::thread::scope(|scope| {
        let workers = chunks
            .into_iter()
            .map(|chunk| {
                let socket = Arc::clone(&socket);
                let accepted_tx = accepted_tx.clone();
                scope.spawn(move || {
                    chunk
                        .into_iter()
                        .map(|snapshot| {
                            prepare_peer_snapshot(
                                Arc::clone(&socket),
                                snapshot,
                                budgets,
                                accepted_tx.clone(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        let mut prepared = Vec::new();
        for worker in workers {
            let mut peers = worker.join().map_err(|_| {
                RakNetError::Wire("RakNet snapshot prepare worker panicked".to_string())
            })?;
            prepared.append(&mut peers);
        }
        Ok(prepared)
    })
}

fn prepare_peer_snapshot(
    socket: Arc<UdpSocket>,
    snapshot: ValidatedPeerSnapshot,
    budgets: RakNetBudgets,
    accepted_tx: mpsc::Sender<RakNetPeer<Running>>,
) -> PreparedPeerSpawn {
    let ValidatedPeerSnapshot {
        checkpoint: mut snapshot,
        duplicate_indexes,
        budgets: _,
    } = snapshot;
    let remote_addr = snapshot.remote_addr;
    let (ingress_tx, ingress_rx) = mpsc::channel(budgets.peer_ingress_queue);
    let (command_tx, command_rx) = mpsc::channel(budgets.peer_command_queue);
    let inbox = Arc::new(PeerInbox::from_validated_queue(
        std::mem::take(&mut snapshot.queued_payloads),
        budgets.application_queue,
    ));
    let router_owned = Arc::new(AtomicBool::new(true));
    let idle_elapsed = PEER_TIMEOUT.saturating_sub(snapshot.timeout_remaining);
    let now = Instant::now();
    let activity = Arc::new(PeerActivity::new(
        now.checked_sub(idle_elapsed).unwrap_or(now),
    ));
    let control = PeerControl {
        ingress_tx,
        command_tx: command_tx.clone(),
        activity: Arc::clone(&activity),
        router_owned: Arc::clone(&router_owned),
    };
    let imported = snapshot.accepted.then(|| RakNetPeer {
        remote_addr,
        command_tx: command_tx.clone(),
        inbox: Arc::clone(&inbox),
        transfer_snapshot: None,
        router_owned: Arc::clone(&router_owned),
        state: PhantomData,
    });
    let actor = PeerActor::from_validated_snapshot(
        socket,
        snapshot,
        duplicate_indexes,
        now,
        budgets,
        accepted_tx,
        command_tx,
        inbox,
        activity,
        router_owned,
    );
    PreparedPeerSpawn {
        remote_addr,
        control,
        imported,
        actor,
        frozen_at: now,
        ingress_rx,
        command_rx,
    }
}

enum PeerCommand {
    Send {
        payload: Vec<u8>,
        reply_tx: oneshot::Sender<Result<(), RakNetError>>,
    },
    Freeze {
        reply_tx: oneshot::Sender<()>,
    },
    SealTransfer {
        reply_tx: oneshot::Sender<PeerSnapshot>,
    },
    Resume,
    Transfer {
        reply_tx: oneshot::Sender<()>,
    },
}

struct PeerActor {
    socket: Arc<UdpSocket>,
    remote_addr: SocketAddr,
    mtu: u16,
    budgets: RakNetBudgets,
    accepted_tx: mpsc::Sender<RakNetPeer<Running>>,
    public_command_tx: mpsc::Sender<PeerCommand>,
    inbox: Arc<PeerInbox>,
    activity: Arc<PeerActivity>,
    accepted: bool,
    router_owned: Arc<AtomicBool>,
    next_datagram: DatagramSequence,
    next_reliable: ReliableSequence,
    next_ordered: OrderSequence,
    next_split_id: u16,
    last_datagram: Option<DatagramSequence>,
    seen_datagrams: HashSet<DatagramSequence>,
    seen_datagram_order: VecDeque<DatagramSequence>,
    seen_reliable: HashSet<ReliableSequence>,
    seen_reliable_order: VecDeque<ReliableSequence>,
    ordered_expected: HashMap<u8, OrderSequence>,
    sequenced_latest: HashMap<u8, OrderSequence>,
    ordered_holdback: HashMap<(u8, OrderSequence), Vec<u8>>,
    reassembly: HashMap<u16, Reassembly>,
    reassembly_bytes: usize,
    reassembly_parts: usize,
    unacked: HashMap<DatagramSequence, UnackedDatagram>,
}

struct PeerActivity {
    last_observed: Mutex<Instant>,
}

impl PeerActivity {
    fn new(last_observed: Instant) -> Self {
        Self {
            last_observed: Mutex::new(last_observed),
        }
    }

    fn observe(&self, observed_at: Instant) {
        let mut current = self
            .last_observed
            .lock()
            .expect("raknet peer activity lock should not be poisoned");
        if observed_at > *current {
            *current = observed_at;
        }
    }

    fn last_observed(&self) -> Instant {
        *self
            .last_observed
            .lock()
            .expect("raknet peer activity lock should not be poisoned")
    }

    fn resume(&self, frozen_at: Instant, resumed_at: Instant) {
        let mut observed = self
            .last_observed
            .lock()
            .expect("raknet peer activity lock should not be poisoned");
        let idle_elapsed = frozen_at.saturating_duration_since(*observed);
        *observed = resumed_at.checked_sub(idle_elapsed).unwrap_or(resumed_at);
    }
}

struct UnackedDatagram {
    bytes: Vec<u8>,
    sent_at: Instant,
    attempts: u16,
}

struct Reassembly {
    parts: Vec<Option<Vec<u8>>>,
}

enum PeerExecution {
    Running,
    Frozen { since: Instant },
}

impl PeerActor {
    fn new(
        socket: Arc<UdpSocket>,
        remote_addr: SocketAddr,
        mtu: u16,
        budgets: RakNetBudgets,
        accepted_tx: mpsc::Sender<RakNetPeer<Running>>,
        public_command_tx: mpsc::Sender<PeerCommand>,
        inbox: Arc<PeerInbox>,
        activity: Arc<PeerActivity>,
        router_owned: Arc<AtomicBool>,
    ) -> Self {
        Self {
            socket,
            remote_addr,
            mtu,
            budgets,
            accepted_tx,
            public_command_tx,
            inbox,
            activity,
            accepted: false,
            router_owned,
            next_datagram: DatagramSequence::default(),
            next_reliable: ReliableSequence::default(),
            next_ordered: OrderSequence::default(),
            next_split_id: 0,
            last_datagram: None,
            seen_datagrams: HashSet::new(),
            seen_datagram_order: VecDeque::new(),
            seen_reliable: HashSet::new(),
            seen_reliable_order: VecDeque::new(),
            ordered_expected: HashMap::new(),
            sequenced_latest: HashMap::new(),
            ordered_holdback: HashMap::new(),
            reassembly: HashMap::new(),
            reassembly_bytes: 0,
            reassembly_parts: 0,
            unacked: HashMap::new(),
        }
    }

    fn from_validated_snapshot(
        socket: Arc<UdpSocket>,
        snapshot: PeerSnapshot,
        duplicate_indexes: PeerDuplicateIndexes,
        restored_at: Instant,
        budgets: RakNetBudgets,
        accepted_tx: mpsc::Sender<RakNetPeer<Running>>,
        public_command_tx: mpsc::Sender<PeerCommand>,
        inbox: Arc<PeerInbox>,
        activity: Arc<PeerActivity>,
        router_owned: Arc<AtomicBool>,
    ) -> Self {
        let mut actor = Self {
            socket,
            remote_addr: snapshot.remote_addr,
            mtu: snapshot.mtu,
            budgets,
            accepted_tx,
            public_command_tx,
            inbox,
            activity,
            accepted: snapshot.accepted,
            router_owned,
            next_datagram: snapshot.next_datagram,
            next_reliable: snapshot.next_reliable,
            next_ordered: snapshot.next_ordered,
            next_split_id: snapshot.next_split_id,
            last_datagram: snapshot.last_datagram,
            seen_datagrams: duplicate_indexes.datagrams,
            seen_datagram_order: snapshot.seen_datagram_order.into(),
            seen_reliable: duplicate_indexes.reliable,
            seen_reliable_order: snapshot.seen_reliable_order.into(),
            ordered_expected: HashMap::new(),
            sequenced_latest: HashMap::new(),
            ordered_holdback: HashMap::new(),
            reassembly: HashMap::new(),
            reassembly_bytes: 0,
            reassembly_parts: 0,
            unacked: HashMap::new(),
        };
        actor.ordered_expected = snapshot.ordered_expected.into_iter().collect();
        actor.sequenced_latest = snapshot.sequenced_latest.into_iter().collect();
        actor.ordered_holdback = snapshot
            .ordered_holdback
            .into_iter()
            .map(|held| ((held.channel, held.sequence), held.payload))
            .collect();
        actor.reassembly_bytes = snapshot
            .reassembly
            .iter()
            .flat_map(|split| split.parts.iter().flatten())
            .map(Vec::len)
            .sum();
        actor.reassembly_parts = snapshot
            .reassembly
            .iter()
            .map(|split| split.parts.len())
            .sum();
        actor.reassembly = snapshot
            .reassembly
            .into_iter()
            .map(|split| (split.split_id, Reassembly { parts: split.parts }))
            .collect();
        actor.unacked = snapshot
            .unacked_datagrams
            .into_iter()
            .map(|datagram| {
                let elapsed = retransmit_delay(datagram.attempts)
                    .saturating_sub(datagram.retransmit_remaining);
                (
                    datagram.sequence,
                    UnackedDatagram {
                        bytes: datagram.bytes,
                        sent_at: restored_at.checked_sub(elapsed).unwrap_or(restored_at),
                        attempts: datagram.attempts,
                    },
                )
            })
            .collect();
        actor
    }

    async fn run(
        mut self,
        mut ingress_rx: mpsc::Receiver<Vec<u8>>,
        mut command_rx: mpsc::Receiver<PeerCommand>,
    ) {
        self.run_with_state(&mut ingress_rx, &mut command_rx, PeerExecution::Running)
            .await;
    }

    async fn run_frozen(
        mut self,
        mut ingress_rx: mpsc::Receiver<Vec<u8>>,
        mut command_rx: mpsc::Receiver<PeerCommand>,
        frozen_at: Instant,
    ) {
        self.run_with_state(
            &mut ingress_rx,
            &mut command_rx,
            PeerExecution::Frozen { since: frozen_at },
        )
        .await;
    }

    async fn run_with_state(
        &mut self,
        ingress_rx: &mut mpsc::Receiver<Vec<u8>>,
        command_rx: &mut mpsc::Receiver<PeerCommand>,
        mut execution: PeerExecution,
    ) {
        let mut deferred = VecDeque::new();
        loop {
            if let PeerExecution::Frozen { since } = execution {
                let Some(command) = command_rx.recv().await else {
                    break;
                };
                match command {
                    PeerCommand::Freeze { reply_tx } => {
                        let _ = reply_tx.send(());
                    }
                    PeerCommand::SealTransfer { reply_tx } => {
                        let _ = reply_tx.send(self.snapshot(since));
                    }
                    PeerCommand::Resume => {
                        self.resume_timers(since, Instant::now());
                        execution = PeerExecution::Running;
                    }
                    PeerCommand::Transfer { reply_tx } => {
                        let _ = reply_tx.send(());
                        break;
                    }
                    command @ PeerCommand::Send { .. } => {
                        if deferred.len() < self.budgets.peer_command_queue {
                            deferred.push_back(command);
                        } else if let PeerCommand::Send { reply_tx, .. } = command {
                            let _ = reply_tx.send(Err(RakNetError::QueueFull));
                        }
                    }
                }
                continue;
            }
            if let Some(command) = deferred.pop_front() {
                if self.apply_command(command, &mut execution).await {
                    break;
                }
                continue;
            }
            let timer_deadline = self.next_timer_deadline();
            // A datagram already admitted by the router is authoritative activity. Process the
            // ordered mailbox before evaluating timeout state so scheduler starvation cannot
            // turn queued traffic into a false idle disconnect.
            tokio::select! {
                biased;
                datagram = ingress_rx.recv() => {
                    let Some(bytes) = datagram else {
                        break;
                    };
                    if let Err(error) = self.receive_datagram(&bytes).await {
                        eprintln!(
                            "RakNet peer {} stopped while receiving a datagram: {error}",
                            self.remote_addr
                        );
                        break;
                    }
                }
                command = command_rx.recv() => {
                    let Some(command) = command else {
                        break;
                    };
                    if self.apply_command(command, &mut execution).await {
                        break;
                    }
                }
                _ = tokio::time::sleep_until(timer_deadline) => {
                    if let Err(error) = self.on_timer().await {
                        eprintln!(
                            "RakNet peer {} stopped during timer maintenance: {error}",
                            self.remote_addr
                        );
                        break;
                    }
                }
            }
        }
        self.inbox.close();
    }

    async fn apply_command(&mut self, command: PeerCommand, execution: &mut PeerExecution) -> bool {
        match command {
            PeerCommand::Send { payload, reply_tx } => {
                let result = self.send_application(payload).await;
                let should_stop = result.is_err();
                if let Err(error) = result.as_ref() {
                    eprintln!(
                        "RakNet peer {} stopped while sending application data: {error}",
                        self.remote_addr
                    );
                }
                let _ = reply_tx.send(result);
                should_stop
            }
            PeerCommand::Freeze { reply_tx } => {
                *execution = PeerExecution::Frozen {
                    since: Instant::now(),
                };
                let _ = reply_tx.send(());
                false
            }
            PeerCommand::SealTransfer { reply_tx } => {
                let _ = reply_tx.send(self.snapshot(Instant::now()));
                false
            }
            PeerCommand::Resume => false,
            PeerCommand::Transfer { reply_tx } => {
                let _ = reply_tx.send(());
                true
            }
        }
    }

    async fn receive_datagram(&mut self, bytes: &[u8]) -> Result<(), RakNetError> {
        self.activity.observe(Instant::now());
        if let Some(acknowledgement) = decode_acknowledgement(bytes)? {
            self.apply_acknowledgement(acknowledgement).await?;
            return Ok(());
        }
        let datagram = decode_datagram(bytes, self.budgets)?;
        self.socket
            .send_to(&encode_ack(datagram.sequence), self.remote_addr)
            .await?;
        if !self.seen_datagrams.insert(datagram.sequence) {
            return Ok(());
        }
        self.seen_datagram_order.push_back(datagram.sequence);
        while self.seen_datagram_order.len() > duplicate_window_limit(self.budgets) {
            if let Some(expired) = self.seen_datagram_order.pop_front() {
                self.seen_datagrams.remove(&expired);
            }
        }
        if let Some(previous) = self.last_datagram {
            let distance = datagram.sequence.wrapping_distance_from(previous);
            if datagram.sequence.is_newer_than(previous) && distance > 1 {
                self.socket
                    .send_to(
                        &encode_nack(previous.next(), datagram.sequence.previous()),
                        self.remote_addr,
                    )
                    .await?;
            }
            if datagram.sequence.is_newer_than(previous) {
                self.last_datagram = Some(datagram.sequence);
            }
        } else {
            self.last_datagram = Some(datagram.sequence);
        }
        for frame in datagram.frames {
            self.receive_frame(frame).await?;
        }
        Ok(())
    }

    async fn apply_acknowledgement(
        &mut self,
        acknowledgement: Acknowledgement,
    ) -> Result<(), RakNetError> {
        match acknowledgement {
            Acknowledgement::Ack(ranges) => {
                self.unacked
                    .retain(|sequence, _| !ranges.iter().any(|range| in_range(*sequence, *range)));
            }
            Acknowledgement::Nack(ranges) => {
                let sequences = self
                    .unacked
                    .keys()
                    .copied()
                    .filter(|sequence| ranges.iter().any(|range| in_range(*sequence, *range)))
                    .collect::<Vec<_>>();
                for sequence in sequences {
                    self.retransmit(sequence).await?;
                }
            }
        }
        Ok(())
    }

    async fn receive_frame(&mut self, mut frame: Frame) -> Result<(), RakNetError> {
        if let Some(reliable_index) = frame.reliable_index {
            if !self.seen_reliable.insert(reliable_index) {
                return Ok(());
            }
            self.seen_reliable_order.push_back(reliable_index);
            let seen_limit = duplicate_window_limit(self.budgets);
            while self.seen_reliable_order.len() > seen_limit {
                if let Some(expired) = self.seen_reliable_order.pop_front() {
                    self.seen_reliable.remove(&expired);
                }
            }
        }

        if let Some(split) = frame.split.take() {
            let Some(payload) = self.accept_fragment(split, frame.payload)? else {
                return Ok(());
            };
            frame.payload = payload;
        }

        if frame.reliability.sequenced() {
            let sequence = frame.sequence_index.ok_or_else(|| {
                RakNetError::Wire("sequenced frame lacked sequence index".to_string())
            })?;
            if self
                .sequenced_latest
                .get(&frame.order_channel)
                .is_some_and(|latest| !sequence.is_newer_than(*latest))
            {
                return Ok(());
            }
            self.sequenced_latest.insert(frame.order_channel, sequence);
            return self.deliver_payload(frame.payload).await;
        }

        if frame.reliability.ordered() {
            let sequence = frame
                .order_index
                .ok_or_else(|| RakNetError::Wire("ordered frame lacked order index".to_string()))?;
            return self
                .accept_ordered(frame.order_channel, sequence, frame.payload)
                .await;
        }

        self.deliver_payload(frame.payload).await
    }

    fn accept_fragment(
        &mut self,
        split: SplitHeader,
        payload: Vec<u8>,
    ) -> Result<Option<Vec<u8>>, RakNetError> {
        let count = usize::try_from(split.count)
            .map_err(|_| RakNetError::Wire("fragment count did not fit usize".to_string()))?;
        let index = usize::try_from(split.index)
            .map_err(|_| RakNetError::Wire("fragment index did not fit usize".to_string()))?;
        if count == 0 || index >= count {
            return Err(RakNetError::Wire("invalid fragment range".to_string()));
        }
        ensure_budget(
            "reassembly parts per peer",
            self.reassembly_parts
                .saturating_add(usize::from(!self.reassembly.contains_key(&split.id)) * count),
            self.budgets.max_reassembly_parts_per_peer,
        )?;
        let entry = self.reassembly.entry(split.id).or_insert_with(|| {
            self.reassembly_parts = self.reassembly_parts.saturating_add(count);
            Reassembly {
                parts: vec![None; count],
            }
        });
        if entry.parts.len() != count {
            return Err(RakNetError::Wire(
                "fragment count changed within split sequence".to_string(),
            ));
        }
        if entry.parts[index].is_none() {
            ensure_budget(
                "reassembly bytes per peer",
                self.reassembly_bytes.saturating_add(payload.len()),
                self.budgets.max_reassembly_bytes_per_peer,
            )?;
            self.reassembly_bytes = self.reassembly_bytes.saturating_add(payload.len());
            entry.parts[index] = Some(payload);
        }
        if entry.parts.iter().any(Option::is_none) {
            return Ok(None);
        }
        let completed = self
            .reassembly
            .remove(&split.id)
            .expect("complete reassembly should still be present");
        self.reassembly_parts = self.reassembly_parts.saturating_sub(completed.parts.len());
        let total_bytes = completed
            .parts
            .iter()
            .map(|part| part.as_ref().map_or(0, Vec::len))
            .sum::<usize>();
        self.reassembly_bytes = self.reassembly_bytes.saturating_sub(total_bytes);
        let mut joined = Vec::with_capacity(total_bytes);
        for part in completed.parts {
            joined.extend(part.expect("complete reassembly should contain every part"));
        }
        Ok(Some(joined))
    }

    async fn accept_ordered(
        &mut self,
        channel: u8,
        sequence: OrderSequence,
        payload: Vec<u8>,
    ) -> Result<(), RakNetError> {
        let expected = self.ordered_expected.entry(channel).or_default();
        if sequence == *expected {
            *expected = expected.next();
            self.deliver_payload(payload).await?;
            loop {
                let expected = self.ordered_expected[&channel];
                let Some(payload) = self.ordered_holdback.remove(&(channel, expected)) else {
                    break;
                };
                self.ordered_expected.insert(channel, expected.next());
                self.deliver_payload(payload).await?;
            }
        } else if sequence.is_newer_than(*expected) {
            ensure_budget(
                "ordered holdback entries",
                self.ordered_holdback.len().saturating_add(1),
                self.budgets.max_ordered_holdback,
            )?;
            self.ordered_holdback
                .entry((channel, sequence))
                .or_insert(payload);
        }
        Ok(())
    }

    async fn deliver_payload(&mut self, payload: Vec<u8>) -> Result<(), RakNetError> {
        if let Some(request_time) = decode_connection_request(&payload)? {
            let response = encode_connection_accept(self.remote_addr, request_time, epoch_millis());
            self.send_reliable_ordered(response).await?;
            return Ok(());
        }
        if let Some(ping_time) = decode_connected_ping(&payload)? {
            self.send_reliable_ordered(encode_connected_pong(ping_time, epoch_millis()))
                .await?;
            return Ok(());
        }
        match payload.first().copied() {
            Some(0x13) if !self.accepted => {
                self.accepted = true;
                let peer = RakNetPeer {
                    remote_addr: self.remote_addr,
                    command_tx: self.public_command_tx.clone(),
                    inbox: Arc::clone(&self.inbox),
                    transfer_snapshot: None,
                    router_owned: Arc::clone(&self.router_owned),
                    state: PhantomData,
                };
                self.accepted_tx
                    .send(peer)
                    .await
                    .map_err(|_| RakNetError::Closed)?;
            }
            Some(0x13 | 0x03) => {}
            Some(0x15 | 0x1c) => return Err(RakNetError::Closed),
            _ if self.accepted => {
                self.inbox.push(payload)?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn send_application(&mut self, payload: Vec<u8>) -> Result<(), RakNetError> {
        if !self.accepted {
            return Err(RakNetError::Wire(
                "application payload sent before online handshake completed".to_string(),
            ));
        }
        ensure_budget(
            "application payload bytes",
            payload.len(),
            self.budgets.max_payload_bytes,
        )?;
        self.send_reliable_ordered(payload).await
    }

    async fn send_reliable_ordered(&mut self, payload: Vec<u8>) -> Result<(), RakNetError> {
        let mtu = usize::from(self.mtu);
        let fragment_capacity = mtu.saturating_sub(DATAGRAM_HEADER_BUDGET);
        if fragment_capacity == 0 {
            return Err(RakNetError::Wire("negotiated mtu is too small".to_string()));
        }
        let order_index = self.next_ordered;
        self.next_ordered = self.next_ordered.next();
        let fragment_count = payload.len().div_ceil(fragment_capacity).max(1);
        let fragment_count_u32 = u32::try_from(fragment_count)
            .map_err(|_| RakNetError::Wire("fragment count exceeded u32".to_string()))?;
        let split_id = self.next_split_id;
        self.next_split_id = self.next_split_id.wrapping_add(1);
        for (fragment_index, chunk) in payload.chunks(fragment_capacity).enumerate() {
            let reliable_index = self.next_reliable;
            self.next_reliable = self.next_reliable.next();
            let split = (fragment_count > 1).then(|| SplitHeader {
                count: fragment_count_u32,
                id: split_id,
                index: u32::try_from(fragment_index)
                    .expect("fragment count was already proven to fit u32"),
            });
            let frame = Frame {
                reliability: Reliability::ReliableOrdered,
                reliable_index: Some(reliable_index),
                sequence_index: None,
                order_index: Some(order_index),
                order_channel: 0,
                split,
                payload: chunk.to_vec(),
            };
            self.send_frame(frame).await?;
        }
        if payload.is_empty() {
            let frame = Frame {
                reliability: Reliability::ReliableOrdered,
                reliable_index: Some(self.next_reliable),
                sequence_index: None,
                order_index: Some(order_index),
                order_channel: 0,
                split: None,
                payload,
            };
            self.next_reliable = self.next_reliable.next();
            self.send_frame(frame).await?;
        }
        Ok(())
    }

    async fn send_frame(&mut self, frame: Frame) -> Result<(), RakNetError> {
        ensure_budget(
            "unacknowledged datagrams",
            self.unacked.len().saturating_add(1),
            self.budgets.max_unacked_datagrams,
        )?;
        let sequence = self.next_datagram;
        self.next_datagram = self.next_datagram.next();
        let bytes = encode_datagram(sequence, &frame);
        ensure_budget(
            "datagram bytes",
            bytes.len(),
            self.budgets.max_datagram_bytes,
        )?;
        self.socket.send_to(&bytes, self.remote_addr).await?;
        self.unacked.insert(
            sequence,
            UnackedDatagram {
                bytes,
                sent_at: Instant::now(),
                attempts: 1,
            },
        );
        Ok(())
    }

    async fn retransmit(&mut self, sequence: DatagramSequence) -> Result<(), RakNetError> {
        let Some(datagram) = self.unacked.get_mut(&sequence) else {
            return Ok(());
        };
        if datagram.attempts >= MAX_RETRANSMIT_ATTEMPTS {
            return Err(RakNetError::RetransmitExhausted {
                sequence: sequence.value(),
                attempts: datagram.attempts,
            });
        }
        self.socket
            .send_to(&datagram.bytes, self.remote_addr)
            .await?;
        datagram.sent_at = Instant::now();
        datagram.attempts = datagram.attempts.saturating_add(1);
        Ok(())
    }

    async fn on_timer(&mut self) -> Result<(), RakNetError> {
        let now = Instant::now();
        if now.duration_since(self.activity.last_observed()) >= PEER_TIMEOUT {
            return Err(RakNetError::PeerTimeout);
        }
        let due = self
            .unacked
            .iter()
            .filter_map(|(sequence, datagram)| {
                (now.duration_since(datagram.sent_at) >= retransmit_delay(datagram.attempts))
                    .then_some(*sequence)
            })
            .collect::<Vec<_>>();
        for sequence in due {
            self.retransmit(sequence).await?;
        }
        Ok(())
    }

    fn next_timer_deadline(&self) -> Instant {
        let timeout = self
            .activity
            .last_observed()
            .checked_add(PEER_TIMEOUT)
            .unwrap_or_else(Instant::now);
        self.unacked
            .values()
            .filter_map(|datagram| {
                datagram
                    .sent_at
                    .checked_add(retransmit_delay(datagram.attempts))
            })
            .min()
            .map_or(timeout, |retransmit| retransmit.min(timeout))
    }

    /// Frozen time does not consume protocol timers, including on abort. All mutable
    /// transfer state is already constructed; activation only rebases timer origins.
    fn resume_timers(&mut self, frozen_at: Instant, resumed_at: Instant) {
        self.activity.resume(frozen_at, resumed_at);
        for datagram in self.unacked.values_mut() {
            let elapsed = frozen_at.saturating_duration_since(datagram.sent_at);
            datagram.sent_at = resumed_at.checked_sub(elapsed).unwrap_or(resumed_at);
        }
    }

    fn snapshot(&self, now: Instant) -> PeerSnapshot {
        let mut unacked_datagrams = self
            .unacked
            .iter()
            .map(|(sequence, datagram)| UnackedDatagramSnapshot {
                sequence: *sequence,
                bytes: datagram.bytes.clone(),
                retransmit_remaining: retransmit_delay(datagram.attempts)
                    .saturating_sub(now.saturating_duration_since(datagram.sent_at)),
                attempts: datagram.attempts,
            })
            .collect::<Vec<_>>();
        unacked_datagrams.sort_by_key(|entry| entry.sequence.value());
        let mut reassembly = self
            .reassembly
            .iter()
            .map(|(split_id, reassembly)| ReassemblySnapshot {
                split_id: *split_id,
                parts: reassembly.parts.clone(),
            })
            .collect::<Vec<_>>();
        reassembly.sort_by_key(|entry| entry.split_id);
        let mut ordered_holdback = self
            .ordered_holdback
            .iter()
            .map(|((channel, sequence), payload)| OrderedPayloadSnapshot {
                channel: *channel,
                sequence: *sequence,
                payload: payload.clone(),
            })
            .collect::<Vec<_>>();
        ordered_holdback.sort_by_key(|entry| (entry.channel, entry.sequence.value()));
        let mut ordered_expected = self
            .ordered_expected
            .iter()
            .map(|(channel, sequence)| (*channel, *sequence))
            .collect::<Vec<_>>();
        ordered_expected.sort_by_key(|(channel, _)| *channel);
        let mut sequenced_latest = self
            .sequenced_latest
            .iter()
            .map(|(channel, sequence)| (*channel, *sequence))
            .collect::<Vec<_>>();
        sequenced_latest.sort_by_key(|(channel, _)| *channel);
        PeerSnapshot {
            remote_addr: self.remote_addr,
            mtu: self.mtu,
            accepted: self.accepted,
            next_datagram: self.next_datagram,
            next_reliable: self.next_reliable,
            next_ordered: self.next_ordered,
            next_split_id: self.next_split_id,
            last_datagram: self.last_datagram,
            seen_datagram_order: self.seen_datagram_order.iter().copied().collect(),
            seen_reliable_order: self.seen_reliable_order.iter().copied().collect(),
            ordered_expected,
            sequenced_latest,
            queued_payloads: self.inbox.snapshot(),
            unacked_datagrams,
            reassembly,
            ordered_holdback,
            timeout_remaining: PEER_TIMEOUT
                .saturating_sub(now.saturating_duration_since(self.activity.last_observed())),
        }
    }
}

fn retransmit_delay(attempts: u16) -> Duration {
    let shift = u32::from(attempts.saturating_sub(1)).min(RETRANSMIT_MAX_BACKOFF_SHIFT);
    RETRANSMIT_BASE_DELAY
        .checked_mul(1_u32 << shift)
        .expect("bounded RakNet retransmit delay should fit Duration")
}

struct PeerInbox {
    queue: Mutex<VecDeque<Vec<u8>>>,
    notify: Notify,
    closed: AtomicBool,
    capacity: usize,
}

impl PeerInbox {
    fn new(capacity: usize) -> Self {
        Self {
            queue: Mutex::new(VecDeque::with_capacity(capacity)),
            notify: Notify::new(),
            closed: AtomicBool::new(false),
            capacity,
        }
    }

    fn from_validated_queue(payloads: Vec<Vec<u8>>, capacity: usize) -> Self {
        Self {
            queue: Mutex::new(payloads.into()),
            notify: Notify::new(),
            closed: AtomicBool::new(false),
            capacity,
        }
    }

    fn push(&self, payload: Vec<u8>) -> Result<(), RakNetError> {
        let mut queue = self
            .queue
            .lock()
            .expect("raknet peer inbox lock should not be poisoned");
        ensure_budget(
            "application queue entries",
            queue.len().saturating_add(1),
            self.capacity,
        )?;
        queue.push_back(payload);
        drop(queue);
        self.notify.notify_one();
        Ok(())
    }

    async fn receive(&self) -> Result<Vec<u8>, RakNetError> {
        loop {
            let notified = self.notify.notified();
            if let Some(payload) = self
                .queue
                .lock()
                .expect("raknet peer inbox lock should not be poisoned")
                .pop_front()
            {
                return Ok(payload);
            }
            if self.closed.load(Ordering::Acquire) {
                return Err(RakNetError::Closed);
            }
            notified.await;
        }
    }

    fn snapshot(&self) -> Vec<Vec<u8>> {
        self.queue
            .lock()
            .expect("raknet peer inbox lock should not be poisoned")
            .iter()
            .cloned()
            .collect()
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
}

fn in_range(
    sequence: DatagramSequence,
    (start, end): (DatagramSequence, DatagramSequence),
) -> bool {
    sequence.wrapping_distance_from(start) <= end.wrapping_distance_from(start)
}

fn duplicate_window_limit(budgets: RakNetBudgets) -> usize {
    budgets.max_unacked_datagrams.saturating_mul(2).max(1)
}

fn epoch_millis() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retransmit_delay_backs_off_and_caps() {
        assert_eq!(retransmit_delay(1), Duration::from_millis(200));
        assert_eq!(retransmit_delay(2), Duration::from_millis(400));
        assert_eq!(retransmit_delay(3), Duration::from_millis(800));
        assert_eq!(retransmit_delay(4), Duration::from_millis(1_600));
        assert_eq!(
            retransmit_delay(MAX_RETRANSMIT_ATTEMPTS),
            Duration::from_millis(1_600)
        );
    }

    async fn actor_with_budgets(budgets: RakNetBudgets) -> PeerActor {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let remote = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let remote_addr = remote.local_addr().unwrap();
        let (accepted_tx, _) = mpsc::channel(1);
        let (command_tx, _) = mpsc::channel(budgets.peer_command_queue);
        PeerActor::new(
            socket,
            remote_addr,
            1_492,
            budgets,
            accepted_tx,
            command_tx,
            Arc::new(PeerInbox::new(budgets.application_queue)),
            Arc::new(PeerActivity::new(Instant::now())),
            Arc::new(AtomicBool::new(true)),
        )
    }

    fn reliable_frame(index: u32, payload: &[u8]) -> Frame {
        Frame {
            reliability: Reliability::Reliable,
            reliable_index: Some(ReliableSequence::new(index)),
            sequence_index: None,
            order_index: None,
            order_channel: 0,
            split: None,
            payload: payload.to_vec(),
        }
    }

    fn reliable_ordered_frame(index: u32, order: u32, payload: &[u8]) -> Frame {
        Frame {
            reliability: Reliability::ReliableOrdered,
            reliable_index: Some(ReliableSequence::new(index)),
            sequence_index: None,
            order_index: Some(OrderSequence::new(order)),
            order_channel: 0,
            split: None,
            payload: payload.to_vec(),
        }
    }

    #[tokio::test]
    async fn cancelled_receive_does_not_consume_a_future_delivery() {
        let inbox = PeerInbox::new(1);
        assert!(
            tokio::time::timeout(Duration::from_millis(1), inbox.receive())
                .await
                .is_err()
        );

        inbox.push(vec![0xfe, 1]).unwrap();
        assert_eq!(inbox.receive().await.unwrap(), vec![0xfe, 1]);
    }

    #[test]
    fn router_delivery_reports_full_mailbox_without_waiting() {
        let (ingress_tx, _ingress_rx) = mpsc::channel(1);
        let (command_tx, _command_rx) = mpsc::channel(1);
        let stale = Instant::now()
            .checked_sub(PEER_TIMEOUT + Duration::from_secs(1))
            .unwrap();
        let activity = Arc::new(PeerActivity::new(stale));
        let control = PeerControl {
            ingress_tx,
            command_tx,
            activity: Arc::clone(&activity),
            router_owned: Arc::new(AtomicBool::new(true)),
        };

        control.deliver(vec![0]).unwrap();
        let admitted_at = activity.last_observed();
        assert!(admitted_at > stale);
        assert!(matches!(
            control.deliver(vec![1]),
            Err(RakNetError::QueueFull)
        ));
        assert_eq!(activity.last_observed(), admitted_at);
    }

    #[tokio::test]
    async fn duplicate_reliable_payload_is_delivered_once() {
        let mut actor = actor_with_budgets(RakNetBudgets::default()).await;
        actor.accepted = true;

        actor
            .receive_frame(reliable_frame(4, &[0xfe, 1]))
            .await
            .unwrap();
        actor
            .receive_frame(reliable_frame(4, &[0xfe, 1]))
            .await
            .unwrap();

        assert_eq!(actor.inbox.receive().await.unwrap(), vec![0xfe, 1]);
        assert!(
            tokio::time::timeout(Duration::from_millis(1), actor.inbox.receive())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn imported_state_preserves_delivery_order_fragments_and_paused_timers() {
        let budgets = RakNetBudgets::default();
        let mut source = actor_with_budgets(budgets).await;
        source.accepted = true;
        source
            .receive_frame(reliable_ordered_frame(0, 0, &[0xfe, 0]))
            .await
            .unwrap();
        source
            .receive_frame(reliable_ordered_frame(2, 2, &[0xfe, 2]))
            .await
            .unwrap();
        let mut fragment = reliable_frame(3, &[0xfe, 3]);
        fragment.split = Some(SplitHeader {
            count: 2,
            id: 7,
            index: 0,
        });
        source.receive_frame(fragment).await.unwrap();
        let frozen_at = Instant::now();
        source.activity = Arc::new(PeerActivity::new(frozen_at - Duration::from_secs(2)));
        source.unacked.insert(
            DatagramSequence::new(9),
            UnackedDatagram {
                bytes: vec![0x84, 9, 0, 0],
                sent_at: frozen_at - Duration::from_millis(50),
                attempts: 1,
            },
        );
        let expected = source.snapshot(frozen_at);
        let (accepted_tx, _accepted_rx) = mpsc::channel(1);
        let prepared = prepare_peer_snapshot(
            Arc::clone(&source.socket),
            expected.clone().into_validated(budgets).unwrap(),
            budgets,
            accepted_tx,
        );
        let mut restored = prepared.actor;
        assert_eq!(restored.snapshot(prepared.frozen_at), expected);

        let resumed_at = prepared.frozen_at + Duration::from_secs(30);
        restored.resume_timers(prepared.frozen_at, resumed_at);
        assert_eq!(restored.snapshot(resumed_at), expected);
        let elapsed = restored.snapshot(resumed_at + Duration::from_millis(40));
        assert_eq!(elapsed.timeout_remaining, Duration::from_millis(9_960));
        assert_eq!(
            elapsed.unacked_datagrams[0].retransmit_remaining,
            Duration::from_millis(110)
        );

        restored
            .receive_frame(reliable_ordered_frame(0, 0, &[0xfe, 0]))
            .await
            .unwrap();
        restored
            .receive_frame(reliable_ordered_frame(1, 1, &[0xfe, 1]))
            .await
            .unwrap();
        let mut fragment = reliable_frame(4, &[4]);
        fragment.split = Some(SplitHeader {
            count: 2,
            id: 7,
            index: 1,
        });
        restored.receive_frame(fragment).await.unwrap();
        assert_eq!(
            restored.inbox.snapshot(),
            vec![
                vec![0xfe, 0],
                vec![0xfe, 1],
                vec![0xfe, 2],
                vec![0xfe, 3, 4]
            ]
        );
        assert_eq!(restored.reassembly_bytes, 0);
        assert_eq!(restored.reassembly_parts, 0);
    }

    #[test]
    fn activity_admitted_while_frozen_starts_idle_timer_at_resume() {
        let frozen_at = Instant::now();
        let activity = PeerActivity::new(frozen_at - Duration::from_secs(2));
        activity.observe(frozen_at + Duration::from_secs(1));
        let resumed_at = frozen_at + Duration::from_secs(30);
        activity.resume(frozen_at, resumed_at);
        assert_eq!(activity.last_observed(), resumed_at);
    }

    #[tokio::test]
    async fn queued_datagram_refreshes_activity_before_overdue_timer() {
        let budgets = RakNetBudgets::default();
        let mut actor = actor_with_budgets(budgets).await;
        actor.activity = Arc::new(PeerActivity::new(
            Instant::now()
                .checked_sub(PEER_TIMEOUT + Duration::from_secs(1))
                .unwrap(),
        ));
        let (ingress_tx, ingress_rx) = mpsc::channel(budgets.peer_ingress_queue);
        let (command_tx, command_rx) = mpsc::channel(budgets.peer_command_queue);
        ingress_tx
            .send(encode_ack(DatagramSequence::new(0)))
            .await
            .unwrap();
        let (freeze_tx, freeze_rx) = oneshot::channel();
        command_tx
            .send(PeerCommand::Freeze {
                reply_tx: freeze_tx,
            })
            .await
            .unwrap();

        let task = tokio::spawn(actor.run(ingress_rx, command_rx));
        tokio::time::timeout(Duration::from_secs(1), freeze_rx)
            .await
            .unwrap()
            .unwrap();
        let (seal_tx, seal_rx) = oneshot::channel();
        command_tx
            .send(PeerCommand::SealTransfer { reply_tx: seal_tx })
            .await
            .unwrap();
        let snapshot = tokio::time::timeout(Duration::from_secs(1), seal_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(snapshot.timeout_remaining > PEER_TIMEOUT - Duration::from_secs(1));

        let (transfer_tx, transfer_rx) = oneshot::channel();
        command_tx
            .send(PeerCommand::Transfer {
                reply_tx: transfer_tx,
            })
            .await
            .unwrap();
        transfer_rx.await.unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn nacked_older_datagram_completes_reliable_ordered_holdback() {
        let mut actor = actor_with_budgets(RakNetBudgets::default()).await;
        actor.accepted = true;
        let first = encode_datagram(
            DatagramSequence::new(0),
            &reliable_ordered_frame(0, 0, &[0xfe, 0]),
        );
        let third = encode_datagram(
            DatagramSequence::new(2),
            &reliable_ordered_frame(2, 2, &[0xfe, 2]),
        );
        let retransmitted = encode_datagram(
            DatagramSequence::new(1),
            &reliable_ordered_frame(1, 1, &[0xfe, 1]),
        );

        actor.receive_datagram(&first).await.unwrap();
        actor.receive_datagram(&third).await.unwrap();
        actor.receive_datagram(&retransmitted).await.unwrap();

        assert_eq!(actor.inbox.receive().await.unwrap(), vec![0xfe, 0]);
        assert_eq!(actor.inbox.receive().await.unwrap(), vec![0xfe, 1]);
        assert_eq!(actor.inbox.receive().await.unwrap(), vec![0xfe, 2]);
    }

    #[tokio::test]
    async fn ordered_holdback_flushes_after_missing_payload_arrives() {
        let mut actor = actor_with_budgets(RakNetBudgets::default()).await;
        actor.accepted = true;

        actor
            .accept_ordered(0, OrderSequence::new(1), vec![2])
            .await
            .unwrap();
        actor
            .accept_ordered(0, OrderSequence::new(0), vec![1])
            .await
            .unwrap();

        assert_eq!(actor.inbox.receive().await.unwrap(), vec![1]);
        assert_eq!(actor.inbox.receive().await.unwrap(), vec![2]);
    }

    #[tokio::test]
    async fn fragments_reassemble_by_index_before_delivery() {
        let mut actor = actor_with_budgets(RakNetBudgets::default()).await;
        actor.accepted = true;
        let mut second = reliable_frame(1, b"rock");
        second.split = Some(SplitHeader {
            count: 2,
            id: 7,
            index: 1,
        });
        let mut first = reliable_frame(2, b"bed");
        first.split = Some(SplitHeader {
            count: 2,
            id: 7,
            index: 0,
        });

        actor.receive_frame(second).await.unwrap();
        actor.receive_frame(first).await.unwrap();

        assert_eq!(actor.inbox.receive().await.unwrap(), b"bedrock");
        assert!(actor.reassembly.is_empty());
        assert_eq!(actor.reassembly_bytes, 0);
    }

    #[tokio::test]
    async fn reassembly_budget_exhaustion_is_a_policy_outcome() {
        let budgets = RakNetBudgets {
            max_reassembly_bytes_per_peer: 3,
            ..RakNetBudgets::default()
        };
        let mut actor = actor_with_budgets(budgets).await;
        let mut frame = reliable_frame(0, b"four");
        frame.split = Some(SplitHeader {
            count: 2,
            id: 1,
            index: 0,
        });

        let error = actor.receive_frame(frame).await.unwrap_err();
        assert!(matches!(
            error,
            RakNetError::Budget(crate::BudgetExceeded {
                resource: "reassembly bytes per peer",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn nack_retransmits_the_requested_datagram() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let remote = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let remote_addr = remote.local_addr().unwrap();
        let budgets = RakNetBudgets::default();
        let (accepted_tx, _) = mpsc::channel(1);
        let (command_tx, _) = mpsc::channel(budgets.peer_command_queue);
        let mut actor = PeerActor::new(
            socket,
            remote_addr,
            1_492,
            budgets,
            accepted_tx,
            command_tx,
            Arc::new(PeerInbox::new(budgets.application_queue)),
            Arc::new(PeerActivity::new(Instant::now())),
            Arc::new(AtomicBool::new(true)),
        );
        let sequence = DatagramSequence::new(9);
        actor.unacked.insert(
            sequence,
            UnackedDatagram {
                bytes: vec![0x84, 9, 0, 0],
                sent_at: Instant::now(),
                attempts: 1,
            },
        );

        actor
            .apply_acknowledgement(Acknowledgement::Nack(vec![(sequence, sequence)]))
            .await
            .unwrap();

        let mut received = [0_u8; 16];
        let (length, _) = remote.recv_from(&mut received).await.unwrap();
        assert_eq!(&received[..length], &[0x84, 9, 0, 0]);
        assert_eq!(actor.unacked[&sequence].attempts, 2);
    }

    #[tokio::test]
    async fn frozen_peer_resumes_without_losing_send_authority() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let remote = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let remote_addr = remote.local_addr().unwrap();
        let budgets = RakNetBudgets::default();
        let (accepted_tx, _) = mpsc::channel(1);
        let (_ingress_tx, ingress_rx) = mpsc::channel(budgets.peer_ingress_queue);
        let (command_tx, command_rx) = mpsc::channel(budgets.peer_command_queue);
        let inbox = Arc::new(PeerInbox::new(budgets.application_queue));
        let activity = Arc::new(PeerActivity::new(Instant::now()));
        let router_owned = Arc::new(AtomicBool::new(false));
        let mut actor = PeerActor::new(
            socket,
            remote_addr,
            1_492,
            budgets,
            accepted_tx,
            command_tx.clone(),
            Arc::clone(&inbox),
            activity,
            Arc::clone(&router_owned),
        );
        actor.accepted = true;
        let actor_task = tokio::spawn(actor.run(ingress_rx, command_rx));
        let peer = RakNetPeer::<Running> {
            remote_addr,
            command_tx,
            inbox,
            transfer_snapshot: None,
            router_owned,
            state: PhantomData,
        };

        let frozen = peer
            .freeze()
            .await
            .unwrap()
            .seal_for_transfer()
            .await
            .unwrap();
        assert_eq!(frozen.snapshot().remote_addr, remote_addr);
        let mut resumed = frozen.resume().unwrap();
        resumed.send_raw(&[0xfe, 4]).await.unwrap();

        let mut received = [0_u8; 1_600];
        let (length, _) =
            tokio::time::timeout(Duration::from_secs(1), remote.recv_from(&mut received))
                .await
                .unwrap()
                .unwrap();
        assert!(length > 4);

        let frozen = resumed
            .freeze()
            .await
            .unwrap()
            .seal_for_transfer()
            .await
            .unwrap();
        let transferred = frozen.mark_transferred().await.unwrap();
        assert_eq!(transferred.snapshot().remote_addr, remote_addr);
        actor_task.await.unwrap();
    }
}
