use super::status::SessionStatusSnapshot;
use super::{
    AcceptedGenerationSession, GenerationId, QueuedAcceptTracker, SessionControl, SessionHandle,
    SessionMessage, SessionReattachRecord, SessionState,
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
    session_tasks: Mutex<JoinSet<(ConnectionId, Result<(), RuntimeError>)>>,
    queued_accepts: QueuedAcceptTracker,
    accepted_tx: mpsc::Sender<AcceptedGenerationSession>,
}

impl SessionRegistry {
    pub(crate) fn new(accepted_tx: mpsc::Sender<AcceptedGenerationSession>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
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

    pub(crate) async fn insert(
        &self,
        connection_id: ConnectionId,
        tx: mpsc::Sender<SessionMessage>,
        control_tx: mpsc::Sender<SessionControl>,
        session: &SessionState,
    ) {
        self.sessions.lock().await.insert(
            connection_id,
            SessionHandle {
                tx,
                control_tx,
                generation: Arc::clone(&session.generation),
                transport: session.transport,
                phase: session.phase,
                adapter: session.adapter.clone(),
                adapter_id: session
                    .adapter
                    .as_ref()
                    .map(|adapter| adapter.descriptor().adapter_id),
                player_id: session.player_id,
                entity_id: session.entity_id,
                gameplay_profile: session
                    .session_capabilities
                    .as_ref()
                    .map(|capabilities| capabilities.gameplay_profile.clone()),
                gameplay: session.gameplay.clone(),
                session_capabilities: session.session_capabilities.clone(),
            },
        );
    }

    pub(crate) async fn sync_from_session(
        &self,
        connection_id: ConnectionId,
        session: &SessionState,
    ) {
        if let Some(handle) = self.sessions.lock().await.get_mut(&connection_id) {
            handle.generation = Arc::clone(&session.generation);
            handle.transport = session.transport;
            handle.phase = session.phase;
            handle.adapter = session.adapter.clone();
            handle.adapter_id = session
                .adapter
                .as_ref()
                .map(|adapter| adapter.descriptor().adapter_id);
            handle.player_id = session.player_id;
            handle.entity_id = session.entity_id;
            handle.gameplay_profile = session
                .session_capabilities
                .as_ref()
                .map(|capabilities| capabilities.gameplay_profile.clone());
            handle.gameplay = session.gameplay.clone();
            handle
                .session_capabilities
                .clone_from(&session.session_capabilities);
        }
    }

    pub(crate) async fn set_login_player(&self, connection_id: ConnectionId, player_id: PlayerId) {
        if let Some(handle) = self.sessions.lock().await.get_mut(&connection_id) {
            handle.player_id = Some(player_id);
        }
    }

    pub(crate) async fn remove(&self, connection_id: ConnectionId) {
        self.sessions.lock().await.remove(&connection_id);
    }

    pub(crate) async fn recipients_for_target(&self, target: EventTarget) -> Vec<SessionHandle> {
        let sessions = self.sessions.lock().await;
        match target {
            EventTarget::Connection(connection_id) => {
                sessions.get(&connection_id).into_iter().cloned().collect()
            }
            EventTarget::Player(target_player_id) => sessions
                .values()
                .filter(|session| session.player_id == Some(target_player_id))
                .cloned()
                .collect(),
            EventTarget::EveryoneExcept(excluded_player_id) => sessions
                .values()
                .filter(|session| {
                    session.player_id.is_some() && session.player_id != Some(excluded_player_id)
                })
                .cloned()
                .collect(),
        }
    }

    pub(crate) async fn gameplay_sessions_for_tick(
        &self,
    ) -> Vec<(
        PlayerId,
        SessionCapabilitySet,
        Arc<dyn GameplayProfileHandle>,
    )> {
        self.sessions
            .lock()
            .await
            .values()
            .filter_map(|handle| {
                let player_id = handle.player_id?;
                let session_capabilities = handle.session_capabilities.clone()?;
                let gameplay = handle.gameplay.clone()?;
                Some((player_id, session_capabilities, gameplay))
            })
            .collect()
    }

    pub(crate) async fn protocol_reload_sessions(&self) -> Vec<ProtocolReloadSession> {
        self.sessions
            .lock()
            .await
            .iter()
            .filter_map(|(connection_id, handle)| {
                let adapter_id = handle.adapter_id.clone()?;
                if !matches!(
                    handle.phase,
                    ConnectionPhase::Status | ConnectionPhase::Login | ConnectionPhase::Play
                ) {
                    return None;
                }
                Some(ProtocolReloadSession {
                    adapter_id,
                    session: ProtocolSessionSnapshot {
                        connection_id: *connection_id,
                        phase: handle.phase,
                        player_id: handle.player_id,
                        entity_id: handle.entity_id,
                    },
                })
            })
            .collect()
    }

    pub(crate) async fn gameplay_reload_sessions(&self) -> Vec<GameplaySessionSnapshot> {
        self.sessions
            .lock()
            .await
            .values()
            .filter_map(|handle| {
                Some(GameplaySessionSnapshot {
                    phase: handle.phase,
                    player_id: Some(handle.player_id?),
                    entity_id: handle.entity_id,
                    protocol: handle
                        .session_capabilities
                        .as_ref()
                        .map(|capabilities| capabilities.protocol.clone())?,
                    gameplay_profile: handle.gameplay_profile.clone()?,
                    protocol_generation: handle
                        .session_capabilities
                        .as_ref()
                        .and_then(|capabilities| capabilities.protocol_generation),
                    gameplay_generation: handle
                        .session_capabilities
                        .as_ref()
                        .and_then(|capabilities| capabilities.gameplay_generation),
                })
            })
            .collect()
    }

    pub(crate) async fn handles_for_generations(
        &self,
        generation_ids: &[GenerationId],
    ) -> Vec<SessionHandle> {
        self.sessions
            .lock()
            .await
            .values()
            .filter(|handle| generation_ids.contains(&handle.generation.generation_id))
            .cloned()
            .collect()
    }

    pub(crate) async fn all_handles(&self) -> Vec<SessionHandle> {
        self.sessions.lock().await.values().cloned().collect()
    }

    pub(crate) async fn play_reattach_records(&self) -> Vec<SessionReattachRecord> {
        self.sessions
            .lock()
            .await
            .iter()
            .filter(|(_, handle)| handle.phase == ConnectionPhase::Play)
            .map(|(connection_id, handle)| SessionReattachRecord {
                connection_id: *connection_id,
                control_tx: handle.control_tx.clone(),
                transport: handle.transport,
                phase: handle.phase,
                adapter_id: handle.adapter_id.clone(),
                player_id: handle.player_id,
                entity_id: handle.entity_id,
                gameplay_profile: handle.gameplay_profile.clone(),
                protocol_generation: handle
                    .session_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.protocol_generation),
                gameplay_generation: handle
                    .session_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.gameplay_generation),
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) async fn len(&self) -> usize {
        self.sessions.lock().await.len()
    }

    #[cfg(test)]
    pub(crate) async fn is_empty(&self) -> bool {
        self.sessions.lock().await.is_empty()
    }

    pub(crate) async fn live_generation_ids(&self) -> HashSet<GenerationId> {
        let mut live_generations = self
            .sessions
            .lock()
            .await
            .values()
            .map(|handle| handle.generation.generation_id)
            .collect::<HashSet<_>>();
        live_generations.extend(self.queued_accepts.generation_ids());
        live_generations
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
