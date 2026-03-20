use super::{RunningServer, RuntimeServer, TopologyGenerationId, now_ms};
use mc_core::{ConnectionId, EntityId, PlayerId, PluginGenerationId};
use mc_plugin_host::PluginHostStatusSnapshot;
use mc_plugin_host::registry::ListenerBinding;
use mc_proto_common::{ConnectionPhase, TransportKind};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TopologyStatusState {
    Active,
    Draining,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyStatusSnapshot {
    pub generation_id: TopologyGenerationId,
    pub state: TopologyStatusState,
    pub drain_deadline_ms: Option<u64>,
    pub listener_bindings: Vec<ListenerBinding>,
    pub default_adapter_id: String,
    pub default_bedrock_adapter_id: Option<String>,
    pub enabled_adapter_ids: Vec<String>,
    pub enabled_bedrock_adapter_ids: Vec<String>,
    pub motd: String,
    pub max_players: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportCountSnapshot {
    pub transport: TransportKind,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseCountSnapshot {
    pub phase: ConnectionPhase,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyGenerationCountSnapshot {
    pub generation_id: TopologyGenerationId,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionalNamedCountSnapshot {
    pub value: Option<String>,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummarySnapshot {
    pub total: usize,
    pub by_transport: Vec<TransportCountSnapshot>,
    pub by_phase: Vec<PhaseCountSnapshot>,
    pub by_topology_generation: Vec<TopologyGenerationCountSnapshot>,
    pub by_adapter_id: Vec<OptionalNamedCountSnapshot>,
    pub by_gameplay_profile: Vec<OptionalNamedCountSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusSnapshot {
    pub connection_id: ConnectionId,
    pub topology_generation_id: TopologyGenerationId,
    pub transport: TransportKind,
    pub phase: ConnectionPhase,
    pub adapter_id: Option<String>,
    pub gameplay_profile: Option<String>,
    pub player_id: Option<PlayerId>,
    pub entity_id: Option<EntityId>,
    pub protocol_generation: Option<PluginGenerationId>,
    pub gameplay_generation: Option<PluginGenerationId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatusSnapshot {
    pub active_topology: TopologyStatusSnapshot,
    pub draining_topologies: Vec<TopologyStatusSnapshot>,
    pub listener_bindings: Vec<ListenerBinding>,
    pub session_summary: SessionSummarySnapshot,
    pub dirty: bool,
    pub plugin_host: Option<PluginHostStatusSnapshot>,
}

impl RunningServer {
    #[must_use]
    pub async fn status(&self) -> RuntimeStatusSnapshot {
        self.runtime.status_snapshot().await
    }

    #[must_use]
    pub async fn session_status(&self) -> Vec<SessionStatusSnapshot> {
        self.runtime.session_status_snapshot().await
    }
}

impl RuntimeServer {
    pub(crate) async fn status_snapshot(&self) -> RuntimeStatusSnapshot {
        let (active_topology, draining_topologies, listener_bindings) = {
            let topology = self
                .topology
                .read()
                .expect("runtime topology lock should not be poisoned");
            let active_topology =
                topology_status_snapshot(&topology.active, TopologyStatusState::Active, None);
            let mut draining_topologies = topology
                .draining
                .iter()
                .map(|entry| {
                    topology_status_snapshot(
                        &entry.generation,
                        TopologyStatusState::Draining,
                        Some(entry.drain_deadline_ms),
                    )
                })
                .collect::<Vec<_>>();
            draining_topologies.sort_by_key(|entry| entry.generation_id.0);
            (
                active_topology,
                draining_topologies,
                topology.active.listener_bindings.clone(),
            )
        };
        let session_status = self.session_status_snapshot().await;
        let session_summary = summarize_sessions(&session_status);
        let dirty = self.state.lock().await.dirty;

        RuntimeStatusSnapshot {
            active_topology,
            draining_topologies,
            listener_bindings,
            session_summary,
            dirty,
            plugin_host: self
                .plugin_host
                .as_ref()
                .map(|plugin_host| plugin_host.status()),
        }
    }

    pub(crate) async fn session_status_snapshot(&self) -> Vec<SessionStatusSnapshot> {
        let mut sessions = self
            .sessions
            .lock()
            .await
            .iter()
            .map(|(connection_id, handle)| SessionStatusSnapshot {
                connection_id: *connection_id,
                topology_generation_id: handle.topology_generation_id,
                transport: handle.transport,
                phase: handle.phase,
                adapter_id: handle.adapter_id.clone(),
                gameplay_profile: handle
                    .gameplay_profile
                    .as_ref()
                    .map(|profile| profile.as_str().to_string()),
                player_id: handle.player_id,
                entity_id: handle.entity_id,
                protocol_generation: handle
                    .session_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.protocol_generation),
                gameplay_generation: handle
                    .session_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.gameplay_generation),
            })
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| session.connection_id);
        sessions
    }

    pub(crate) async fn log_status_summary(&self, reason: &str) {
        let status = self.status_snapshot().await;
        eprintln!("{reason}");
        for line in format_runtime_status_summary(&status).lines() {
            eprintln!("{line}");
        }
    }
}

#[must_use]
pub fn format_runtime_status_summary(snapshot: &RuntimeStatusSnapshot) -> String {
    let mut lines = vec![
        format!(
            "runtime active-topology={} draining-topologies={} listeners={} sessions={} dirty={}",
            snapshot.active_topology.generation_id.0,
            snapshot.draining_topologies.len(),
            snapshot.listener_bindings.len(),
            snapshot.session_summary.total,
            snapshot.dirty,
        ),
        format!(
            "topology tcp-default={} tcp-enabled={} udp-default={} udp-enabled={} max-players={} motd={:?}",
            snapshot.active_topology.default_adapter_id,
            join_or_dash(&snapshot.active_topology.enabled_adapter_ids),
            snapshot
                .active_topology
                .default_bedrock_adapter_id
                .as_deref()
                .unwrap_or("-"),
            join_or_dash(&snapshot.active_topology.enabled_bedrock_adapter_ids),
            snapshot.active_topology.max_players,
            snapshot.active_topology.motd,
        ),
        format!(
            "session-summary transport={} phase={}",
            format_transport_counts(&snapshot.session_summary.by_transport),
            format_phase_counts(&snapshot.session_summary.by_phase),
        ),
    ];

    if !snapshot.draining_topologies.is_empty() {
        lines.push(format!(
            "draining {}",
            snapshot
                .draining_topologies
                .iter()
                .map(|topology| {
                    format!(
                        "{}@{}",
                        topology.generation_id.0,
                        topology.drain_deadline_ms.unwrap_or_else(now_ms)
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        ));
    }

    if let Some(plugin_host) = &snapshot.plugin_host {
        lines.push(format!(
            "plugins protocol={} gameplay={} storage={} auth={} active-quarantines={} artifact-quarantines={} pending-fatal={}",
            plugin_host.protocols.len(),
            plugin_host.gameplay.len(),
            plugin_host.storage.len(),
            plugin_host.auth.len(),
            plugin_host.active_quarantine_count(),
            plugin_host.artifact_quarantine_count(),
            plugin_host.pending_fatal_error.as_deref().unwrap_or("none"),
        ));
    }

    lines.join("\n")
}

fn topology_status_snapshot(
    generation: &std::sync::Arc<super::RuntimeTopologyGeneration>,
    state: TopologyStatusState,
    drain_deadline_ms: Option<u64>,
) -> TopologyStatusSnapshot {
    TopologyStatusSnapshot {
        generation_id: generation.generation_id,
        state,
        drain_deadline_ms,
        listener_bindings: generation.listener_bindings.clone(),
        default_adapter_id: generation.default_adapter.descriptor().adapter_id,
        default_bedrock_adapter_id: generation
            .default_bedrock_adapter
            .as_ref()
            .map(|adapter| adapter.descriptor().adapter_id),
        enabled_adapter_ids: generation
            .protocol_registry
            .adapter_ids_for_transport(TransportKind::Tcp),
        enabled_bedrock_adapter_ids: generation
            .protocol_registry
            .adapter_ids_for_transport(TransportKind::Udp),
        motd: generation.config.motd.clone(),
        max_players: generation.config.max_players,
    }
}

fn summarize_sessions(sessions: &[SessionStatusSnapshot]) -> SessionSummarySnapshot {
    let mut transport_counts = HashMap::new();
    let mut phase_counts = [0_usize; 4];
    let mut topology_counts = HashMap::new();
    let mut adapter_counts = HashMap::new();
    let mut gameplay_counts = HashMap::new();

    for session in sessions {
        *transport_counts.entry(session.transport).or_insert(0) += 1;
        phase_counts[phase_index(session.phase)] += 1;
        *topology_counts
            .entry(session.topology_generation_id)
            .or_insert(0) += 1;
        *adapter_counts
            .entry(session.adapter_id.clone())
            .or_insert(0) += 1;
        *gameplay_counts
            .entry(session.gameplay_profile.clone())
            .or_insert(0) += 1;
    }

    let by_transport = [TransportKind::Tcp, TransportKind::Udp]
        .into_iter()
        .map(|transport| TransportCountSnapshot {
            transport,
            count: *transport_counts.get(&transport).unwrap_or(&0),
        })
        .collect();
    let by_phase = [
        ConnectionPhase::Handshaking,
        ConnectionPhase::Status,
        ConnectionPhase::Login,
        ConnectionPhase::Play,
    ]
    .into_iter()
    .map(|phase| PhaseCountSnapshot {
        phase,
        count: phase_counts[phase_index(phase)],
    })
    .collect();

    let mut by_topology_generation = topology_counts
        .into_iter()
        .map(|(generation_id, count)| TopologyGenerationCountSnapshot {
            generation_id,
            count,
        })
        .collect::<Vec<_>>();
    by_topology_generation.sort_by_key(|entry| entry.generation_id.0);

    let mut by_adapter_id = adapter_counts
        .into_iter()
        .map(|(value, count)| OptionalNamedCountSnapshot { value, count })
        .collect::<Vec<_>>();
    by_adapter_id.sort_by(optional_named_count_cmp);

    let mut by_gameplay_profile = gameplay_counts
        .into_iter()
        .map(|(value, count)| OptionalNamedCountSnapshot { value, count })
        .collect::<Vec<_>>();
    by_gameplay_profile.sort_by(optional_named_count_cmp);

    SessionSummarySnapshot {
        total: sessions.len(),
        by_transport,
        by_phase,
        by_topology_generation,
        by_adapter_id,
        by_gameplay_profile,
    }
}

const fn phase_index(phase: ConnectionPhase) -> usize {
    match phase {
        ConnectionPhase::Handshaking => 0,
        ConnectionPhase::Status => 1,
        ConnectionPhase::Login => 2,
        ConnectionPhase::Play => 3,
    }
}

fn optional_named_count_cmp(
    left: &OptionalNamedCountSnapshot,
    right: &OptionalNamedCountSnapshot,
) -> Ordering {
    match (&left.value, &right.value) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left), Some(right)) => left.cmp(right),
    }
}

fn join_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values.join(",")
    }
}

fn format_transport_counts(counts: &[TransportCountSnapshot]) -> String {
    counts
        .iter()
        .map(|entry| {
            format!(
                "{}:{}",
                match entry.transport {
                    TransportKind::Tcp => "tcp",
                    TransportKind::Udp => "udp",
                },
                entry.count
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn format_phase_counts(counts: &[PhaseCountSnapshot]) -> String {
    counts
        .iter()
        .map(|entry| {
            format!(
                "{}:{}",
                match entry.phase {
                    ConnectionPhase::Handshaking => "handshaking",
                    ConnectionPhase::Status => "status",
                    ConnectionPhase::Login => "login",
                    ConnectionPhase::Play => "play",
                },
                entry.count
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}
