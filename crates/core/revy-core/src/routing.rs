use crate::ConnectionId;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub struct ConnectionIdSource {
    next_connection_id: AtomicU64,
}
impl Default for ConnectionIdSource {
    fn default() -> Self {
        Self {
            next_connection_id: AtomicU64::new(1),
        }
    }
}

impl ConnectionIdSource {
    #[must_use]
    pub fn next_connection_id(&self) -> ConnectionId {
        ConnectionId(self.next_connection_id.fetch_add(1, Ordering::Relaxed))
    }

    pub fn observe_connection_id(&self, connection_id: ConnectionId) {
        let target = connection_id.0.saturating_add(1);
        let mut next = self.next_connection_id.load(Ordering::Relaxed);
        while next < target {
            match self.next_connection_id.compare_exchange_weak(
                next,
                target,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => next = observed,
            }
        }
    }
}
