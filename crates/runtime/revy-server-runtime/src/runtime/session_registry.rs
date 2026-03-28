use super::status::SessionStatusSnapshot;
use super::{
    AcceptedGenerationSession, GenerationId, QueuedAcceptTracker, RuntimeServer, SessionControl,
    SessionHandle, SessionMessage, SessionReattachRecord, SessionRecipient, SharedSessionState,
};
use crate::RuntimeError;
use mc_core::{ConnectionId, EventTarget, PlayerId, SessionCapabilitySet};
use mc_plugin_api::codec::gameplay::GameplaySessionSnapshot;
use mc_plugin_api::codec::protocol::ProtocolSessionSnapshot;
use mc_plugin_host::runtime::{GameplayProfileHandle, ProtocolReloadSession};
use mc_proto_common::ConnectionPhase;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct SessionRegistry {
    sessions: Mutex<HashMap<ConnectionId, SessionHandle>>,
    pending_login_routes: Mutex<HashMap<ConnectionId, PlayerId>>,
    session_tasks: Mutex<JoinSet<(ConnectionId, Result<(), RuntimeError>)>>,
    queued_accepts: QueuedAcceptTracker,
    accepted_tx: mpsc::Sender<AcceptedGenerationSession>,
}

impl SessionRegistry {
    pub(crate) fn new(accepted_tx: mpsc::Sender<AcceptedGenerationSession>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            pending_login_routes: Mutex::new(HashMap::new()),
            session_tasks: Mutex::new(JoinSet::new()),
            queued_accepts: QueuedAcceptTracker::default(),
            accepted_tx,
        }
    }

    pub(crate) fn accepted_sender(&self) -> mpsc::Sender<AcceptedGenerationSession> {
        self.accepted_tx.clone()
    }

    pub(crate) fn queued_accepts(&self) -> QueuedAcceptTracker {
        self.queued_accepts.clone()
    }

    pub(crate) async fn next_connection_id(&self) -> ConnectionId {
        ConnectionId(NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) fn observe_connection_id(&self, connection_id: ConnectionId) {
        let target = connection_id.0.saturating_add(1);
        let mut next = NEXT_CONNECTION_ID.load(Ordering::Relaxed);
        while next < target {
            match NEXT_CONNECTION_ID.compare_exchange_weak(
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

    pub(crate) async fn insert(
        &self,
        connection_id: ConnectionId,
        tx: mpsc::Sender<SessionMessage>,
        control_tx: mpsc::Sender<SessionControl>,
        shared_state: SharedSessionState,
    ) {
        self.sessions.lock().await.insert(
            connection_id,
            SessionHandle {
                tx,
                control_tx,
                shared_state,
            },
        );
    }

    pub(crate) async fn record_pending_login_route(
        &self,
        connection_id: ConnectionId,
        player_id: PlayerId,
    ) {
        let sessions = self.sessions.lock().await;
        if sessions.contains_key(&connection_id) {
            self.pending_login_routes
                .lock()
                .await
                .insert(connection_id, player_id);
        }
    }

    pub(crate) async fn clear_pending_login_route(&self, connection_id: ConnectionId) {
        self.pending_login_routes
            .lock()
            .await
            .remove(&connection_id);
    }

    pub(crate) async fn remove(&self, connection_id: ConnectionId) {
        self.sessions.lock().await.remove(&connection_id);
        self.clear_pending_login_route(connection_id).await;
    }

    async fn session_entries(&self) -> Vec<(ConnectionId, SessionHandle)> {
        self.sessions
            .lock()
            .await
            .iter()
            .map(|(connection_id, handle)| (*connection_id, handle.clone()))
            .collect()
    }

    async fn pending_login_routes_snapshot(&self) -> HashMap<ConnectionId, PlayerId> {
        self.pending_login_routes.lock().await.clone()
    }

    pub(crate) async fn recipients_for_target(&self, target: EventTarget) -> Vec<SessionRecipient> {
        match target {
            EventTarget::Connection(connection_id) => self
                .sessions
                .lock()
                .await
                .get(&connection_id)
                .map(|handle| SessionRecipient {
                    tx: handle.tx.clone(),
                    control_tx: handle.control_tx.clone(),
                })
                .into_iter()
                .collect(),
            EventTarget::Player(target_player_id) => {
                let mut recipients = Vec::new();
                let pending_login_routes = self.pending_login_routes_snapshot().await;
                for (connection_id, handle) in self.session_entries().await {
                    let committed_player_id = handle.shared_state.read().await.player_id;
                    let routed_player_id = committed_player_id
                        .or_else(|| pending_login_routes.get(&connection_id).copied());
                    if routed_player_id == Some(target_player_id) {
                        recipients.push(SessionRecipient {
                            tx: handle.tx,
                            control_tx: handle.control_tx,
                        });
                    }
                }
                recipients
            }
            EventTarget::EveryoneExcept(excluded_player_id) => {
                let mut recipients = Vec::new();
                let pending_login_routes = self.pending_login_routes_snapshot().await;
                for (connection_id, handle) in self.session_entries().await {
                    let committed_player_id = handle.shared_state.read().await.player_id;
                    let routed_player_id = committed_player_id
                        .or_else(|| pending_login_routes.get(&connection_id).copied());
                    if routed_player_id.is_some() && routed_player_id != Some(excluded_player_id) {
                        recipients.push(SessionRecipient {
                            tx: handle.tx,
                            control_tx: handle.control_tx,
                        });
                    }
                }
                recipients
            }
        }
    }

    pub(crate) async fn gameplay_sessions_for_tick(
        &self,
    ) -> Vec<(
        PlayerId,
        SessionCapabilitySet,
        Arc<dyn GameplayProfileHandle>,
    )> {
        let mut sessions = Vec::new();
        for (_, handle) in self.session_entries().await {
            let session = handle.shared_state.read().await;
            let context = RuntimeServer::session_runtime_context(&session);
            let Some(player_id) = context.player_id else {
                continue;
            };
            let Some(session_capabilities) = context.session_capabilities else {
                continue;
            };
            let Some(gameplay) = context.gameplay else {
                continue;
            };
            sessions.push((player_id, session_capabilities, gameplay));
        }
        sessions
    }

    pub(crate) async fn protocol_reload_sessions(&self) -> Vec<ProtocolReloadSession> {
        let mut sessions = Vec::new();
        for (connection_id, handle) in self.session_entries().await {
            let view = RuntimeServer::read_session_view(&handle.shared_state).await;
            let Some(adapter_id) = view.adapter_id.clone() else {
                continue;
            };
            if !matches!(
                view.phase,
                ConnectionPhase::Status | ConnectionPhase::Login | ConnectionPhase::Play
            ) {
                continue;
            }
            sessions.push(ProtocolReloadSession {
                adapter_id,
                session: ProtocolSessionSnapshot {
                    connection_id,
                    phase: view.phase,
                    player_id: view.player_id,
                    entity_id: view.entity_id,
                },
            });
        }
        sessions
    }

    pub(crate) async fn gameplay_reload_sessions(&self) -> Vec<GameplaySessionSnapshot> {
        let mut sessions = Vec::new();
        for (_, handle) in self.session_entries().await {
            let session = handle.shared_state.read().await;
            let view = RuntimeServer::session_view(&session);
            let context = RuntimeServer::session_runtime_context(&session);
            if let Some(snapshot) = RuntimeServer::gameplay_session_snapshot(&view, &context) {
                sessions.push(snapshot);
            }
        }
        sessions
    }

    pub(crate) async fn handles_for_generations(
        &self,
        generation_ids: &[GenerationId],
    ) -> Vec<SessionHandle> {
        let mut handles = Vec::new();
        for (_, handle) in self.session_entries().await {
            let generation_id = handle.shared_state.read().await.generation.generation_id;
            if generation_ids.contains(&generation_id) {
                handles.push(handle);
            }
        }
        handles
    }

    pub(crate) async fn all_handles(&self) -> Vec<SessionHandle> {
        self.sessions.lock().await.values().cloned().collect()
    }

    pub(crate) async fn play_reattach_records(&self) -> Vec<SessionReattachRecord> {
        let mut records = Vec::new();
        for (connection_id, handle) in self.session_entries().await {
            let view = RuntimeServer::read_session_view(&handle.shared_state).await;
            if view.phase != ConnectionPhase::Play {
                continue;
            }
            records.push(SessionReattachRecord {
                connection_id,
                control_tx: handle.control_tx,
                transport: view.transport,
                phase: view.phase,
                adapter_id: view.adapter_id,
                player_id: view.player_id,
                entity_id: view.entity_id,
                gameplay_profile: view.gameplay_profile,
                protocol_generation: view.protocol_generation,
                gameplay_generation: view.gameplay_generation,
            });
        }
        records
    }

    #[cfg(test)]
    pub(crate) async fn len(&self) -> usize {
        self.sessions.lock().await.len()
    }

    #[cfg(test)]
    pub(crate) async fn is_empty(&self) -> bool {
        self.sessions.lock().await.is_empty()
    }

    #[cfg(test)]
    pub(crate) async fn pending_login_route_count_for_test(&self) -> usize {
        self.pending_login_routes.lock().await.len()
    }

    pub(crate) async fn live_generation_ids(&self) -> HashSet<GenerationId> {
        let mut live_generations = HashSet::new();
        for (_, handle) in self.session_entries().await {
            live_generations.insert(handle.shared_state.read().await.generation.generation_id);
        }
        live_generations.extend(self.queued_accepts.generation_ids());
        live_generations
    }

    pub(crate) async fn session_status_snapshot(&self) -> Vec<SessionStatusSnapshot> {
        let mut sessions = Vec::new();
        for (connection_id, handle) in self.session_entries().await {
            let view = RuntimeServer::read_session_view(&handle.shared_state).await;
            sessions.push(SessionStatusSnapshot {
                connection_id,
                generation_id: view.generation_id,
                transport: view.transport,
                phase: view.phase,
                adapter_id: view.adapter_id,
                gameplay_profile: view
                    .gameplay_profile
                    .as_ref()
                    .map(|profile| profile.as_str().to_string()),
                player_id: view.player_id,
                entity_id: view.entity_id,
                protocol_generation: view.protocol_generation,
                gameplay_generation: view.gameplay_generation,
            });
        }
        sessions.sort_by_key(|session| session.connection_id);
        sessions
    }

    pub(crate) async fn spawn_task(
        &self,
        task: impl std::future::Future<Output = (ConnectionId, Result<(), RuntimeError>)>
        + Send
        + 'static,
    ) {
        self.session_tasks.lock().await.spawn(task);
    }

    pub(crate) async fn reap_completed_tasks(&self) {
        let mut session_tasks = self.session_tasks.lock().await;
        while let Some(result) = session_tasks.try_join_next() {
            match result {
                Ok((connection_id, Ok(()))) => {
                    let _ = connection_id;
                }
                Ok((connection_id, Err(error))) => {
                    eprintln!("session {connection_id:?} ended with error: {error}");
                }
                Err(error) => {
                    eprintln!("session task join failed: {error}");
                }
            }
        }
    }

    pub(crate) async fn join_all_tasks(&self) {
        let mut session_tasks = {
            let mut guard = self.session_tasks.lock().await;
            std::mem::replace(&mut *guard, JoinSet::new())
        };
        while let Some(result) = session_tasks.join_next().await {
            match result {
                Ok((connection_id, Ok(()))) => {
                    let _ = connection_id;
                }
                Ok((connection_id, Err(error))) => {
                    eprintln!("session {connection_id:?} ended with error: {error}");
                }
                Err(error) => {
                    eprintln!("session task join failed: {error}");
                }
            }
        }
    }
}
