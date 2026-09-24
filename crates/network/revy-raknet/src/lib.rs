pub mod budget;
pub mod peer;
pub mod sequence;
pub mod server;
mod wire;

pub use budget::{BudgetExceeded, RakNetBudgets};
pub use peer::{
    Frozen as FrozenPeer, OrderedPayloadSnapshot, PeerSnapshot, RakNetPeer, RakNetSender,
    ReassemblySnapshot, Running as RunningPeer, Transferred as TransferredPeer,
    UnackedDatagramSnapshot, ValidatedPeerSnapshot, validate_peer_snapshots,
};
pub use server::{Bound, Frozen, RakNetServer, ReceivePaused, ServerConfig, Serving};

#[derive(Debug, thiserror::Error)]
pub enum RakNetError {
    #[error("raknet wire error: {0}")]
    Wire(String),
    #[error(transparent)]
    Budget(#[from] BudgetExceeded),
    #[error("raknet peer is closed")]
    Closed,
    #[error("raknet peer activity timed out")]
    PeerTimeout,
    #[error(
        "raknet datagram sequence {sequence} exhausted its retransmit budget after {attempts} attempts"
    )]
    RetransmitExhausted { sequence: u32, attempts: u16 },
    #[error("raknet peer command queue is full")]
    QueueFull,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
