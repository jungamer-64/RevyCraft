use super::{GenerationId, RunningServer, RuntimeServer, now_ms};
use crate::{ListenerBinding, PluginFailureAction, PluginFailureMatrix, PluginHostStatusSnapshot};
use mc_core::{ConnectionId, EntityId, PlayerId, PluginGenerationId};
use mc_proto_common::{ConnectionPhase, TransportKind};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GenerationStatusState {
    Active,
    Draining,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationStatusSnapshot {
    pub generation_id: GenerationId,
    pub state: GenerationStatusState,
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
pub struct GenerationCountSnapshot {
    pub generation_id: GenerationId,
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
    pub by_generation: Vec<GenerationCountSnapshot>,
    pub by_adapter_id: Vec<OptionalNamedCountSnapshot>,
    pub by_gameplay_profile: Vec<OptionalNamedCountSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusSnapshot {
    pub connection_id: ConnectionId,
    pub generation_id: GenerationId,
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
    pub active_generation: GenerationStatusSnapshot,
    pub draining_generations: Vec<GenerationStatusSnapshot>,
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
        let (active_generation, draining_generations, listener_bindings) = {
            let generation_state = self
                .generation_state
                .read()
                .expect("runtime generation lock should not be poisoned");
            let active_generation = generation_status_snapshot(
                &generation_state.active,
                GenerationStatusState::Active,
                None,
            );
            let mut draining_generations = generation_state
                .draining
                .iter()
                .map(|entry| {
                    generation_status_snapshot(
                        &entry.generation,
                        GenerationStatusState::Draining,
                        Some(entry.drain_deadline_ms),
                    )
                })
                .collect::<Vec<_>>();
            draining_generations.sort_by_key(|entry| entry.generation_id.0);
            (
                active_generation,
                draining_generations,
                generation_state.active.listener_bindings.clone(),
            )
        };
        let session_status = self.session_status_snapshot().await;
        let session_summary = summarize_sessions(&session_status);
        let dirty = self.state.lock().await.dirty;

        RuntimeStatusSnapshot {
            active_generation,
            draining_generations,
            listener_bindings,
            session_summary,
            dirty,
            plugin_host: self
                .reload_host
                .as_ref()
                .map(|reload_host| summarize_plugin_host_status(reload_host.status())),
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
                generation_id: handle.generation.generation_id,
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
            "runtime active-generation={} draining-generations={} listeners={} sessions={} dirty={}",
            snapshot.active_generation.generation_id.0,
            snapshot.draining_generations.len(),
            snapshot.listener_bindings.len(),
            snapshot.session_summary.total,
            snapshot.dirty,
        ),
        format!(
            "generation tcp-default={} tcp-enabled={} udp-default={} udp-enabled={} max-players={} motd={:?}",
            snapshot.active_generation.default_adapter_id,
            join_or_dash(&snapshot.active_generation.enabled_adapter_ids),
            snapshot
                .active_generation
                .default_bedrock_adapter_id
                .as_deref()
                .unwrap_or("-"),
            join_or_dash(&snapshot.active_generation.enabled_bedrock_adapter_ids),
            snapshot.active_generation.max_players,
            snapshot.active_generation.motd,
        ),
        format!(
            "session-summary transport={} phase={}",
            format_transport_counts(&snapshot.session_summary.by_transport),
            format_phase_counts(&snapshot.session_summary.by_phase),
        ),
    ];

    if !snapshot.draining_generations.is_empty() {
        lines.push(format!(
            "draining {}",
            snapshot
                .draining_generations
                .iter()
                .map(|generation| {
                    format!(
                        "{}@{}",
                        generation.generation_id.0,
                        generation.drain_deadline_ms.unwrap_or_else(now_ms)
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        ));
    }

    if let Some(plugin_host) = &snapshot.plugin_host {
        lines.push(format!(
            "plugins protocol={} gameplay={} storage={} auth={} admin-ui={} active-quarantines={} artifact-quarantines={} pending-fatal={}",
            plugin_host.protocol_count,
            plugin_host.gameplay_count,
            plugin_host.storage_count,
            plugin_host.auth_count,
            plugin_host.admin_ui_count,
            plugin_host.active_quarantine_count,
            plugin_host.artifact_quarantine_count,
            plugin_host.pending_fatal_error.as_deref().unwrap_or("none"),
        ));
    }

    lines.join("\n")
}

fn generation_status_snapshot(
    generation: &std::sync::Arc<super::ActiveGeneration>,
    state: GenerationStatusState,
    drain_deadline_ms: Option<u64>,
) -> GenerationStatusSnapshot {
    GenerationStatusSnapshot {
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
            .adapter_ids_for_transport(TransportKind::Tcp)
            .into_iter()
            .map(|adapter_id| adapter_id.to_string())
            .collect(),
        enabled_bedrock_adapter_ids: generation
            .protocol_registry
            .adapter_ids_for_transport(TransportKind::Udp)
            .into_iter()
            .map(|adapter_id| adapter_id.to_string())
            .collect(),
        motd: generation.config.network.motd.clone(),
        max_players: generation.config.network.max_players,
    }
}

fn summarize_plugin_host_status(
    snapshot: mc_plugin_host::host::PluginHostStatusSnapshot,
) -> PluginHostStatusSnapshot {
    let active_quarantine_count = snapshot.active_quarantine_count();
    let artifact_quarantine_count = snapshot.artifact_quarantine_count();
    PluginHostStatusSnapshot {
        failure_matrix: PluginFailureMatrix {
            protocol: map_failure_action(snapshot.failure_matrix.protocol),
            gameplay: map_failure_action(snapshot.failure_matrix.gameplay),
            storage: map_failure_action(snapshot.failure_matrix.storage),
            auth: map_failure_action(snapshot.failure_matrix.auth),
            admin_ui: map_failure_action(snapshot.failure_matrix.admin_ui),
        },
        pending_fatal_error: snapshot.pending_fatal_error,
        protocol_count: snapshot.protocols.len(),
        gameplay_count: snapshot.gameplay.len(),
        storage_count: snapshot.storage.len(),
        auth_count: snapshot.auth.len(),
        admin_ui_count: snapshot.admin_ui.len(),
        active_quarantine_count,
        artifact_quarantine_count,
    }
}

const fn map_failure_action(
    action: mc_plugin_host::host::PluginFailureAction,
) -> PluginFailureAction {
    match action {
        mc_plugin_host::host::PluginFailureAction::Quarantine => PluginFailureAction::Quarantine,
        mc_plugin_host::host::PluginFailureAction::Skip => PluginFailureAction::Skip,
        mc_plugin_host::host::PluginFailureAction::FailFast => PluginFailureAction::FailFast,
    }
}

pub(crate) fn summarize_sessions(sessions: &[SessionStatusSnapshot]) -> SessionSummarySnapshot {
    let mut transport_counts = HashMap::new();
    let mut phase_counts = [0_usize; 4];
    let mut topology_counts = HashMap::new();
    let mut adapter_counts = HashMap::new();
    let mut gameplay_counts = HashMap::new();

    for session in sessions {
        *transport_counts.entry(session.transport).or_insert(0) += 1;
        phase_counts[phase_index(session.phase)] += 1;
        *topology_counts.entry(session.generation_id).or_insert(0) += 1;
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

    let mut by_generation = topology_counts
        .into_iter()
        .map(|(generation_id, count)| GenerationCountSnapshot {
            generation_id,
            count,
        })
        .collect::<Vec<_>>();
    by_generation.sort_by_key(|entry| entry.generation_id.0);

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
        by_generation,
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
