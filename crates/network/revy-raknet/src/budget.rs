#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RakNetBudgets {
    pub max_peers: usize,
    pub peer_ingress_queue: usize,
    pub peer_command_queue: usize,
    pub application_queue: usize,
    pub max_datagram_bytes: usize,
    pub max_payload_bytes: usize,
    pub max_reassembly_bytes_per_peer: usize,
    pub max_reassembly_parts_per_peer: usize,
    pub max_ordered_holdback: usize,
    pub max_unacked_datagrams: usize,
}

impl Default for RakNetBudgets {
    fn default() -> Self {
        Self {
            max_peers: 4_096,
            peer_ingress_queue: 256,
            peer_command_queue: 256,
            application_queue: 256,
            max_datagram_bytes: 2_048,
            max_payload_bytes: 4 * 1024 * 1024,
            max_reassembly_bytes_per_peer: 8 * 1024 * 1024,
            max_reassembly_parts_per_peer: 4_096,
            max_ordered_holdback: 1_024,
            max_unacked_datagrams: 2_048,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("raknet {resource} budget exhausted: requested {requested}, limit {limit}")]
pub struct BudgetExceeded {
    pub resource: &'static str,
    pub requested: usize,
    pub limit: usize,
}

pub(crate) fn ensure_budget(
    resource: &'static str,
    requested: usize,
    limit: usize,
) -> Result<(), BudgetExceeded> {
    if requested > limit {
        Err(BudgetExceeded {
            resource,
            requested,
            limit,
        })
    } else {
        Ok(())
    }
}
