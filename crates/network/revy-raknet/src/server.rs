use crate::peer::{
    Frozen as FrozenPeer, PeerControl, PeerSnapshot, RakNetPeer, Running, ValidatedPeerSnapshot,
    prepare_peer_snapshots, spawn_peer,
};
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
pub struct ReceivePaused;
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

/// A failed server typestate transition that retains the pre-transition authority.
pub struct ServerFreezeError<State> {
    server: RakNetServer<State>,
    error: RakNetError,
}

impl<State> ServerFreezeError<State> {
    #[must_use]
    pub fn into_parts(self) -> (RakNetServer<State>, RakNetError) {
        (self.server, self.error)
    }
}

impl<State> std::fmt::Debug for ServerFreezeError<State> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerFreezeError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<State> std::fmt::Display for ServerFreezeError<State> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl<State> std::error::Error for ServerFreezeError<State> {}

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
        Self::from_socket(config, socket)
    }

    /// Constructs a bound server from an already-imported UDP socket authority.
    ///
    /// # Errors
    ///
    /// Returns [`RakNetError`] when the socket address is incompatible with the validated
    /// listener configuration.
    pub fn from_socket(config: ServerConfig, socket: Arc<UdpSocket>) -> Result<Self, RakNetError> {
        let local_addr = socket.local_addr()?;
        let configured = config.bind_addr;
        if configured.port() != 0 && configured.port() != local_addr.port()
            || !configured.ip().is_unspecified() && configured.ip() != local_addr.ip()
        {
            return Err(RakNetError::Wire(format!(
                "imported UDP socket {local_addr} does not match configured listener {configured}"
            )));
        }
        Ok(Self {
            shared: Arc::new(ServerShared { socket, config }),
            serving: None,
            frozen: None,
            state: PhantomData,
        })
    }

    #[must_use]
    pub fn start(self) -> RakNetServer<Serving> {
        self.start_with_receive_authority(true)
    }

    /// Starts the socket router without granting receive authority.
    ///
    /// A pre-bound candidate listener uses this state until its runtime epoch is committed.
    #[must_use]
    pub fn start_paused(self) -> RakNetServer<ReceivePaused> {
        self.start_with_receive_authority(false)
    }

    fn start_with_receive_authority<State>(self, receive_enabled: bool) -> RakNetServer<State> {
        let accept_capacity = self.shared.config.budgets.max_peers.max(1);
        let (accept_tx, accept_rx) = mpsc::channel(accept_capacity);
        let (control_tx, control_rx) = mpsc::channel(1);
        let shared = Arc::clone(&self.shared);
        tokio::spawn(async move {
            Router::new(shared, accept_tx)
                .run(control_rx, !receive_enabled)
                .await;
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
        let peer = self
            .serving
            .as_mut()
            .expect("serving server should own its accept channel")
            .accept_rx
            .recv()
            .await
            .ok_or(RakNetError::Closed)?;
        peer.claim_session_authority();
        Ok(peer)
    }

    pub async fn freeze(mut self) -> Result<RakNetServer<Frozen>, ServerFreezeError<Serving>> {
        let serving = self
            .serving
            .take()
            .expect("serving server should own router control");
        let (reply_tx, reply_rx) = oneshot::channel();
        if serving
            .control_tx
            .send(RouterCommand::Freeze { reply_tx })
            .await
            .is_err()
        {
            return Err(ServerFreezeError {
                server: RakNetServer {
                    shared: self.shared,
                    serving: Some(serving),
                    frozen: None,
                    state: PhantomData,
                },
                error: RakNetError::Closed,
            });
        }
        let snapshot = match reply_rx.await {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                return Err(ServerFreezeError {
                    server: RakNetServer {
                        shared: self.shared,
                        serving: Some(serving),
                        frozen: None,
                        state: PhantomData,
                    },
                    error,
                });
            }
            Err(_) => {
                return Err(ServerFreezeError {
                    server: RakNetServer {
                        shared: self.shared,
                        serving: Some(serving),
                        frozen: None,
                        state: PhantomData,
                    },
                    error: RakNetError::Closed,
                });
            }
        };
        Ok(RakNetServer {
            shared: self.shared,
            serving: Some(serving),
            frozen: Some(snapshot),
            state: PhantomData,
        })
    }

    pub async fn pause_receive(mut self) -> Result<RakNetServer<ReceivePaused>, RakNetError> {
        let serving = self
            .serving
            .take()
            .expect("serving server should own router control");
        let (reply_tx, reply_rx) = oneshot::channel();
        serving
            .control_tx
            .send(RouterCommand::PauseReceive { reply_tx })
            .await
            .map_err(|_| RakNetError::Closed)?;
        reply_rx.await.map_err(|_| RakNetError::Closed)?;
        Ok(RakNetServer {
            shared: self.shared,
            serving: Some(serving),
            frozen: None,
            state: PhantomData,
        })
    }
}

impl RakNetServer<ReceivePaused> {
    /// Freezes every router-owned peer while receive authority is already withheld.
    ///
    /// This is the executable-transfer seal transition: pausing receive prevents new datagrams
    /// from entering the router, while this operation also stops retransmit and timeout mutation
    /// and returns the exact peer checkpoint retained by the frozen server capability.
    pub async fn freeze(
        mut self,
    ) -> Result<RakNetServer<Frozen>, ServerFreezeError<ReceivePaused>> {
        let serving = self
            .serving
            .take()
            .expect("receive-paused server should retain router control");
        let (reply_tx, reply_rx) = oneshot::channel();
        if serving
            .control_tx
            .send(RouterCommand::Freeze { reply_tx })
            .await
            .is_err()
        {
            return Err(ServerFreezeError {
                server: RakNetServer {
                    shared: self.shared,
                    serving: Some(serving),
                    frozen: None,
                    state: PhantomData,
                },
                error: RakNetError::Closed,
            });
        }
        let snapshot = match reply_rx.await {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                return Err(ServerFreezeError {
                    server: RakNetServer {
                        shared: self.shared,
                        serving: Some(serving),
                        frozen: None,
                        state: PhantomData,
                    },
                    error,
                });
            }
            Err(_) => {
                return Err(ServerFreezeError {
                    server: RakNetServer {
                        shared: self.shared,
                        serving: Some(serving),
                        frozen: None,
                        state: PhantomData,
                    },
                    error: RakNetError::Closed,
                });
            }
        };
        Ok(RakNetServer {
            shared: self.shared,
            serving: Some(serving),
            frozen: Some(snapshot),
            state: PhantomData,
        })
    }

    /// Imports validated peer checkpoints while the child router has no receive authority.
    ///
    /// Accepted peers are returned as frozen capabilities for session actors. Offline-handshake
    /// peers remain router-owned and are resumed atomically with receive authority.
    ///
    /// # Errors
    ///
    /// Returns [`RakNetError`] without importing any peer when a checkpoint is duplicated,
    /// malformed, or exceeds the configured budgets.
    pub async fn import_peers(
        &mut self,
        peers: Vec<ValidatedPeerSnapshot>,
    ) -> Result<Vec<RakNetPeer<FrozenPeer>>, RakNetError> {
        let serving = self
            .serving
            .as_ref()
            .expect("receive-paused server should retain router control");
        let (reply_tx, reply_rx) = oneshot::channel();
        serving
            .control_tx
            .send(RouterCommand::ImportPeers { peers, reply_tx })
            .await
            .map_err(|_| RakNetError::Closed)?;
        let imported = reply_rx.await.map_err(|_| RakNetError::Closed)??;
        Ok(imported)
    }

    pub async fn resume_receive(mut self) -> Result<RakNetServer<Serving>, RakNetError> {
        let serving = self
            .serving
            .take()
            .expect("receive-paused server should retain router control");
        let (reply_tx, reply_rx) = oneshot::channel();
        serving
            .control_tx
            .send(RouterCommand::ResumeReceive { reply_tx })
            .await
            .map_err(|_| RakNetError::Closed)?;
        reply_rx.await.map_err(|_| RakNetError::Closed)??;
        Ok(RakNetServer {
            shared: self.shared,
            serving: Some(serving),
            frozen: None,
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

    pub async fn resume(mut self) -> Result<RakNetServer<Serving>, RakNetError> {
        let serving = self
            .serving
            .take()
            .expect("frozen server should retain router control");
        let (reply_tx, reply_rx) = oneshot::channel();
        serving
            .control_tx
            .send(RouterCommand::Resume { reply_tx })
            .await
            .map_err(|_| RakNetError::Closed)?;
        reply_rx.await.map_err(|_| RakNetError::Closed)??;
        Ok(RakNetServer {
            shared: self.shared,
            serving: Some(serving),
            frozen: None,
            state: PhantomData,
        })
    }

    pub async fn mark_transferred(mut self) -> Result<ServerSnapshot, RakNetError> {
        let serving = self
            .serving
            .take()
            .expect("frozen server should retain router control");
        let (reply_tx, reply_rx) = oneshot::channel();
        serving
            .control_tx
            .send(RouterCommand::Transfer { reply_tx })
            .await
            .map_err(|_| RakNetError::Closed)?;
        reply_rx.await.map_err(|_| RakNetError::Closed)??;
        self.frozen
            .take()
            .ok_or_else(|| RakNetError::Wire("frozen server lost its sealed snapshot".to_string()))
    }
}

impl<State> RakNetServer<State> {
    pub fn local_addr(&self) -> Result<SocketAddr, RakNetError> {
        Ok(self.shared.socket.local_addr()?)
    }
}

#[cfg(unix)]
impl<State> std::os::fd::AsFd for RakNetServer<State> {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.shared.socket.as_fd()
    }
}

#[cfg(windows)]
impl<State> std::os::windows::io::AsRawSocket for RakNetServer<State> {
    fn as_raw_socket(&self) -> std::os::windows::io::RawSocket {
        self.shared.socket.as_raw_socket()
    }
}

enum RouterCommand {
    PauseReceive {
        reply_tx: oneshot::Sender<()>,
    },
    ResumeReceive {
        reply_tx: oneshot::Sender<Result<(), RakNetError>>,
    },
    ImportPeers {
        peers: Vec<ValidatedPeerSnapshot>,
        reply_tx: oneshot::Sender<Result<Vec<RakNetPeer<FrozenPeer>>, RakNetError>>,
    },
    Freeze {
        reply_tx: oneshot::Sender<Result<ServerSnapshot, RakNetError>>,
    },
    Resume {
        reply_tx: oneshot::Sender<Result<(), RakNetError>>,
    },
    Transfer {
        reply_tx: oneshot::Sender<Result<(), RakNetError>>,
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

    async fn run(
        mut self,
        mut control_rx: mpsc::Receiver<RouterCommand>,
        mut receive_paused: bool,
    ) {
        let mut receive_buffer = vec![0_u8; self.shared.config.budgets.max_datagram_bytes];
        let mut frozen = false;
        loop {
            if frozen || receive_paused {
                let Some(command) = control_rx.recv().await else {
                    break;
                };
                match command {
                    RouterCommand::PauseReceive { reply_tx } => {
                        receive_paused = true;
                        let _ = reply_tx.send(());
                    }
                    RouterCommand::ResumeReceive { reply_tx } => {
                        let result = self.resume_peers().await;
                        if result.is_ok() {
                            receive_paused = false;
                        }
                        let _ = reply_tx.send(result);
                    }
                    RouterCommand::ImportPeers { peers, reply_tx } => {
                        let _ = reply_tx.send(self.import_peers(peers));
                    }
                    RouterCommand::Freeze { reply_tx } => {
                        let result = if frozen {
                            self.snapshot_peers()
                        } else {
                            self.freeze_peers().await
                        };
                        if result.is_ok() {
                            frozen = true;
                        }
                        let _ = reply_tx.send(result);
                    }
                    RouterCommand::Resume { reply_tx } => {
                        let result = self.resume_peers().await;
                        if result.is_ok() {
                            frozen = false;
                            receive_paused = false;
                        }
                        let _ = reply_tx.send(result);
                    }
                    RouterCommand::Transfer { reply_tx } => {
                        let result = self.transfer_peers().await;
                        let should_stop = result.is_ok();
                        let _ = reply_tx.send(result);
                        if should_stop {
                            break;
                        }
                    }
                }
                continue;
            }
            tokio::select! {
                command = control_rx.recv() => {
                    let Some(command) = command else {
                        break;
                    };
                    match command {
                        RouterCommand::PauseReceive { reply_tx } => {
                            receive_paused = true;
                            let _ = reply_tx.send(());
                        }
                        RouterCommand::ResumeReceive { reply_tx } => {
                            let _ = reply_tx.send(Ok(()));
                        }
                        RouterCommand::ImportPeers { reply_tx, .. } => {
                            let _ = reply_tx.send(Err(RakNetError::Wire(
                                "peer checkpoints require a receive-paused router".to_string(),
                            )));
                        }
                        RouterCommand::Freeze { reply_tx } => {
                            let result = self.freeze_peers().await;
                            if result.is_ok() {
                                frozen = true;
                            }
                            let _ = reply_tx.send(result);
                        }
                        RouterCommand::Resume { reply_tx } => {
                            receive_paused = false;
                            let _ = reply_tx.send(Ok(()));
                        }
                        RouterCommand::Transfer { reply_tx } => {
                            let result = self.transfer_peers().await;
                            let should_stop = result.is_ok();
                            let _ = reply_tx.send(result);
                            if should_stop {
                                break;
                            }
                        }
                    }
                }
                closed = self.closed_rx.recv() => {
                    if let Some(remote_addr) = closed {
                        self.peers.remove(&remote_addr);
                    }
                }
                received = self.shared.socket.recv_from(&mut receive_buffer) => {
                    let (length, remote_addr) = match received {
                        Ok(received) => received,
                        Err(error) if recoverable_udp_receive_error(&error) => continue,
                        Err(error) => {
                            eprintln!("RakNet socket router stopped while receiving: {error}");
                            break;
                        }
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
            match peer.deliver(bytes.to_vec()) {
                Ok(()) | Err(RakNetError::QueueFull) => {}
                Err(error) => return Err(error),
            }
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
            if !control.is_router_owned() {
                continue;
            }
            snapshots.spawn(async move { control.freeze_for_transfer().await });
        }
        let mut peers = Vec::with_capacity(self.peers.len());
        let mut failure = None;
        while let Some(result) = snapshots.join_next().await {
            match result {
                Ok(Ok(snapshot)) => peers.push(snapshot),
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(error) => {
                    failure.get_or_insert_with(|| {
                        RakNetError::Wire(format!("raknet peer freeze task failed: {error}"))
                    });
                }
            };
        }
        if let Some(error) = failure {
            let compensation = self.resume_peer_controls().await;
            return match compensation {
                Ok(()) => Err(error),
                Err(compensation) => Err(RakNetError::Wire(format!(
                    "raknet peer freeze failed: {error}; resume compensation failed: {compensation}"
                ))),
            };
        }
        Ok(ServerSnapshot {
            local_addr: self.shared.socket.local_addr()?,
            peers,
        })
    }

    fn import_peers(
        &mut self,
        mut snapshots: Vec<ValidatedPeerSnapshot>,
    ) -> Result<Vec<RakNetPeer<FrozenPeer>>, RakNetError> {
        if snapshots
            .iter()
            .any(|snapshot| snapshot.budgets() != self.shared.config.budgets)
        {
            return Err(RakNetError::Wire(
                "RakNet peer checkpoint was validated for different resource budgets".to_string(),
            ));
        }
        let requested = self.peers.len().saturating_add(snapshots.len());
        if requested > self.shared.config.budgets.max_peers {
            return Err(crate::BudgetExceeded {
                resource: "raknet peers",
                requested,
                limit: self.shared.config.budgets.max_peers,
            }
            .into());
        }
        snapshots.sort_by_key(ValidatedPeerSnapshot::remote_addr);
        let mut addresses = std::collections::HashSet::with_capacity(snapshots.len());
        for snapshot in &snapshots {
            if self.peers.contains_key(&snapshot.remote_addr())
                || !addresses.insert(snapshot.remote_addr())
            {
                return Err(RakNetError::Wire(format!(
                    "duplicate transferred RakNet peer {}",
                    snapshot.remote_addr()
                )));
            }
        }
        let prepared = prepare_peer_snapshots(
            Arc::clone(&self.shared.socket),
            snapshots,
            self.shared.config.budgets,
            self.accept_tx.clone(),
        )?;
        let mut imported = Vec::new();
        for prepared_peer in prepared {
            let remote_addr = prepared_peer.remote_addr();
            let peer = prepared_peer.spawn(self.closed_tx.clone());
            self.peers.insert(remote_addr, peer.control);
            if let Some(peer) = peer.imported {
                peer.claim_session_authority();
                imported.push(peer);
            }
        }
        Ok(imported)
    }

    fn snapshot_peers(&self) -> Result<ServerSnapshot, RakNetError> {
        Err(RakNetError::Wire(
            "a repeated server freeze must reuse the snapshot held by RakNetServer<Frozen>"
                .to_string(),
        ))
    }

    async fn resume_peers(&self) -> Result<(), RakNetError> {
        let Err(error) = self.resume_peer_controls().await else {
            return Ok(());
        };
        if let Err(compensation) = self.freeze_peer_controls().await {
            return Err(RakNetError::Wire(format!(
                "raknet peer resume failed: {error}; freeze compensation failed: {compensation}"
            )));
        }
        Err(error)
    }

    async fn resume_peer_controls(&self) -> Result<(), RakNetError> {
        for control in self.peers.values().cloned() {
            if !control.is_router_owned() {
                continue;
            }
            control.resume()?;
        }
        Ok(())
    }

    async fn freeze_peer_controls(&self) -> Result<(), RakNetError> {
        let mut freezes = JoinSet::new();
        for control in self.peers.values().cloned() {
            if !control.is_router_owned() {
                continue;
            }
            freezes.spawn(async move { control.freeze().await.map(|_| ()) });
        }
        collect_peer_control_results(freezes, "freeze compensation").await
    }

    async fn transfer_peers(&self) -> Result<(), RakNetError> {
        let mut transfers = JoinSet::new();
        for control in self.peers.values().cloned() {
            if !control.is_router_owned() {
                continue;
            }
            transfers.spawn(async move { control.mark_transferred().await });
        }
        while let Some(result) = transfers.join_next().await {
            result.map_err(|error| {
                RakNetError::Wire(format!("raknet peer transfer task failed: {error}"))
            })??;
        }
        Ok(())
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

fn recoverable_udp_receive_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionRefused
    )
}

async fn collect_peer_control_results(
    mut tasks: JoinSet<Result<(), RakNetError>>,
    operation: &'static str,
) -> Result<(), RakNetError> {
    let mut failures = Vec::new();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failures.push(error.to_string()),
            Err(error) => failures.push(format!("task failed: {error}")),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(RakNetError::Wire(format!(
            "raknet peer {operation} failed: {}",
            failures.join("; ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::MAGIC;
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    fn peer_snapshot(remote_addr: SocketAddr) -> PeerSnapshot {
        PeerSnapshot {
            remote_addr,
            mtu: 1_200,
            accepted: true,
            next_datagram: Default::default(),
            next_reliable: Default::default(),
            next_ordered: Default::default(),
            next_split_id: 0,
            last_datagram: None,
            seen_datagram_order: Vec::new(),
            seen_reliable_order: Vec::new(),
            ordered_expected: Vec::new(),
            sequenced_latest: Vec::new(),
            queued_payloads: Vec::new(),
            unacked_datagrams: Vec::new(),
            reassembly: Vec::new(),
            ordered_holdback: Vec::new(),
            timeout_remaining: Duration::from_secs(10),
        }
    }

    fn config() -> ServerConfig {
        ServerConfig {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            motd: "paused router".to_string(),
            server_name: "RevyCraft".to_string(),
            game_version: "test".to_string(),
            protocol_number: 1,
            raknet_version: 11,
            max_players: 8,
            online_players: 0,
            server_guid: 7,
            budgets: RakNetBudgets::default(),
        }
    }

    fn offline_ping() -> Vec<u8> {
        let mut bytes = Vec::with_capacity(33);
        bytes.push(0x01);
        bytes.extend_from_slice(&13_i64.to_be_bytes());
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&17_i64.to_be_bytes());
        bytes
    }

    #[test]
    fn destination_scoped_udp_errors_do_not_terminate_the_router() {
        for kind in [
            std::io::ErrorKind::Interrupted,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::ConnectionRefused,
        ] {
            assert!(recoverable_udp_receive_error(&std::io::Error::new(
                kind,
                "destination-scoped udp error"
            )));
        }
        assert!(!recoverable_udp_receive_error(&std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "listener authority is invalid"
        )));
    }

    #[tokio::test]
    async fn prebound_router_does_not_receive_until_authority_is_granted() {
        let server = RakNetServer::<Bound>::bind(config()).await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let paused = server.start_paused();
        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        client.send_to(&offline_ping(), server_addr).await.unwrap();

        let mut response = [0_u8; 2_048];
        assert!(
            tokio::time::timeout(Duration::from_millis(50), client.recv(&mut response))
                .await
                .is_err()
        );

        let _serving = paused.resume_receive().await.unwrap();
        let length = tokio::time::timeout(Duration::from_secs(1), client.recv(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(length > 1);
        assert_eq!(response[0], 0x1c);
    }

    #[tokio::test]
    async fn paused_router_imports_peer_checkpoint_before_receive_authority() {
        let server = RakNetServer::<Bound>::bind(config()).await.unwrap();
        let mut paused = server.start_paused();
        let remote = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let remote_addr = remote.local_addr().unwrap();
        let expected = peer_snapshot(remote_addr);
        let mut imported = paused
            .import_peers(vec![
                expected
                    .clone()
                    .into_validated(RakNetBudgets::default())
                    .unwrap(),
            ])
            .await
            .unwrap();
        assert_eq!(imported.len(), 1);
        tokio::time::sleep(Duration::from_millis(30)).await;
        let imported = imported.pop().unwrap().seal_for_transfer().await.unwrap();
        assert_eq!(imported.snapshot(), &expected);

        let mut peer = imported.resume().unwrap();
        peer.send_raw(&[0xfe, 1, 2, 3]).await.unwrap();
        let mut datagram = [0_u8; 2_048];
        let length = tokio::time::timeout(Duration::from_secs(1), remote.recv(&mut datagram))
            .await
            .unwrap()
            .unwrap();
        assert!(length > 4);

        let _serving = paused.resume_receive().await.unwrap();
    }

    #[tokio::test]
    async fn receive_paused_router_seals_imported_peer_state() {
        let server = RakNetServer::<Bound>::bind(config()).await.unwrap();
        let mut paused = server.start_paused();
        let remote = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let mut expected = peer_snapshot(remote.local_addr().unwrap());
        expected.accepted = false;
        let imported = paused
            .import_peers(vec![
                expected
                    .clone()
                    .into_validated(RakNetBudgets::default())
                    .unwrap(),
            ])
            .await
            .unwrap();
        assert!(imported.is_empty());

        let frozen = paused.freeze().await.unwrap();
        assert_eq!(frozen.snapshot().peers.len(), 1);
        let sealed = &frozen.snapshot().peers[0];
        assert_eq!(sealed.remote_addr, expected.remote_addr);
        assert_eq!(sealed.mtu, expected.mtu);
        assert_eq!(sealed.accepted, expected.accepted);
        assert!(sealed.timeout_remaining <= expected.timeout_remaining);

        let _serving = frozen.resume().await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_peer_import_is_atomic_and_retryable() {
        let server = RakNetServer::<Bound>::bind(config()).await.unwrap();
        let mut paused = server.start_paused();
        let remote = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let snapshot = peer_snapshot(remote.local_addr().unwrap());
        assert!(
            paused
                .import_peers(vec![
                    snapshot
                        .clone()
                        .into_validated(RakNetBudgets::default())
                        .unwrap(),
                    snapshot
                        .clone()
                        .into_validated(RakNetBudgets::default())
                        .unwrap(),
                ])
                .await
                .is_err()
        );
        assert_eq!(
            paused
                .import_peers(vec![
                    snapshot.into_validated(RakNetBudgets::default()).unwrap(),
                ])
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn import_admission_requires_the_listener_resource_policy() {
        let server = RakNetServer::<Bound>::bind(config()).await.unwrap();
        let mut paused = server.start_paused();
        let remote = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let snapshot = peer_snapshot(remote.local_addr().unwrap());
        let budgets = RakNetBudgets::default();
        let other_policy = RakNetBudgets {
            max_payload_bytes: budgets.max_payload_bytes + 1,
            ..budgets
        };
        assert!(matches!(
            paused
                .import_peers(vec![snapshot.clone().into_validated(other_policy).unwrap()])
                .await,
            Err(RakNetError::Wire(_))
        ));
        let imported = paused
            .import_peers(vec![snapshot.into_validated(budgets).unwrap()])
            .await
            .unwrap();
        assert_eq!(imported.len(), 1);
    }
}
