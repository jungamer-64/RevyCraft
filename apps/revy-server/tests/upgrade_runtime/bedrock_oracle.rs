use base64::Engine;
use bedrock_protocol::ProtoVersion;
use bedrock_protocol::V924;
use bedrock_protocol::v662::enums::{ContainerID, PlayerPositionMode};
use bedrock_protocol::v662::packets::{
    LoginPacket, MobEquipmentPacket, MovePlayerPacket, NetworkStackLatencyPacket,
    RequestNetworkSettingsPacket,
};
use bedrock_protocol::v662::types::{ActorRuntimeID, NetworkItemStackDescriptor};
use mc_proto_be_common::{
    BEDROCK_GAME_PACKET_ID, BEDROCK_RAKNET_MAGIC, BedrockCompression, decode_packet_batch,
    encode_packet_batch,
};
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;

const ORACLE_MTU: usize = 1_492;
const MAX_DATAGRAM_BYTES: usize = 2_048;
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_REASSEMBLY_PARTS: usize = 4_096;
const INCOMPLETE_SPLIT_ID: u16 = 0x4c41;
const OFFLINE_EXCHANGE_ATTEMPT_LIMIT: u8 = 30;
const ORACLE_RETRANSMIT_BASE_DELAY: Duration = Duration::from_millis(200);
const ORACLE_RETRANSMIT_MAX_BACKOFF_SHIFT: u32 = 3;
const MAX_RETRANSMIT_ATTEMPTS: u16 = 600;
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(120);
const DISCONNECT_PACKET_ID: u16 = 5;
const NETWORK_SETTINGS_PACKET_ID: u16 = 143;
const START_GAME_PACKET_ID: u16 = 11;
const PLAYER_HOTBAR_PACKET_ID: u16 = 48;
const NETWORK_STACK_LATENCY_PACKET_ID: u16 = 115;

pub(crate) struct BedrockOracle {
    socket: Arc<UdpSocket>,
    next_datagram: u32,
    next_reliable: u32,
    next_ordered: u32,
    sent_datagrams: HashMap<u32, SentDatagram>,
    seen_reliable: HashSet<u32>,
    reassembly: HashMap<u16, Vec<Option<Vec<u8>>>>,
    expected_ordered: u32,
    ordered_holdback: HashMap<u32, Vec<u8>>,
    ready_payloads: VecDeque<Vec<u8>>,
    pending_batch: BedrockPacketBatch,
    withheld_server_datagram: Option<u32>,
    withhold_next_server_datagram: bool,
    compression: Option<BedrockCompression>,
    actor_runtime_id: Option<u64>,
    next_keepalive: tokio::time::Instant,
}

impl BedrockOracle {
    pub(crate) async fn connect(server_addr: SocketAddr) -> io::Result<Self> {
        let bind_addr = match server_addr.ip() {
            IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            IpAddr::V6(_) => "[::]:0"
                .parse()
                .map_err(|error| invalid_data(format!("invalid IPv6 bind address: {error}")))?,
        };
        let socket = Arc::new(UdpSocket::bind(bind_addr).await?);
        socket.connect(server_addr).await?;
        open_offline_session(&socket, server_addr).await?;

        let mut client = Self {
            socket,
            next_datagram: 0,
            next_reliable: 0,
            next_ordered: 0,
            sent_datagrams: HashMap::new(),
            seen_reliable: HashSet::new(),
            reassembly: HashMap::new(),
            expected_ordered: 0,
            ordered_holdback: HashMap::new(),
            ready_payloads: VecDeque::new(),
            pending_batch: BedrockPacketBatch::default(),
            withheld_server_datagram: None,
            withhold_next_server_datagram: false,
            compression: None,
            actor_runtime_id: None,
            next_keepalive: tokio::time::Instant::now() + Duration::from_secs(4),
        };

        let client_guid = client_guid(&client.socket)?;
        let mut request = Vec::with_capacity(18);
        request.push(0x09);
        request.extend_from_slice(&client_guid.to_be_bytes());
        request.extend_from_slice(&epoch_millis().to_be_bytes());
        request.push(0);
        client.send_reliable_ordered(&request).await?;
        loop {
            let payload = client.receive_payload().await?;
            match payload.first().copied() {
                Some(0x10) => {
                    client.send_reliable_ordered(&[0x13]).await?;
                    return Ok(client);
                }
                Some(0x00) => client.reply_connected_ping(&payload).await?,
                _ => {}
            }
        }
    }

    pub(crate) async fn login(&mut self, username: &str) -> io::Result<()> {
        let protocol_version = i32::try_from(V924::PROTOCOL_VERSION)
            .map_err(|_| invalid_data("Bedrock protocol version exceeds i32"))?;
        let settings = encode_packet_batch(
            &[V924::RequestNetworkSettingsPacket(Box::new(
                RequestNetworkSettingsPacket {
                    client_network_version: protocol_version,
                },
            ))],
            None,
        )
        .map_err(codec_error)?;
        self.send_bedrock_batch(settings).await?;

        let V924::NetworkSettingsPacket(settings) = self
            .receive_bedrock_packet(NETWORK_SETTINGS_PACKET_ID)
            .await?
        else {
            return Err(invalid_data(
                "Bedrock packet id 143 did not decode as NetworkSettings",
            ));
        };
        self.compression = Some(BedrockCompression::zlib(settings.compression_threshold));

        let login = bedrock_login_packet(username, self.compression.as_ref())?;
        self.send_bedrock_batch(login).await?;
        let V924::StartGamePacket(start_game) =
            self.receive_bedrock_packet(START_GAME_PACKET_ID).await?
        else {
            return Err(invalid_data(
                "Bedrock packet id 11 did not decode as StartGame",
            ));
        };
        self.actor_runtime_id = Some(start_game.target_runtime_id.0);
        Ok(())
    }

    pub(crate) async fn relocate(&mut self, x: f32) -> io::Result<()> {
        let runtime_id = self
            .actor_runtime_id
            .ok_or_else(|| invalid_data("Bedrock login completed without StartGame"))?;
        let packet = V924::MovePlayerPacket(Box::new(MovePlayerPacket {
            player_runtime_id: ActorRuntimeID(runtime_id),
            position: (x, 5.62, 0.5),
            rotation: (0.0, 0.0),
            y_head_rotation: 0.0,
            position_mode: PlayerPositionMode::Normal,
            on_ground: true,
            riding_runtime_id: ActorRuntimeID(0),
            tick: 0,
        }));
        self.send_packets(&[packet]).await
    }

    pub(crate) async fn begin_in_flight_marker(&mut self) -> io::Result<()> {
        self.release_withheld_ack().await?;
        self.withhold_next_server_datagram = true;
        self.send_connected_ping().await?;
        self.send_incomplete_fragment().await
    }

    pub(crate) async fn begin_roundtrip(&mut self, slot: i8) -> io::Result<()> {
        self.send_held_slot(slot).await
    }

    pub(crate) async fn release_in_flight(&mut self) -> io::Result<()> {
        self.release_withheld_ack().await
    }

    async fn send_held_slot(&mut self, slot: i8) -> io::Result<()> {
        let runtime_id = self
            .actor_runtime_id
            .ok_or_else(|| invalid_data("Bedrock login completed without StartGame"))?;
        self.send_packets(&[V924::MobEquipmentPacket(Box::new(MobEquipmentPacket {
            target_runtime_id: ActorRuntimeID(runtime_id),
            item: NetworkItemStackDescriptor {
                id: 0,
                stack_size: None,
                aux_value: None,
                net_id_variant: None,
                block_runtime_id: None,
                user_data_buffer: None,
            },
            slot,
            selected_slot: slot,
            container_id: ContainerID::Inventory,
        }))])
        .await
    }

    pub(crate) async fn await_in_flight_marker(&mut self) -> io::Result<()> {
        loop {
            let payload = self.receive_payload().await?;
            if payload.first().copied() != Some(0x03) {
                continue;
            }
            if self.withheld_server_datagram.is_none() {
                return Err(invalid_data(
                    "Bedrock workload completed without an unacknowledged server datagram",
                ));
            }
            return Ok(());
        }
    }

    /// Hotbar notifications have no request identifier. Reload resync may precede a command's
    /// response with arbitrarily many older notifications; none acknowledges a new rejection.
    pub(crate) async fn await_selected_slot(
        &mut self,
        wanted: u32,
        deadline: tokio::time::Instant,
        resync_settle: Duration,
    ) -> io::Result<()> {
        let mut response_deadline = None;
        loop {
            let V924::PlayerHotbarPacket(hotbar) = self
                .receive_bedrock_packet_until(
                    PLAYER_HOTBAR_PACKET_ID,
                    response_deadline.unwrap_or(deadline),
                )
                .await?
            else {
                return Err(invalid_data(
                    "Bedrock packet id 48 did not decode as PlayerHotbar",
                ));
            };
            if hotbar.selected_slot == wanted {
                return Ok(());
            }
            // The ordinary network timeout still applies until a slot notification arrives.
            // Only an observed old value can arm an application retry; silence is handled by
            // reliable transport recovery, not by issuing additional gameplay commands.
            response_deadline
                .get_or_insert_with(|| deadline.min(tokio::time::Instant::now() + resync_settle));
        }
    }

    /// Services the connection until a command arrives. Only the socket receive and timer waits
    /// race the command: ACK, ordered delivery, and reliable sends always finish before returning
    /// ownership. Cancelling those operations could acknowledge a packet without retaining it.
    pub(crate) async fn receive_command<T>(
        &mut self,
        command: impl std::future::Future<Output = T>,
    ) -> io::Result<T> {
        tokio::pin!(command);
        let mut bytes = [0_u8; MAX_DATAGRAM_BYTES];
        loop {
            self.maintain_outbound().await?;
            while let Some(payload) = self.ready_payloads.pop_front() {
                self.reply_application_keepalive(&payload).await?;
            }
            let wait = self.next_maintenance_delay(tokio::time::Instant::now());
            tokio::select! {
                result = &mut command => return Ok(result),
                received = self.socket.recv(&mut bytes) => {
                    self.accept_datagram(&bytes[..received?]).await?;
                }
                () = tokio::time::sleep(wait) => {}
            }
        }
    }

    async fn send_connected_ping(&mut self) -> io::Result<()> {
        let mut ping = Vec::with_capacity(9);
        ping.push(0x00);
        ping.extend_from_slice(&epoch_millis().to_be_bytes());
        self.send_reliable_ordered(&ping).await
    }

    async fn send_packets(&mut self, packets: &[V924]) -> io::Result<()> {
        let batch = encode_packet_batch(packets, self.compression.as_ref()).map_err(codec_error)?;
        self.send_bedrock_batch(batch).await
    }

    async fn send_bedrock_batch(&mut self, batch: Vec<u8>) -> io::Result<()> {
        let mut payload = Vec::with_capacity(batch.len().saturating_add(1));
        payload.push(BEDROCK_GAME_PACKET_ID);
        payload.extend_from_slice(&batch);
        self.send_reliable_ordered(&payload).await
    }

    async fn send_reliable_ordered(&mut self, payload: &[u8]) -> io::Result<()> {
        let reliable = take_u24(&mut self.next_reliable);
        let ordered = take_u24(&mut self.next_ordered);
        let frame = encode_frame(3, reliable, Some(ordered), None, payload)?;
        self.send_datagram(frame).await
    }

    async fn send_incomplete_fragment(&mut self) -> io::Result<()> {
        let reliable = take_u24(&mut self.next_reliable);
        let frame = encode_frame(
            2,
            reliable,
            None,
            Some(SplitHeader {
                count: 2,
                id: INCOMPLETE_SPLIT_ID,
                index: 0,
            }),
            &[BEDROCK_GAME_PACKET_ID],
        )?;
        self.send_datagram(frame).await
    }

    async fn send_datagram(&mut self, frame: Vec<u8>) -> io::Result<()> {
        let sequence = take_u24(&mut self.next_datagram);
        let mut datagram = Vec::with_capacity(frame.len().saturating_add(4));
        datagram.push(0x84);
        write_u24_le(&mut datagram, sequence);
        datagram.extend_from_slice(&frame);
        self.socket.send(&datagram).await?;
        self.sent_datagrams.insert(
            sequence,
            SentDatagram {
                bytes: datagram,
                sent_at: tokio::time::Instant::now(),
                attempts: 1,
            },
        );
        Ok(())
    }

    async fn receive_bedrock_packet(&mut self, wanted_packet_id: u16) -> io::Result<V924> {
        let deadline = tokio::time::Instant::now() + RECEIVE_TIMEOUT;
        self.receive_bedrock_packet_until(wanted_packet_id, deadline)
            .await
    }

    async fn receive_bedrock_packet_until(
        &mut self,
        wanted_packet_id: u16,
        deadline: tokio::time::Instant,
    ) -> io::Result<V924> {
        loop {
            if let Some(packet) = self.pending_batch.next(wanted_packet_id)? {
                return Ok(packet);
            }
            let payload = self.receive_payload_until(deadline).await?;
            match payload.first().copied() {
                Some(BEDROCK_GAME_PACKET_ID) => {
                    self.pending_batch =
                        BedrockPacketBatch::decode(&payload[1..], self.compression.as_ref())?;
                }
                Some(0x00) => self.reply_connected_ping(&payload).await?,
                Some(0x03) => {}
                Some(0x15 | 0x1c) => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "RakNet peer disconnected",
                    ));
                }
                _ => {}
            }
        }
    }

    async fn receive_payload(&mut self) -> io::Result<Vec<u8>> {
        let deadline = tokio::time::Instant::now() + RECEIVE_TIMEOUT;
        self.receive_payload_until(deadline).await
    }

    async fn receive_payload_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> io::Result<Vec<u8>> {
        let mut bytes = [0_u8; MAX_DATAGRAM_BYTES];
        loop {
            let now = tokio::time::Instant::now();
            let remaining = deadline.checked_duration_since(now).ok_or_else(|| {
                let mut held_orders = self.ordered_holdback.keys().copied().collect::<Vec<_>>();
                held_orders.sort_unstable();
                held_orders.truncate(8);
                io::Error::new(io::ErrorKind::TimedOut, format!(
                    "RakNet receive timed out: local={:?}, expected_order={}, holdback={held_orders:?}, reassembly={}, ready={}, unacked_client={}, withheld_server={:?}",
                    self.socket.local_addr(), self.expected_ordered, self.reassembly.len(), self.ready_payloads.len(), self.sent_datagrams.len(), self.withheld_server_datagram,
                ))
            })?;
            self.maintain_outbound().await?;
            if let Some(payload) = self.ready_payloads.pop_front() {
                self.reply_application_keepalive(&payload).await?;
                return Ok(payload);
            }
            let wait = remaining.min(self.next_maintenance_delay(now));
            let length = match tokio::time::timeout(wait, self.socket.recv(&mut bytes)).await {
                Ok(result) => result?,
                Err(_) => continue,
            };
            self.accept_datagram(&bytes[..length]).await?;
        }
    }

    async fn accept_datagram(&mut self, datagram: &[u8]) -> io::Result<()> {
        match datagram.first().copied() {
            Some(0xc0) => {
                for (start, end) in decode_ack_ranges(datagram)? {
                    self.sent_datagrams
                        .retain(|sequence, _| !in_u24_range(*sequence, start, end));
                }
            }
            Some(0xa0) => {
                for (start, end) in decode_ack_ranges(datagram)? {
                    let retransmit = self
                        .sent_datagrams
                        .iter()
                        .filter(|(sequence, _)| in_u24_range(**sequence, start, end))
                        .map(|(sequence, _)| *sequence)
                        .collect::<Vec<_>>();
                    for sequence in retransmit {
                        self.retransmit(sequence).await?;
                    }
                }
            }
            Some(flags) if flags & 0x80 != 0 && flags & 0x60 == 0 => {
                let (sequence, frames) = decode_datagram(datagram)?;
                if self.withheld_server_datagram != Some(sequence) {
                    if self.withhold_next_server_datagram {
                        self.withhold_next_server_datagram = false;
                        self.withheld_server_datagram = Some(sequence);
                    } else {
                        self.send_ack(sequence).await?;
                    }
                }
                for frame in frames {
                    if !self.seen_reliable.insert(frame.reliable_index) {
                        continue;
                    }
                    let Some(payload) = self.accept_fragment(frame)? else {
                        continue;
                    };
                    if let Some(order) = payload.order_index {
                        accept_ordered(
                            &mut self.expected_ordered,
                            &mut self.ordered_holdback,
                            &mut self.ready_payloads,
                            order,
                            payload.payload,
                        );
                    } else {
                        self.ready_payloads.push_back(payload.payload);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn reply_application_keepalive(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.first().copied() != Some(BEDROCK_GAME_PACKET_ID) {
            return Ok(());
        }
        let mut batch = BedrockPacketBatch::decode(&payload[1..], self.compression.as_ref())?;
        while let Some(V924::NetworkStackLatencyPacket(packet)) =
            batch.next(NETWORK_STACK_LATENCY_PACKET_ID)?
        {
            if packet.is_from_server {
                self.send_packets(&[V924::NetworkStackLatencyPacket(Box::new(
                    NetworkStackLatencyPacket {
                        creation_time: packet.creation_time,
                        is_from_server: false,
                    },
                ))])
                .await?;
            }
        }
        Ok(())
    }

    fn accept_fragment(&mut self, frame: DecodedFrame) -> io::Result<Option<OrderedPayload>> {
        let Some(split) = frame.split else {
            return Ok(Some(OrderedPayload {
                order_index: frame.order_index,
                payload: frame.payload,
            }));
        };
        let count = usize::try_from(split.count)
            .map_err(|_| invalid_data("fragment count exceeds usize"))?;
        let index = usize::try_from(split.index)
            .map_err(|_| invalid_data("fragment index exceeds usize"))?;
        if count == 0 || count > MAX_REASSEMBLY_PARTS || index >= count {
            return Err(invalid_data("invalid or over-budget RakNet fragment range"));
        }
        let parts = self
            .reassembly
            .entry(split.id)
            .or_insert_with(|| vec![None; count]);
        if parts.len() != count {
            return Err(invalid_data(
                "RakNet fragment count changed during reassembly",
            ));
        }
        parts[index] = Some(frame.payload);
        if parts.iter().any(Option::is_none) {
            return Ok(None);
        }
        let parts = self
            .reassembly
            .remove(&split.id)
            .expect("complete oracle reassembly remains present");
        let mut payload = Vec::new();
        for part in parts {
            payload.extend(part.expect("complete oracle reassembly has every part"));
            if payload.len() > MAX_FRAME_BYTES {
                return Err(invalid_data(
                    "reassembled RakNet payload exceeds oracle budget",
                ));
            }
        }
        Ok(Some(OrderedPayload {
            order_index: frame.order_index,
            payload,
        }))
    }

    async fn reply_connected_ping(&mut self, payload: &[u8]) -> io::Result<()> {
        let ping_time = payload
            .get(1..9)
            .ok_or_else(|| invalid_data("truncated connected ping"))?;
        let mut pong = Vec::with_capacity(17);
        pong.push(0x03);
        pong.extend_from_slice(ping_time);
        pong.extend_from_slice(&epoch_millis().to_be_bytes());
        self.send_reliable_ordered(&pong).await
    }

    async fn send_ack(&self, sequence: u32) -> io::Result<()> {
        let mut ack = Vec::with_capacity(7);
        ack.push(0xc0);
        ack.extend_from_slice(&1_u16.to_be_bytes());
        ack.push(1);
        write_u24_le(&mut ack, sequence);
        self.socket.send(&ack).await?;
        Ok(())
    }

    async fn release_withheld_ack(&mut self) -> io::Result<()> {
        if let Some(sequence) = self.withheld_server_datagram.take() {
            self.send_ack(sequence).await?;
        }
        Ok(())
    }

    async fn retransmit_due(&mut self) -> io::Result<()> {
        let now = tokio::time::Instant::now();
        let due = self
            .sent_datagrams
            .iter()
            .filter_map(|(sequence, datagram)| {
                (now.duration_since(datagram.sent_at) >= oracle_retransmit_delay(datagram.attempts))
                    .then_some(*sequence)
            })
            .collect::<Vec<_>>();
        for sequence in due {
            self.retransmit(sequence).await?;
        }
        Ok(())
    }

    async fn maintain_outbound(&mut self) -> io::Result<()> {
        self.retransmit_due().await?;
        let now = tokio::time::Instant::now();
        if now >= self.next_keepalive {
            self.send_connected_ping().await?;
            self.next_keepalive = now + Duration::from_secs(4);
        }
        Ok(())
    }

    fn next_maintenance_delay(&self, now: tokio::time::Instant) -> Duration {
        let keepalive = self.next_keepalive.saturating_duration_since(now);
        self.sent_datagrams
            .values()
            .map(|datagram| {
                oracle_retransmit_delay(datagram.attempts)
                    .saturating_sub(now.saturating_duration_since(datagram.sent_at))
            })
            .min()
            .map_or(keepalive, |retransmit| retransmit.min(keepalive))
            .max(Duration::from_millis(1))
    }

    async fn retransmit(&mut self, sequence: u32) -> io::Result<()> {
        let datagram = self
            .sent_datagrams
            .get_mut(&sequence)
            .ok_or_else(|| invalid_data("NACK referenced an unknown oracle datagram"))?;
        if datagram.attempts >= MAX_RETRANSMIT_ATTEMPTS {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "RakNet oracle retransmit attempt budget exhausted",
            ));
        }
        self.socket.send(&datagram.bytes).await?;
        datagram.sent_at = tokio::time::Instant::now();
        datagram.attempts = datagram.attempts.saturating_add(1);
        Ok(())
    }
}

fn oracle_retransmit_delay(attempts: u16) -> Duration {
    let shift = u32::from(attempts.saturating_sub(1)).min(ORACLE_RETRANSMIT_MAX_BACKOFF_SHIFT);
    ORACLE_RETRANSMIT_BASE_DELAY
        .checked_mul(1_u32 << shift)
        .expect("bounded oracle retransmit delay should fit Duration")
}

fn accept_ordered(
    expected_ordered: &mut u32,
    holdback: &mut HashMap<u32, Vec<u8>>,
    ready: &mut VecDeque<Vec<u8>>,
    order: u32,
    payload: Vec<u8>,
) {
    if order == *expected_ordered {
        ready.push_back(payload);
        *expected_ordered = next_u24(*expected_ordered);
        while let Some(payload) = holdback.remove(expected_ordered) {
            ready.push_back(payload);
            *expected_ordered = next_u24(*expected_ordered);
        }
    } else if is_newer_u24(order, *expected_ordered) {
        holdback.entry(order).or_insert(payload);
    }
}

struct SentDatagram {
    bytes: Vec<u8>,
    sent_at: tokio::time::Instant,
    attempts: u16,
}

struct SplitHeader {
    count: u32,
    id: u16,
    index: u32,
}

struct DecodedFrame {
    reliable_index: u32,
    order_index: Option<u32>,
    split: Option<SplitHeader>,
    payload: Vec<u8>,
}

struct OrderedPayload {
    order_index: Option<u32>,
    payload: Vec<u8>,
}

async fn open_offline_session(socket: &UdpSocket, server_addr: SocketAddr) -> io::Result<()> {
    let mut request_one = Vec::with_capacity(ORACLE_MTU);
    request_one.push(0x05);
    request_one.extend_from_slice(&BEDROCK_RAKNET_MAGIC);
    request_one.push(V924::RAKNET_VERSION);
    request_one.resize(ORACLE_MTU, 0);
    let reply_one = exchange_offline(socket, &request_one, 0x06).await?;
    let mtu = u16::from_be_bytes(
        reply_one
            .get(reply_one.len().saturating_sub(2)..)
            .ok_or_else(|| invalid_data("truncated RakNet open connection reply 1"))?
            .try_into()
            .map_err(|_| invalid_data("invalid RakNet MTU field"))?,
    );

    let mut request_two = Vec::with_capacity(48);
    request_two.push(0x07);
    request_two.extend_from_slice(&BEDROCK_RAKNET_MAGIC);
    encode_address(&mut request_two, server_addr)?;
    request_two.extend_from_slice(&mtu.to_be_bytes());
    request_two.extend_from_slice(&client_guid(socket)?.to_be_bytes());
    let _reply_two = exchange_offline(socket, &request_two, 0x08).await?;
    Ok(())
}

async fn exchange_offline(
    socket: &UdpSocket,
    request: &[u8],
    expected_reply: u8,
) -> io::Result<Vec<u8>> {
    for _ in 0..OFFLINE_EXCHANGE_ATTEMPT_LIMIT {
        socket.send(request).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        loop {
            let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now())
            else {
                break;
            };
            let mut bytes = vec![0_u8; MAX_DATAGRAM_BYTES];
            let received = tokio::time::timeout(remaining, socket.recv(&mut bytes)).await;
            let length = match received {
                Ok(result) => result?,
                Err(_) => break,
            };
            bytes.truncate(length);
            if bytes.first().copied() == Some(expected_reply) {
                return Ok(bytes);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "RakNet offline reply 0x{expected_reply:02x} timed out after {OFFLINE_EXCHANGE_ATTEMPT_LIMIT} attempts"
        ),
    ))
}

fn encode_address(output: &mut Vec<u8>, address: SocketAddr) -> io::Result<()> {
    match address {
        SocketAddr::V4(address) => {
            output.push(4);
            output.extend(address.ip().octets().map(|octet| !octet));
            output.extend_from_slice(&address.port().to_be_bytes());
            Ok(())
        }
        SocketAddr::V6(_) => Err(invalid_data(
            "the Bedrock latency oracle currently requires an IPv4 listener",
        )),
    }
}

fn encode_frame(
    reliability: u8,
    reliable_index: u32,
    order_index: Option<u32>,
    split: Option<SplitHeader>,
    payload: &[u8],
) -> io::Result<Vec<u8>> {
    let bit_len = payload
        .len()
        .checked_mul(8)
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| invalid_data("RakNet frame exceeds u16 bit length"))?;
    let mut frame = Vec::with_capacity(payload.len().saturating_add(24));
    frame.push((reliability << 5) | u8::from(split.is_some()) * 0x10);
    frame.extend_from_slice(&bit_len.to_be_bytes());
    write_u24_le(&mut frame, reliable_index);
    if let Some(order_index) = order_index {
        write_u24_le(&mut frame, order_index);
        frame.push(0);
    }
    if let Some(split) = split {
        frame.extend_from_slice(&split.count.to_be_bytes());
        frame.extend_from_slice(&split.id.to_be_bytes());
        frame.extend_from_slice(&split.index.to_be_bytes());
    }
    frame.extend_from_slice(payload);
    Ok(frame)
}

fn decode_datagram(bytes: &[u8]) -> io::Result<(u32, Vec<DecodedFrame>)> {
    let mut reader = SliceReader::new(bytes);
    let flags = reader.u8()?;
    if flags & 0x80 == 0 || flags & 0x60 != 0 {
        return Err(invalid_data("invalid RakNet datagram flags"));
    }
    let sequence = reader.u24_le()?;
    let mut frames = Vec::new();
    while !reader.is_empty() {
        let frame_flags = reader.u8()?;
        let reliability = frame_flags >> 5;
        let payload_len = usize::from(reader.u16_be()?).div_ceil(8);
        if payload_len > MAX_FRAME_BYTES {
            return Err(invalid_data("RakNet frame exceeds oracle payload budget"));
        }
        let reliable_index = if matches!(reliability, 2 | 3 | 4 | 6 | 7) {
            reader.u24_le()?
        } else {
            return Err(invalid_data("server emitted a non-reliable RakNet frame"));
        };
        if matches!(reliability, 1 | 4) {
            let _ = reader.u24_le()?;
        }
        let order_index = if matches!(reliability, 1 | 3 | 4 | 7) {
            let index = reader.u24_le()?;
            let _channel = reader.u8()?;
            Some(index)
        } else {
            None
        };
        let split = if frame_flags & 0x10 != 0 {
            Some(SplitHeader {
                count: reader.u32_be()?,
                id: reader.u16_be()?,
                index: reader.u32_be()?,
            })
        } else {
            None
        };
        frames.push(DecodedFrame {
            reliable_index,
            order_index,
            split,
            payload: reader.bytes(payload_len)?.to_vec(),
        });
    }
    Ok((sequence, frames))
}

fn decode_ack_ranges(bytes: &[u8]) -> io::Result<Vec<(u32, u32)>> {
    let mut reader = SliceReader::new(bytes);
    let _kind = reader.u8()?;
    let count = usize::from(reader.u16_be()?);
    let mut ranges = Vec::with_capacity(count);
    for _ in 0..count {
        let single = reader.u8()? != 0;
        let start = reader.u24_le()?;
        let end = if single { start } else { reader.u24_le()? };
        ranges.push((start, end));
    }
    Ok(ranges)
}

/// A batch can satisfy several successive reads; returning one packet must not discard
/// later notifications in the same reliable payload.
#[derive(Default)]
struct BedrockPacketBatch {
    bytes: Vec<u8>,
    cursor: usize,
}

impl BedrockPacketBatch {
    fn decode(encoded: &[u8], compression: Option<&BedrockCompression>) -> io::Result<Self> {
        let bytes = match compression {
            Some(mode) => mode.decompress(encoded).map_err(codec_error)?,
            None => encoded.to_vec(),
        };
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(invalid_data("Bedrock batch exceeds oracle payload budget"));
        }
        Ok(Self { bytes, cursor: 0 })
    }

    fn next(&mut self, wanted_packet_id: u16) -> io::Result<Option<V924>> {
        let batch = &self.bytes;
        while self.cursor < batch.len() {
            let packet_len = usize::try_from(read_var_u32(batch, &mut self.cursor)?)
                .map_err(|_| invalid_data("Bedrock packet length exceeds usize"))?;
            let packet_end = self
                .cursor
                .checked_add(packet_len)
                .ok_or_else(|| invalid_data("Bedrock packet boundary overflowed"))?;
            let packet = batch
                .get(self.cursor..packet_end)
                .ok_or_else(|| invalid_data("truncated Bedrock packet batch"))?;
            let mut header_cursor = 0_usize;
            let packet_id = u16::try_from(read_var_u32(packet, &mut header_cursor)? & 0x3ff)
                .map_err(|_| invalid_data("Bedrock packet id exceeds u16"))?;
            self.cursor = packet_end;
            if packet_id == wanted_packet_id || packet_id == DISCONNECT_PACKET_ID {
                let mut single_packet_batch = Vec::with_capacity(packet_len.saturating_add(5));
                write_var_u32(
                    &mut single_packet_batch,
                    u32::try_from(packet_len)
                        .map_err(|_| invalid_data("Bedrock packet length exceeds u32"))?,
                );
                single_packet_batch.extend_from_slice(packet);
                let mut decoded =
                    decode_packet_batch::<V924>(&single_packet_batch, None).map_err(codec_error)?;
                if decoded.len() != 1 {
                    return Err(invalid_data(
                        "single-packet Bedrock batch decoded an invalid packet count",
                    ));
                }
                let packet = decoded.pop();
                if let Some(V924::DisconnectPacket(disconnect)) = &packet {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        format!(
                            "Bedrock server disconnected: {:?}; {}",
                            disconnect.reason,
                            disconnect
                                .message
                                .as_ref()
                                .map_or("", |message| message.kick_message.as_str()),
                        ),
                    ));
                }
                return Ok(packet);
            }
        }
        Ok(None)
    }
}

#[test]
fn application_disconnect_terminates_an_outstanding_gameplay_response() {
    use bedrock_protocol::v662::enums::ConnectionFailReason;
    use bedrock_protocol::v712::packets::{DisconnectMessage, DisconnectPacket};

    let packet = V924::DisconnectPacket(Box::new(DisconnectPacket {
        reason: ConnectionFailReason::Disconnected,
        message: Some(DisconnectMessage {
            kick_message: "keepalive timed out".to_owned(),
            filtered_message: "keepalive timed out".to_owned(),
        }),
    }));
    let batch = encode_packet_batch(&[packet], None).unwrap();
    let error = BedrockPacketBatch::decode(&batch, None)
        .unwrap()
        .next(PLAYER_HOTBAR_PACKET_ID)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    assert!(error.to_string().contains("keepalive timed out"));
}

fn read_var_u32(bytes: &[u8], cursor: &mut usize) -> io::Result<u32> {
    let mut value = 0_u32;
    for shift in (0..35).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| invalid_data("truncated Bedrock varuint"))?;
        *cursor = cursor
            .checked_add(1)
            .ok_or_else(|| invalid_data("Bedrock varuint cursor overflowed"))?;
        if shift == 28 && byte & 0xf0 != 0 {
            return Err(invalid_data("Bedrock varuint exceeds u32"));
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(invalid_data("Bedrock varuint exceeds five bytes"))
}

fn write_var_u32(output: &mut Vec<u8>, mut value: u32) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn bedrock_login_packet(
    username: &str,
    compression: Option<&BedrockCompression>,
) -> io::Result<Vec<u8>> {
    let protocol_version = i32::try_from(V924::PROTOCOL_VERSION)
        .map_err(|_| invalid_data("Bedrock protocol version exceeds i32"))?;
    let chain_entry = test_jwt(&json!({"extraData":{"displayName":username}}));
    let chain = json!({ "chain": [chain_entry] }).to_string();
    let client_jwt = test_jwt(&json!({"DisplayName":username}));
    let mut connection_request = Vec::new();
    connection_request.extend_from_slice(
        &u32::try_from(chain.len())
            .map_err(|_| invalid_data("Bedrock login chain exceeds u32"))?
            .to_le_bytes(),
    );
    connection_request.extend_from_slice(chain.as_bytes());
    connection_request.extend_from_slice(
        &u32::try_from(client_jwt.len())
            .map_err(|_| invalid_data("Bedrock client JWT exceeds u32"))?
            .to_le_bytes(),
    );
    connection_request.extend_from_slice(client_jwt.as_bytes());
    encode_packet_batch(
        &[V924::LoginPacket(Box::new(LoginPacket {
            client_network_version: protocol_version,
            connection_request,
        }))],
        compression,
    )
    .map_err(codec_error)
}

fn test_jwt(payload: &serde_json::Value) -> String {
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
    let payload =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
    format!("{header}.{payload}.")
}

fn client_guid(socket: &UdpSocket) -> io::Result<i64> {
    let local = socket.local_addr()?;
    let address = match local.ip() {
        IpAddr::V4(address) => u64::from(u32::from(address)),
        IpAddr::V6(address) => address
            .segments()
            .into_iter()
            .fold(0_u64, |value, segment| {
                value.rotate_left(7) ^ u64::from(segment)
            }),
    };
    Ok(i64::from_ne_bytes(
        (address ^ u64::from(local.port())).to_ne_bytes(),
    ))
}

fn epoch_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

fn take_u24(value: &mut u32) -> u32 {
    let current = *value & 0x00ff_ffff;
    *value = next_u24(current);
    current
}

const fn next_u24(value: u32) -> u32 {
    value.wrapping_add(1) & 0x00ff_ffff
}

fn is_newer_u24(candidate: u32, base: u32) -> bool {
    let distance = candidate.wrapping_sub(base) & 0x00ff_ffff;
    distance != 0 && distance < 0x0080_0000
}

fn in_u24_range(value: u32, start: u32, end: u32) -> bool {
    (value.wrapping_sub(start) & 0x00ff_ffff) <= (end.wrapping_sub(start) & 0x00ff_ffff)
}

fn write_u24_le(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes()[..3]);
}

fn codec_error(error: impl std::fmt::Display) -> io::Error {
    invalid_data(format!("Bedrock packet codec failed: {error}"))
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[test]
fn consecutive_ordered_frames_remain_queued_until_consumed() {
    let mut expected = 0;
    let mut holdback = HashMap::new();
    let mut ready = VecDeque::new();

    accept_ordered(&mut expected, &mut holdback, &mut ready, 0, vec![1]);
    accept_ordered(&mut expected, &mut holdback, &mut ready, 1, vec![2]);

    assert_eq!(expected, 2);
    assert_eq!(ready, VecDeque::from([vec![1], vec![2]]));
}

#[test]
fn oracle_retransmit_delay_backs_off_and_caps() {
    assert_eq!(oracle_retransmit_delay(1), Duration::from_millis(200));
    assert_eq!(oracle_retransmit_delay(2), Duration::from_millis(400));
    assert_eq!(oracle_retransmit_delay(3), Duration::from_millis(800));
    assert_eq!(oracle_retransmit_delay(4), Duration::from_millis(1_600));
    assert_eq!(
        oracle_retransmit_delay(MAX_RETRANSMIT_ATTEMPTS),
        Duration::from_millis(1_600)
    );
}

#[tokio::test]
async fn command_observes_fully_retained_acknowledged_datagram() -> io::Result<()> {
    let server = UdpSocket::bind("127.0.0.1:0").await?;
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
    socket.connect(server.local_addr()?).await?;
    server.connect(socket.local_addr()?).await?;
    let mut oracle = BedrockOracle {
        socket,
        next_datagram: 0,
        next_reliable: 0,
        next_ordered: 0,
        sent_datagrams: HashMap::new(),
        seen_reliable: HashSet::new(),
        reassembly: HashMap::new(),
        expected_ordered: 0,
        ordered_holdback: HashMap::new(),
        ready_payloads: VecDeque::new(),
        pending_batch: BedrockPacketBatch::default(),
        withheld_server_datagram: None,
        withhold_next_server_datagram: false,
        compression: None,
        actor_runtime_id: None,
        next_keepalive: tokio::time::Instant::now() + Duration::from_secs(4),
    };
    let mut datagram = vec![0x84, 0, 0, 0];
    // The future ordered frame must remain in holdback when a command arrives after its ACK.
    datagram.extend(encode_frame(3, 0, Some(1), None, &[0x03])?);
    server.send(&datagram).await?;
    let command = async {
        let mut bytes = [0; MAX_DATAGRAM_BYTES];
        let length = server.recv(&mut bytes).await?;
        assert_eq!(decode_ack_ranges(&bytes[..length])?, vec![(0, 0)]);
        io::Result::Ok(())
    };
    tokio::time::timeout(Duration::from_secs(1), oracle.receive_command(command))
        .await
        .map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "oracle did not service command")
        })???;
    assert!(oracle.seen_reliable.contains(&0));
    assert_eq!(oracle.ordered_holdback.get(&1), Some(&vec![0x03]));
    Ok(())
}

#[tokio::test]
async fn hotbar_response_drains_stale_resync_without_retransmitting_commands() -> io::Result<()> {
    let server = UdpSocket::bind("127.0.0.1:0").await?;
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
    socket.connect(server.local_addr()?).await?;
    let mut oracle = BedrockOracle {
        socket,
        next_datagram: 0,
        next_reliable: 0,
        next_ordered: 0,
        sent_datagrams: HashMap::new(),
        seen_reliable: HashSet::new(),
        reassembly: HashMap::new(),
        expected_ordered: 0,
        ordered_holdback: HashMap::new(),
        ready_payloads: VecDeque::new(),
        pending_batch: BedrockPacketBatch::default(),
        withheld_server_datagram: None,
        withhold_next_server_datagram: false,
        compression: None,
        actor_runtime_id: Some(1),
        next_keepalive: tokio::time::Instant::now() + Duration::from_secs(4),
    };
    // Uncompressed batch: four-byte packet, id 48, varint slot, inventory id, select=true.
    let mut batch = vec![0xfe];
    for _ in 0..32 {
        batch.extend_from_slice(&[4, 48, 6, 0, 1]);
    }
    batch.extend_from_slice(&[4, 48, 1, 0, 1]);
    oracle.ready_payloads.push_back(batch);
    oracle.begin_roundtrip(1).await?;
    oracle
        .await_selected_slot(
            1,
            tokio::time::Instant::now() + Duration::from_secs(1),
            Duration::from_millis(250),
        )
        .await?;
    assert_eq!(oracle.next_ordered, 1);
    assert!(oracle.ready_payloads.is_empty());

    oracle.ready_payloads.push_back(vec![0xfe, 4, 48, 6, 0, 1]);
    let error = oracle
        .await_selected_slot(
            1,
            tokio::time::Instant::now() + Duration::from_millis(10),
            Duration::from_millis(250),
        )
        .await
        .expect_err("old slots alone must not satisfy the current command");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(oracle.next_ordered, 1);

    // Without an old notification, the network deadline applies even when the response
    // arrives after the shorter resync settle window. Use actual UDP delivery here.
    let oracle_addr = oracle.socket.local_addr()?;
    let delayed_response = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut datagram = vec![0x84, 0, 0, 0];
        datagram.extend(encode_frame(3, 0, Some(0), None, &[0xfe, 4, 48, 1, 0, 1])?);
        server.send_to(&datagram, oracle_addr).await?;
        io::Result::Ok(())
    };
    tokio::try_join!(
        delayed_response,
        oracle.await_selected_slot(
            1,
            tokio::time::Instant::now() + Duration::from_secs(2),
            Duration::from_millis(250),
        ),
    )?;
    assert_eq!(oracle.next_ordered, 1);
    Ok(())
}

struct SliceReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> SliceReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn is_empty(&self) -> bool {
        self.cursor == self.bytes.len()
    }

    fn bytes(&mut self, length: usize) -> io::Result<&'a [u8]> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or_else(|| invalid_data("RakNet oracle cursor overflowed"))?;
        let bytes = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| invalid_data("truncated RakNet packet"))?;
        self.cursor = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u16_be(&mut self) -> io::Result<u16> {
        Ok(u16::from_be_bytes(
            self.bytes(2)?
                .try_into()
                .map_err(|_| invalid_data("truncated RakNet u16"))?,
        ))
    }

    fn u24_le(&mut self) -> io::Result<u32> {
        let bytes = self.bytes(3)?;
        Ok(u32::from(bytes[0]) | u32::from(bytes[1]) << 8 | u32::from(bytes[2]) << 16)
    }

    fn u32_be(&mut self) -> io::Result<u32> {
        Ok(u32::from_be_bytes(
            self.bytes(4)?
                .try_into()
                .map_err(|_| invalid_data("truncated RakNet u32"))?,
        ))
    }
}
