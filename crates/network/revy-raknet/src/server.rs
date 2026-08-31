use crate::peer::{PeerControl, PeerSnapshot, RakNetPeer, Running, spawn_peer};
use crate::wire::{
    OfflineRequest, decode_offline_request, encode_incompatible_protocol,
    encode_open_connection_reply_1, encode_open_connection_reply_2, encode_unconnected_pong,
};
use crate::{RakNetBudgets, RakNetError};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

pub struct Bound;
pub struct Serving;
pub struct Frozen;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub bind_addr: SocketAddr,
    pub motd: String,
    pub server_name: String,
    pub game_version: String,
    pub protocol_number: u32,
    pub raknet_version: u8,
    pub max_players: u32,
    pub online_players: u32,
    pub server_guid: i64,
    pub budgets: RakNetBudgets,
}

pub struct RakNetServer<State> {
    shared: Arc<ServerShared>,
    serving: Option<ServingState>,
    frozen: Option<ServerSnapshot>,
    state: PhantomData<State>,
}

struct ServerShared {
    socket: Arc<UdpSocket>,
    config: ServerConfig,
}

struct ServingState {
    accept_rx: mpsc::Receiver<RakNetPeer<Running>>,
    control_tx: mpsc::Sender<RouterCommand>,
}

#[derive(Clone, Debug)]
pub struct ServerSnapshot {
    pub local_addr: SocketAddr,
    pub peers: Vec<PeerSnapshot>,
}

impl RakNetServer<Bound> {
    pub async fn bind(config: ServerConfig) -> Result<Self, RakNetError> {
        let socket = Arc::new(UdpSocket::bind(config.bind_addr).await?);
        Ok(Self {
            shared: Arc::new(ServerShared { socket, config }),
            serving: None,
            frozen: None,
            state: PhantomData,
        })
    }

    #[must_use]
    pub fn start(self) -> RakNetServer<Serving> {
        let accept_capacity = self.shared.config.budgets.max_peers.max(1);
        let (accept_tx, accept_rx) = mpsc::channel(accept_capacity);
        let (control_tx, control_rx) = mpsc::channel(1);
        let shared = Arc::clone(&self.shared);
        tokio::spawn(async move {
            Router::new(shared, accept_tx).run(control_rx).await;
        });
        RakNetServer {
            shared: self.shared,
            serving: Some(ServingState {
                accept_rx,
                control_tx,
            }),
            frozen: None,
            state: PhantomData,
        }
    }
}

impl RakNetServer<Serving> {
    pub async fn accept(&mut self) -> Result<RakNetPeer<Running>, RakNetError> {
        self.serving
            .as_mut()
            .expect("serving server should own its accept channel")
            .accept_rx
            .recv()
            .await
            .ok_or(RakNetError::Closed)
    }

    pub async fn freeze(mut self) -> Result<RakNetServer<Frozen>, RakNetError> {
        let serving = self
            .serving
            .take()
            .expect("serving server should own router control");
        let (reply_tx, reply_rx) = oneshot::channel();
        serving
            .control_tx
            .send(RouterCommand::Freeze { reply_tx })
            .await
            .map_err(|_| RakNetError::Closed)?;
        let snapshot = reply_rx.await.map_err(|_| RakNetError::Closed)??;
        Ok(RakNetServer {
            shared: self.shared,
            serving: None,
            frozen: Some(snapshot),
            state: PhantomData,
        })
    }
}

impl RakNetServer<Frozen> {
    #[must_use]
    pub fn snapshot(&self) -> &ServerSnapshot {
        self.frozen
            .as_ref()
            .expect("frozen server should own its sealed snapshot")
    }
}

impl<State> RakNetServer<State> {
    pub fn local_addr(&self) -> Result<SocketAddr, RakNetError> {
        Ok(self.shared.socket.local_addr()?)
    }
}

enum RouterCommand {
    Freeze {
        reply_tx: oneshot::Sender<Result<ServerSnapshot, RakNetError>>,
    },
}

struct Router {
    shared: Arc<ServerShared>,
    accept_tx: mpsc::Sender<RakNetPeer<Running>>,
    peers: HashMap<SocketAddr, PeerControl>,
    closed_tx: mpsc::UnboundedSender<SocketAddr>,
    closed_rx: mpsc::UnboundedReceiver<SocketAddr>,
}

impl Router {
    fn new(shared: Arc<ServerShared>, accept_tx: mpsc::Sender<RakNetPeer<Running>>) -> Self {
        let (closed_tx, closed_rx) = mpsc::unbounded_channel();
        Self {
            shared,
            accept_tx,
            peers: HashMap::new(),
            closed_tx,
            closed_rx,
        }
    }

    async fn run(mut self, mut control_rx: mpsc::Receiver<RouterCommand>) {
        let mut receive_buffer = vec![0_u8; self.shared.config.budgets.max_datagram_bytes];
        loop {
            tokio::select! {
                command = control_rx.recv() => {
                    let Some(RouterCommand::Freeze { reply_tx }) = command else {
                        break;
                    };
                    let _ = reply_tx.send(self.freeze_peers().await);
                    break;
                }
                closed = self.closed_rx.recv() => {
                    if let Some(remote_addr) = closed {
                        self.peers.remove(&remote_addr);
                    }
                }
                received = self.shared.socket.recv_from(&mut receive_buffer) => {
                    let Ok((length, remote_addr)) = received else {
                        break;
                    };
                    let bytes = &receive_buffer[..length];
                    if self.route_packet(remote_addr, bytes).await.is_err() {
                        self.peers.remove(&remote_addr);
                    }
                }
            }
        }
    }

    async fn route_packet(
        &mut self,
        remote_addr: SocketAddr,
        bytes: &[u8],
    ) -> Result<(), RakNetError> {
        if let Some(request) = decode_offline_request(bytes)? {
            return self.handle_offline(remote_addr, request).await;
        }
        if let Some(peer) = self.peers.get(&remote_addr) {
            peer.deliver(bytes.to_vec()).await?;
        }
        Ok(())
    }

    async fn handle_offline(
        &mut self,
        remote_addr: SocketAddr,
        request: OfflineRequest,
    ) -> Result<(), RakNetError> {
        match request {
            OfflineRequest::Ping { time } => {
                let response =
                    encode_unconnected_pong(time, self.shared.config.server_guid, &self.motd())?;
                self.shared.socket.send_to(&response, remote_addr).await?;
            }
            OfflineRequest::OpenConnection1 { protocol, mtu } => {
                let response = if protocol == self.shared.config.raknet_version {
                    encode_open_connection_reply_1(
                        self.shared.config.server_guid,
                        self.clamp_mtu(mtu),
                    )
                } else {
                    encode_incompatible_protocol(
                        self.shared.config.raknet_version,
                        self.shared.config.server_guid,
                    )
                };
                self.shared.socket.send_to(&response, remote_addr).await?;
            }
            OfflineRequest::OpenConnection2 { mtu, client_guid } => {
                let _client_identity = client_guid;
                if !self.peers.contains_key(&remote_addr) {
                    if self.peers.len() >= self.shared.config.budgets.max_peers {
                        return Err(crate::BudgetExceeded {
                            resource: "raknet peers",
                            requested: self.peers.len().saturating_add(1),
                            limit: self.shared.config.budgets.max_peers,
                        }
                        .into());
                    }
                    let peer = spawn_peer(
                        Arc::clone(&self.shared.socket),
                        remote_addr,
                        self.clamp_mtu(mtu),
                        self.shared.config.budgets,
                        self.accept_tx.clone(),
                        self.closed_tx.clone(),
                    );
                    self.peers.insert(remote_addr, peer.control);
                }
                let response = encode_open_connection_reply_2(
                    self.shared.config.server_guid,
                    remote_addr,
                    self.clamp_mtu(mtu),
                );
                self.shared.socket.send_to(&response, remote_addr).await?;
            }
        }
        Ok(())
    }

    async fn freeze_peers(&self) -> Result<ServerSnapshot, RakNetError> {
        let mut snapshots = JoinSet::new();
        for control in self.peers.values().cloned() {
            snapshots.spawn(async move { control.freeze().await });
        }
        let mut peers = Vec::with_capacity(self.peers.len());
        while let Some(result) = snapshots.join_next().await {
            peers.push(result.map_err(|error| {
                RakNetError::Wire(format!("raknet peer freeze task failed: {error}"))
            })??);
        }
        Ok(ServerSnapshot {
            local_addr: self.shared.socket.local_addr()?,
            peers,
        })
    }

    fn clamp_mtu(&self, mtu: u16) -> u16 {
        let maximum =
            u16::try_from(self.shared.config.budgets.max_datagram_bytes).unwrap_or(u16::MAX);
        mtu.clamp(576, maximum.max(576))
    }

    fn motd(&self) -> String {
        format!(
            "MCPE;{};{};{};{};{};{};{};Survival;1;{};{};",
            self.shared.config.motd,
            self.shared.config.protocol_number,
            self.shared.config.game_version,
            self.shared.config.online_players,
            self.shared.config.max_players,
            self.shared.config.server_guid,
            self.shared.config.server_name,
            self.shared.config.bind_addr.port(),
            self.shared.config.bind_addr.port(),
        )
    }
}
