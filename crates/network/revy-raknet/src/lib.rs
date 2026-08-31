pub mod budget;
pub mod peer;
pub mod sequence;
pub mod server;
mod wire;

pub use budget::{BudgetExceeded, RakNetBudgets};
pub use peer::{RakNetPeer, Running as RunningPeer};
pub use server::{Bound, Frozen, RakNetServer, ServerConfig, Serving};

#[derive(Debug, thiserror::Error)]
pub enum RakNetError {
    #[error("raknet wire error: {0}")]
    Wire(String),
    #[error(transparent)]
    Budget(#[from] BudgetExceeded),
    #[error("raknet peer is closed")]
    Closed,
    #[error("raknet peer command queue is full")]
    QueueFull,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
