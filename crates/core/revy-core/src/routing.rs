use crate::ConnectionId;
use std::collections::BTreeMap;
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRoutes<ConnectionId, PlayerId> {
    pending_login_routes: BTreeMap<ConnectionId, PlayerId>,
}

impl<ConnectionId, PlayerId> Default for SessionRoutes<ConnectionId, PlayerId> {
    fn default() -> Self {
        Self {
            pending_login_routes: BTreeMap::new(),
        }
    }
}

impl<ConnectionId, PlayerId> SessionRoutes<ConnectionId, PlayerId>
where
    ConnectionId: Ord + Copy,
    PlayerId: Copy,
{
    pub fn insert_pending_login_route(&mut self, connection_id: ConnectionId, player_id: PlayerId) {
        self.pending_login_routes.insert(connection_id, player_id);
    }

    pub fn clear_pending_login_route(&mut self, connection_id: ConnectionId) -> Option<PlayerId> {
        self.pending_login_routes.remove(&connection_id)
    }

    #[must_use]
    pub fn pending_login_route(&self, connection_id: ConnectionId) -> Option<PlayerId> {
        self.pending_login_routes.get(&connection_id).copied()
    }

    pub fn pending_login_routes(&self) -> impl Iterator<Item = (ConnectionId, PlayerId)> + '_ {
        self.pending_login_routes
            .iter()
            .map(|(connection_id, player_id)| (*connection_id, *player_id))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.pending_login_routes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending_login_routes.is_empty()
    }
}

impl<ConnectionId, PlayerId> SessionRoutes<ConnectionId, PlayerId>
where
    ConnectionId: Ord + Clone,
    PlayerId: Clone,
{
    #[must_use]
    pub fn snapshot_pending_login_routes(&self) -> BTreeMap<ConnectionId, PlayerId> {
        self.pending_login_routes.clone()
    }
}
