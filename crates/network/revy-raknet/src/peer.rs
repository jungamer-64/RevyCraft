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
use tokio::time::{Instant, MissedTickBehavior};

const RETRANSMIT_AFTER: Duration = Duration::from_millis(200);
const PEER_TIMEOUT: Duration = Duration::from_secs(12);
const TIMER_GRANULARITY: Duration = Duration::from_millis(25);
const MAX_RETRANSMIT_ATTEMPTS: u16 = 60;
const DATAGRAM_HEADER_BUDGET: usize = 64;

pub struct Running;
pub struct Frozen;
pub struct Transferred;

pub struct RakNetPeer<State> {
    remote_addr: SocketAddr,
    command_tx: mpsc::Sender<PeerCommand>,
    inbox: Arc<PeerInbox>,
    frozen: Option<PeerSnapshot>,
    state: PhantomData<State>,
}

impl RakNetPeer<Running> {
    #[must_use]
    pub const fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    pub async fn recv_raw(&mut self) -> Result<Vec<u8>, RakNetError> {
        self.inbox.receive().await
    }

    pub async fn send_raw(&mut self, payload: &[u8]) -> Result<(), RakNetError> {
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

    pub async fn freeze(self) -> Result<RakNetPeer<Frozen>, RakNetError> {
        let snapshot = freeze_control(&self.command_tx).await?;
        Ok(RakNetPeer {
            remote_addr: self.remote_addr,
            command_tx: self.command_tx,
            inbox: self.inbox,
            frozen: Some(snapshot),
            state: PhantomData,
        })
    }
}

impl RakNetPeer<Frozen> {
    #[must_use]
    pub fn snapshot(&self) -> &PeerSnapshot {
        self.frozen
            .as_ref()
            .expect("frozen peer should always own its snapshot")
    }

    #[must_use]
    pub fn mark_transferred(mut self) -> RakNetPeer<Transferred> {
        RakNetPeer {
            remote_addr: self.remote_addr,
            command_tx: self.command_tx,
            inbox: self.inbox,
            frozen: self.frozen.take(),
            state: PhantomData,
        }
    }
}

impl RakNetPeer<Transferred> {
    #[must_use]
    pub fn snapshot(&self) -> &PeerSnapshot {
        self.frozen
            .as_ref()
            .expect("transferred peer should retain its sealed snapshot")
    }
}

#[derive(Clone, Debug)]
pub struct PeerSnapshot {
    pub remote_addr: SocketAddr,
    pub mtu: u16,
    pub next_datagram: DatagramSequence,
    pub next_reliable: ReliableSequence,
    pub next_ordered: OrderSequence,
    pub queued_payloads: Vec<Vec<u8>>,
    pub unacked_datagrams: Vec<UnackedDatagramSnapshot>,
    pub reassembly: Vec<ReassemblySnapshot>,
    pub ordered_holdback: Vec<OrderedPayloadSnapshot>,
    pub timeout_remaining: Duration,
}

#[derive(Clone, Debug)]
pub struct UnackedDatagramSnapshot {
    pub sequence: DatagramSequence,
    pub bytes: Vec<u8>,
    pub retransmit_remaining: Duration,
    pub attempts: u16,
}

#[derive(Clone, Debug)]
pub struct ReassemblySnapshot {
    pub split_id: u16,
    pub parts: Vec<Option<Vec<u8>>>,
}

#[derive(Clone, Debug)]
pub struct OrderedPayloadSnapshot {
    pub channel: u8,
    pub sequence: OrderSequence,
    pub payload: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct PeerControl {
    command_tx: mpsc::Sender<PeerCommand>,
}

impl PeerControl {
    pub(crate) async fn deliver(&self, bytes: Vec<u8>) -> Result<(), RakNetError> {
        self.command_tx
            .send(PeerCommand::Datagram { bytes })
            .await
            .map_err(|_| RakNetError::Closed)
    }

    pub(crate) async fn freeze(&self) -> Result<PeerSnapshot, RakNetError> {
        freeze_control(&self.command_tx).await
    }
}

async fn freeze_control(
    command_tx: &mpsc::Sender<PeerCommand>,
) -> Result<PeerSnapshot, RakNetError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    command_tx
        .send(PeerCommand::Freeze { reply_tx })
        .await
        .map_err(|_| RakNetError::Closed)?;
    reply_rx.await.map_err(|_| RakNetError::Closed)
}

pub(crate) struct PeerSpawn {
    pub control: PeerControl,
}

pub(crate) fn spawn_peer(
    socket: Arc<UdpSocket>,
    remote_addr: SocketAddr,
    mtu: u16,
    budgets: RakNetBudgets,
    accepted_tx: mpsc::Sender<RakNetPeer<Running>>,
    closed_tx: mpsc::UnboundedSender<SocketAddr>,
) -> PeerSpawn {
    let (command_tx, command_rx) = mpsc::channel(budgets.peer_command_queue);
    let inbox = Arc::new(PeerInbox::new(budgets.application_queue));
    let control = PeerControl {
        command_tx: command_tx.clone(),
    };
    let actor = PeerActor::new(
        socket,
        remote_addr,
        mtu,
        budgets,
        accepted_tx,
        command_tx,
        inbox,
    );
    tokio::spawn(async move {
        actor.run(command_rx).await;
        let _ = closed_tx.send(remote_addr);
    });
    PeerSpawn { control }
}

enum PeerCommand {
    Datagram {
        bytes: Vec<u8>,
    },
    Send {
        payload: Vec<u8>,
        reply_tx: oneshot::Sender<Result<(), RakNetError>>,
    },
    Freeze {
        reply_tx: oneshot::Sender<PeerSnapshot>,
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
    accepted: bool,
    next_datagram: DatagramSequence,
    next_reliable: ReliableSequence,
    next_ordered: OrderSequence,
    next_split_id: u16,
    last_datagram: Option<DatagramSequence>,
    seen_reliable: HashSet<ReliableSequence>,
    seen_reliable_order: VecDeque<ReliableSequence>,
    ordered_expected: HashMap<u8, OrderSequence>,
    sequenced_latest: HashMap<u8, OrderSequence>,
    ordered_holdback: HashMap<(u8, OrderSequence), Vec<u8>>,
    reassembly: HashMap<u16, Reassembly>,
    reassembly_bytes: usize,
    reassembly_parts: usize,
    unacked: HashMap<DatagramSequence, UnackedDatagram>,
    last_activity: Instant,
}

struct UnackedDatagram {
    bytes: Vec<u8>,
    sent_at: Instant,
    attempts: u16,
}

struct Reassembly {
    parts: Vec<Option<Vec<u8>>>,
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
    ) -> Self {
        Self {
            socket,
            remote_addr,
            mtu,
            budgets,
            accepted_tx,
            public_command_tx,
            inbox,
            accepted: false,
            next_datagram: DatagramSequence::default(),
            next_reliable: ReliableSequence::default(),
            next_ordered: OrderSequence::default(),
            next_split_id: 0,
            last_datagram: None,
            seen_reliable: HashSet::new(),
            seen_reliable_order: VecDeque::new(),
            ordered_expected: HashMap::new(),
            sequenced_latest: HashMap::new(),
            ordered_holdback: HashMap::new(),
            reassembly: HashMap::new(),
            reassembly_bytes: 0,
            reassembly_parts: 0,
            unacked: HashMap::new(),
            last_activity: Instant::now(),
        }
    }

    async fn run(mut self, mut command_rx: mpsc::Receiver<PeerCommand>) {
        let mut timer = tokio::time::interval(TIMER_GRANULARITY);
        timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                command = command_rx.recv() => {
                    let Some(command) = command else {
                        break;
                    };
                    match command {
                        PeerCommand::Datagram { bytes } => {
                            if self.receive_datagram(&bytes).await.is_err() {
                                break;
                            }
                        }
                        PeerCommand::Send { payload, reply_tx } => {
                            let result = self.send_application(payload).await;
                            let should_stop = result.is_err();
                            let _ = reply_tx.send(result);
                            if should_stop {
                                break;
                            }
                        }
                        PeerCommand::Freeze { reply_tx } => {
                            let _ = reply_tx.send(self.snapshot());
                            break;
                        }
                    }
                }
                _ = timer.tick() => {
                    if self.on_timer().await.is_err() {
                        break;
                    }
                }
            }
        }
        self.inbox.close();
    }

    async fn receive_datagram(&mut self, bytes: &[u8]) -> Result<(), RakNetError> {
        self.last_activity = Instant::now();
        if let Some(acknowledgement) = decode_acknowledgement(bytes)? {
            self.apply_acknowledgement(acknowledgement).await?;
            return Ok(());
        }
        let datagram = decode_datagram(bytes, self.budgets)?;
        self.socket
            .send_to(&encode_ack(datagram.sequence), self.remote_addr)
            .await?;
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
            if !datagram.sequence.is_newer_than(previous) {
                return Ok(());
            }
        }
        self.last_datagram = Some(datagram.sequence);
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
            let seen_limit = self.budgets.max_unacked_datagrams.saturating_mul(2);
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
                    frozen: None,
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
            return Err(RakNetError::Closed);
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
        if now.duration_since(self.last_activity) >= PEER_TIMEOUT {
            return Err(RakNetError::Closed);
        }
        let due = self
            .unacked
            .iter()
            .filter_map(|(sequence, datagram)| {
                (now.duration_since(datagram.sent_at) >= RETRANSMIT_AFTER).then_some(*sequence)
            })
            .collect::<Vec<_>>();
        for sequence in due {
            self.retransmit(sequence).await?;
        }
        Ok(())
    }

    fn snapshot(&self) -> PeerSnapshot {
        let now = Instant::now();
        PeerSnapshot {
            remote_addr: self.remote_addr,
            mtu: self.mtu,
            next_datagram: self.next_datagram,
            next_reliable: self.next_reliable,
            next_ordered: self.next_ordered,
            queued_payloads: self.inbox.snapshot(),
            unacked_datagrams: self
                .unacked
                .iter()
                .map(|(sequence, datagram)| UnackedDatagramSnapshot {
                    sequence: *sequence,
                    bytes: datagram.bytes.clone(),
                    retransmit_remaining: RETRANSMIT_AFTER
                        .saturating_sub(now.duration_since(datagram.sent_at)),
                    attempts: datagram.attempts,
                })
                .collect(),
            reassembly: self
                .reassembly
                .iter()
                .map(|(split_id, reassembly)| ReassemblySnapshot {
                    split_id: *split_id,
                    parts: reassembly.parts.clone(),
                })
                .collect(),
            ordered_holdback: self
                .ordered_holdback
                .iter()
                .map(|((channel, sequence), payload)| OrderedPayloadSnapshot {
                    channel: *channel,
                    sequence: *sequence,
                    payload: payload.clone(),
                })
                .collect(),
            timeout_remaining: PEER_TIMEOUT.saturating_sub(now.duration_since(self.last_activity)),
        }
    }
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
}
