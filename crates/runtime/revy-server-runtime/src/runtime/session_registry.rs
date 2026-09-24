use super::status::SessionStatusSnapshot;
use super::{
    AcceptedGenerationSession, GenerationId, QueuedAcceptTracker, SessionControl,
    SessionDirectoryEntry, SessionDirectoryPhase, SessionDirectoryProjection, SessionHandle,
    SessionMessage, SessionPlayerProjection, SessionRecipient,
};
use crate::CutoverConnectionMix;
use crate::RuntimeError;
use crate::runtime::executable::FrozenProcessSessionState;
use crate::runtime::{CoreStore, RuntimeEpoch, SharedCoreEvent};
use crate::transport::PreparedProcessTransportSocket;
use mc_plugin_contract::codec::gameplay::GameplaySessionSnapshot;
use mc_plugin_host::runtime::{GameplayProfileHandle, ProtocolReloadSession};
use revy_runtime_transfer::{SharedTransferArena, SocketTransferTarget};
use revy_voxel_core::{
    ConnectionId, ConnectionIdSource, EventTarget, GameplayCapability, PlayerId,
    SessionCapabilitySet,
};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinSet;

const SESSION_PREPARE_STABILITY_LIMIT: usize = 8;
const SESSION_CUTOVER_FAN_OUT: usize = 256;

pub(crate) struct PreparedSessionDirectory {
    revision: u64,
}

pub(crate) struct PreparedProcessSessionSockets {
    pub(crate) directory_revision: u64,
    pub(crate) sessions: Vec<(ConnectionId, PreparedProcessTransportSocket)>,
}

pub(crate) struct FrozenProcessSessionStates {
    pub(crate) sessions: Vec<FrozenProcessSessionState>,
}

pub(crate) struct SessionRegistration {
    connection_id: ConnectionId,
    handle: SessionHandle,
    task: Pin<Box<dyn Future<Output = (ConnectionId, Result<(), RuntimeError>)> + Send + 'static>>,
}

impl SessionRegistration {
    pub(crate) fn new(
        connection_id: ConnectionId,
        handle: SessionHandle,
        task: impl Future<Output = (ConnectionId, Result<(), RuntimeError>)> + Send + 'static,
    ) -> Self {
        Self {
            connection_id,
            handle,
            task: Box::pin(task),
        }
    }
}

#[derive(Clone, Copy)]
enum ProcessTransferTransition {
    Rollback,
    Commit,
}

/// Owns session control fan-out through its acknowledgement boundary. A failed actor does
/// not cancel commands already admitted by other actors: every wave is drained before
/// returning the first failure, so recovery cannot race late prepare/freeze commands.
pub(crate) struct SessionRegistry {
    connection_ids: ConnectionIdSource,
    directory_revision: Arc<AtomicU64>,
    player_projection: Arc<SessionPlayerProjection>,
    sessions: Mutex<HashMap<ConnectionId, SessionHandle>>,
    session_tasks: Mutex<JoinSet<(ConnectionId, Result<(), RuntimeError>)>>,
    queued_accepts: QueuedAcceptTracker,
    accepted_tx: mpsc::Sender<AcceptedGenerationSession>,
}

impl SessionRegistry {
    pub(crate) fn new(accepted_tx: mpsc::Sender<AcceptedGenerationSession>) -> Self {
        Self {
            connection_ids: ConnectionIdSource::default(),
            directory_revision: Arc::new(AtomicU64::new(0)),
            player_projection: Arc::new(SessionPlayerProjection::new()),
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
        self.connection_ids.next_connection_id()
    }

    pub(crate) fn observe_connection_id(&self, connection_id: ConnectionId) {
        self.connection_ids.observe_connection_id(connection_id);
    }

    pub(crate) async fn insert(
        &self,
        connection_id: ConnectionId,
        tx: mpsc::Sender<SessionMessage>,
        control_tx: mpsc::Sender<SessionControl>,
        directory: Arc<SessionDirectoryProjection>,
    ) {
        directory.attach();
        self.sessions.lock().await.insert(
            connection_id,
            SessionHandle {
                tx,
                control_tx,
                directory,
            },
        );
        self.directory_revision.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) async fn register_imported(
        &self,
        registrations: Vec<SessionRegistration>,
    ) -> Result<(), RuntimeError> {
        let revision_increment = u64::try_from(registrations.len())
            .map_err(|_| RuntimeError::Config("imported session count exceeds u64".to_string()))?;
        let mut connection_ids = HashSet::with_capacity(registrations.len());
        for registration in &registrations {
            if !connection_ids.insert(registration.connection_id) {
                return Err(RuntimeError::Config(format!(
                    "child import duplicates session {:?}",
                    registration.connection_id
                )));
            }
        }

        let mut sessions = self.sessions.lock().await;
        if let Some(registration) = registrations
            .iter()
            .find(|registration| sessions.contains_key(&registration.connection_id))
        {
            return Err(RuntimeError::Config(format!(
                "child import collides with active session {:?}",
                registration.connection_id
            )));
        }
        let mut session_tasks = self.session_tasks.lock().await;
        for registration in registrations {
            registration.handle.directory.attach();
            sessions.insert(registration.connection_id, registration.handle);
            session_tasks.spawn(registration.task);
        }
        self.directory_revision
            .fetch_add(revision_increment, Ordering::AcqRel);
        Ok(())
    }

    pub(crate) async fn remove(&self, connection_id: ConnectionId) {
        let removed = self.sessions.lock().await.remove(&connection_id);
        if let Some(handle) = removed {
            handle.directory.detach();
            self.directory_revision.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub(crate) fn directory_projection(
        &self,
        connection_id: ConnectionId,
        entry: SessionDirectoryEntry,
    ) -> Arc<SessionDirectoryProjection> {
        Arc::new(SessionDirectoryProjection::new(
            connection_id,
            entry,
            Arc::clone(&self.directory_revision),
            Arc::clone(&self.player_projection),
        ))
    }

    pub(crate) fn directory_revision(&self) -> u64 {
        self.directory_revision.load(Ordering::Acquire)
    }

    async fn session_entries(&self) -> Vec<(ConnectionId, SessionHandle)> {
        self.sessions
            .lock()
            .await
            .iter()
            .map(|(connection_id, handle)| (*connection_id, handle.clone()))
            .collect()
    }

    fn read_directory(handle: &SessionHandle) -> SessionDirectoryEntry {
        handle.directory.read()
    }

    pub(crate) async fn recipients_for_target(&self, target: EventTarget) -> Vec<SessionRecipient> {
        match target {
            EventTarget::Connection(connection_id) => self
                .sessions
                .lock()
                .await
                .get(&connection_id)
                .map(|handle| SessionRecipient {
                    connection_id,
                    tx: handle.tx.clone(),
                    control_tx: handle.control_tx.clone(),
                })
                .into_iter()
                .collect(),
            EventTarget::Player(target_player_id) => {
                let Some(connection_id) = self.player_projection.connection(target_player_id)
                else {
                    return Vec::new();
                };
                self.sessions
                    .lock()
                    .await
                    .get(&connection_id)
                    .map(|handle| SessionRecipient {
                        connection_id,
                        tx: handle.tx.clone(),
                        control_tx: handle.control_tx.clone(),
                    })
                    .into_iter()
                    .collect()
            }
            EventTarget::EveryoneExcept(excluded_player_id) => self
                .session_entries()
                .await
                .into_iter()
                .filter_map(|(connection_id, handle)| {
                    let player_id = Self::read_directory(&handle).player_id()?;
                    (player_id != excluded_player_id).then_some(SessionRecipient {
                        connection_id,
                        tx: handle.tx,
                        control_tx: handle.control_tx,
                    })
                })
                .collect(),
        }
    }

    pub(crate) async fn gameplay_tick_subscribers(
        &self,
    ) -> Vec<(
        PlayerId,
        SessionCapabilitySet,
        Arc<dyn GameplayProfileHandle>,
    )> {
        self.session_entries()
            .await
            .into_iter()
            .filter_map(|(_, handle)| {
                let directory = Self::read_directory(&handle);
                match directory.phase {
                    SessionDirectoryPhase::Play {
                        player_id,
                        gameplay,
                        capabilities,
                        ..
                    } if capabilities
                        .gameplay
                        .contains(&GameplayCapability::SessionTick) =>
                    {
                        Some((player_id, capabilities, gameplay))
                    }
                    _ => None,
                }
            })
            .collect()
    }

    pub(crate) async fn protocol_reload_sessions(&self) -> Vec<ProtocolReloadSession> {
        self.session_entries()
            .await
            .into_iter()
            .filter_map(|(connection_id, handle)| {
                Self::read_directory(&handle).protocol_reload_session(connection_id)
            })
            .collect()
    }

    pub(crate) async fn gameplay_reload_sessions(&self) -> Vec<GameplaySessionSnapshot> {
        self.session_entries()
            .await
            .into_iter()
            .filter_map(|(_, handle)| {
                let directory = Self::read_directory(&handle);
                match directory.phase {
                    SessionDirectoryPhase::Play {
                        player_id,
                        entity_id,
                        capabilities,
                        ..
                    } => Some(GameplaySessionSnapshot {
                        phase: mc_proto_common::ConnectionPhase::Play,
                        player_id: Some(player_id),
                        entity_id: Some(entity_id),
                        protocol: capabilities.protocol,
                        gameplay_profile: capabilities.gameplay_profile,
                        protocol_generation: capabilities.protocol_generation,
                        gameplay_generation: capabilities.gameplay_generation,
                    }),
                    _ => None,
                }
            })
            .collect()
    }

    pub(crate) async fn handles_for_generations(
        &self,
        generation_ids: &[GenerationId],
    ) -> Vec<SessionHandle> {
        self.session_entries()
            .await
            .into_iter()
            .filter_map(|(_, handle)| {
                generation_ids
                    .contains(&Self::read_directory(&handle).generation_id)
                    .then_some(handle)
            })
            .collect()
    }

    pub(crate) async fn all_handles(&self) -> Vec<SessionHandle> {
        self.sessions.lock().await.values().cloned().collect()
    }

    pub(crate) async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    pub(crate) async fn connection_mix(&self) -> CutoverConnectionMix {
        self.session_entries().await.into_iter().fold(
            CutoverConnectionMix::default(),
            |mut mix, (_, handle)| {
                match Self::read_directory(&handle).transport {
                    mc_proto_common::TransportKind::Tcp => mix.java += 1,
                    mc_proto_common::TransportKind::Udp => mix.bedrock += 1,
                }
                mix
            },
        )
    }

    pub(crate) async fn prepare_cutover(
        &self,
        candidate: Arc<RuntimeEpoch>,
        core: Arc<CoreStore>,
        force_resync: bool,
    ) -> Result<PreparedSessionDirectory, RuntimeError> {
        for attempt in 1..=SESSION_PREPARE_STABILITY_LIMIT {
            let before = self.directory_revision();
            self.prepare_cutover_once(Arc::clone(&candidate), Arc::clone(&core), force_resync)
                .await?;
            let after = self.directory_revision();
            if before == after {
                return Ok(PreparedSessionDirectory { revision: after });
            }
            if attempt == SESSION_PREPARE_STABILITY_LIMIT {
                return Err(RuntimeError::BudgetExceeded {
                    resource: "session cutover prepare stability attempts",
                    requested: attempt.saturating_add(1),
                    limit: SESSION_PREPARE_STABILITY_LIMIT,
                });
            }
        }
        unreachable!("session prepare loop returns at the configured limit")
    }

    async fn prepare_cutover_once(
        &self,
        candidate: Arc<RuntimeEpoch>,
        core: Arc<CoreStore>,
        force_resync: bool,
    ) -> Result<(), RuntimeError> {
        let mut entries = self.session_entries().await.into_iter();
        let mut pending = JoinSet::new();
        for _ in 0..SESSION_CUTOVER_FAN_OUT {
            let Some((connection_id, handle)) = entries.next() else {
                break;
            };
            let candidate = Arc::clone(&candidate);
            let core = Arc::clone(&core);
            pending.spawn(Self::prepare_session(
                connection_id,
                handle,
                candidate,
                core,
                force_resync,
            ));
        }
        let mut failure = None;
        while let Some(result) = pending.join_next().await {
            if let Err(error) = result.map_err(RuntimeError::from).and_then(|result| result)
                && failure.is_none()
            {
                failure = Some(error);
            }
            if let Some((connection_id, handle)) = entries.next() {
                let candidate = Arc::clone(&candidate);
                let core = Arc::clone(&core);
                pending.spawn(Self::prepare_session(
                    connection_id,
                    handle,
                    candidate,
                    core,
                    force_resync,
                ));
            }
        }
        failure.map_or(Ok(()), Err)
    }

    async fn prepare_session(
        connection_id: ConnectionId,
        handle: SessionHandle,
        candidate: Arc<RuntimeEpoch>,
        core: Arc<CoreStore>,
        force_resync: bool,
    ) -> Result<(), RuntimeError> {
        if handle.control_tx.is_closed() {
            return Ok(());
        }
        let resync_events = match Self::read_directory(&handle).player_id() {
            Some(player_id) => core
                .session_resync_events(player_id)
                .await
                .into_iter()
                .map(|event| Arc::new(event.event))
                .collect(),
            None => Vec::new(),
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        if handle
            .control_tx
            .send(SessionControl::PrepareCutover {
                candidate,
                resync_events,
                force_resync,
                ack_tx,
            })
            .await
            .is_err()
        {
            return Ok(());
        }
        ack_rx.await.unwrap_or_else(|_| {
            Err(RuntimeError::Config(format!(
                "session {connection_id:?} dropped cutover prepare acknowledgement"
            )))
        })
    }

    pub(crate) async fn abort_cutover(&self, epoch_revision: u64) -> Result<(), RuntimeError> {
        let mut entries = self.session_entries().await.into_iter();
        let mut pending = JoinSet::new();
        for _ in 0..SESSION_CUTOVER_FAN_OUT {
            let Some((connection_id, handle)) = entries.next() else {
                break;
            };
            pending.spawn(async move {
                let (ack_tx, ack_rx) = oneshot::channel();
                handle
                    .control_tx
                    .send(SessionControl::AbortCutover {
                        epoch_revision,
                        ack_tx,
                    })
                    .await
                    .map_err(|_| {
                        RuntimeError::Config(format!(
                            "session {connection_id:?} closed during cutover abort"
                        ))
                    })?;
                ack_rx.await.map_err(|_| {
                    RuntimeError::Config(format!(
                        "session {connection_id:?} dropped cutover abort acknowledgement"
                    ))
                })?
            });
        }
        let mut failure = None;
        while let Some(result) = pending.join_next().await {
            if let Err(error) = result.map_err(RuntimeError::from).and_then(|result| result)
                && failure.is_none()
            {
                failure = Some(error);
            }
            if let Some((connection_id, handle)) = entries.next() {
                pending.spawn(async move {
                    let (ack_tx, ack_rx) = oneshot::channel();
                    handle
                        .control_tx
                        .send(SessionControl::AbortCutover {
                            epoch_revision,
                            ack_tx,
                        })
                        .await
                        .map_err(|_| {
                            RuntimeError::Config(format!(
                                "session {connection_id:?} closed during cutover abort"
                            ))
                        })?;
                    ack_rx.await.map_err(|_| {
                        RuntimeError::Config(format!(
                            "session {connection_id:?} dropped cutover abort acknowledgement"
                        ))
                    })?
                });
            }
        }
        failure.map_or(Ok(()), Err)
    }

    pub(crate) async fn prepare_process_transfer_sockets(
        &self,
        target: SocketTransferTarget,
        arena: Arc<SharedTransferArena>,
    ) -> Result<PreparedProcessSessionSockets, RuntimeError> {
        let prepared_revision = self.directory_revision();
        let mut entries = self.session_entries().await.into_iter();
        let mut pending = JoinSet::new();
        for _ in 0..SESSION_CUTOVER_FAN_OUT {
            let Some((connection_id, handle)) = entries.next() else {
                break;
            };
            pending.spawn(Self::prepare_process_session_socket(
                connection_id,
                handle,
                target,
                Arc::clone(&arena),
            ));
        }
        let mut sessions = Vec::new();
        let mut failure = None;
        while let Some(result) = pending.join_next().await {
            match result.map_err(RuntimeError::from).and_then(|result| result) {
                Ok(session) => sessions.push(session),
                Err(error) if failure.is_none() => failure = Some(error),
                Err(_) => {}
            }
            if let Some((connection_id, handle)) = entries.next() {
                pending.spawn(Self::prepare_process_session_socket(
                    connection_id,
                    handle,
                    target,
                    Arc::clone(&arena),
                ));
            }
        }
        if let Some(error) = failure {
            return match self.rollback_process_transfer().await {
                Ok(()) => Err(error),
                Err(rollback) => Err(RuntimeError::Config(format!(
                    "{error}; process-transfer preparation rollback failed: {rollback}"
                ))),
            };
        }
        let sealed_revision = self.directory_revision();
        if sealed_revision != prepared_revision {
            let rollback = self.rollback_process_transfer().await;
            if let Err(rollback) = rollback {
                return Err(RuntimeError::Config(format!(
                    "session directory changed from {prepared_revision} to {sealed_revision}; process-transfer preparation rollback failed: {rollback}"
                )));
            }
            return Err(RuntimeError::CutoverDirectoryChanged {
                prepared_revision,
                sealed_revision,
            });
        }
        sessions.sort_by_key(|(connection_id, _)| connection_id.0);
        Ok(PreparedProcessSessionSockets {
            directory_revision: sealed_revision,
            sessions,
        })
    }

    pub(crate) async fn freeze_for_process_transfer(
        &self,
        prepared_revision: u64,
    ) -> Result<FrozenProcessSessionStates, RuntimeError> {
        let sealed_revision = self.directory_revision();
        if sealed_revision != prepared_revision {
            return Err(RuntimeError::CutoverDirectoryChanged {
                prepared_revision,
                sealed_revision,
            });
        }
        let mut entries = self.session_entries().await.into_iter();
        let mut pending = JoinSet::new();
        for _ in 0..SESSION_CUTOVER_FAN_OUT {
            let Some((connection_id, handle)) = entries.next() else {
                break;
            };
            pending.spawn(Self::freeze_process_session(connection_id, handle));
        }
        let mut sessions = Vec::new();
        let mut failure = None;
        while let Some(result) = pending.join_next().await {
            match result.map_err(RuntimeError::from).and_then(|result| result) {
                Ok(session) => sessions.push(session),
                Err(error) if failure.is_none() => failure = Some(error),
                Err(_) => {}
            }
            if let Some((connection_id, handle)) = entries.next() {
                pending.spawn(Self::freeze_process_session(connection_id, handle));
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        let sealed_revision = self.directory_revision();
        if sealed_revision != prepared_revision {
            return Err(RuntimeError::CutoverDirectoryChanged {
                prepared_revision,
                sealed_revision,
            });
        }
        sessions.sort_by_key(|session| session.state.actor().connection_id().0);
        Ok(FrozenProcessSessionStates { sessions })
    }

    pub(crate) async fn rollback_process_transfer(&self) -> Result<(), RuntimeError> {
        self.transition_process_transfer(ProcessTransferTransition::Rollback)
            .await
    }

    pub(crate) async fn commit_process_transfer(&self) -> Result<(), RuntimeError> {
        self.transition_process_transfer(ProcessTransferTransition::Commit)
            .await
    }

    async fn transition_process_transfer(
        &self,
        transition: ProcessTransferTransition,
    ) -> Result<(), RuntimeError> {
        let mut entries = self.session_entries().await.into_iter();
        let mut pending = JoinSet::new();
        for _ in 0..SESSION_CUTOVER_FAN_OUT {
            let Some((connection_id, handle)) = entries.next() else {
                break;
            };
            pending.spawn(Self::transition_process_session(
                connection_id,
                handle,
                transition,
            ));
        }
        let mut failure = None;
        while let Some(result) = pending.join_next().await {
            if let Err(error) = result.map_err(RuntimeError::from).and_then(|result| result)
                && failure.is_none()
            {
                failure = Some(error);
            }
            if let Some((connection_id, handle)) = entries.next() {
                pending.spawn(Self::transition_process_session(
                    connection_id,
                    handle,
                    transition,
                ));
            }
        }
        failure.map_or(Ok(()), Err)
    }

    async fn transition_process_session(
        connection_id: ConnectionId,
        handle: SessionHandle,
        transition: ProcessTransferTransition,
    ) -> Result<(), RuntimeError> {
        if handle.control_tx.is_closed() {
            return Ok(());
        }
        let (ack_tx, ack_rx) = oneshot::channel();
        let control = match transition {
            ProcessTransferTransition::Rollback => {
                SessionControl::RollbackProcessTransfer { ack_tx }
            }
            ProcessTransferTransition::Commit => SessionControl::CommitProcessTransfer { ack_tx },
        };
        if handle.control_tx.send(control).await.is_err() {
            return Ok(());
        }
        ack_rx.await.unwrap_or_else(|_| {
            Err(RuntimeError::Config(format!(
                "session {connection_id:?} dropped process-transfer acknowledgement"
            )))
        })
    }

    async fn prepare_process_session_socket(
        connection_id: ConnectionId,
        handle: SessionHandle,
        target: SocketTransferTarget,
        arena: Arc<SharedTransferArena>,
    ) -> Result<(ConnectionId, PreparedProcessTransportSocket), RuntimeError> {
        if handle.control_tx.is_closed() {
            return Err(RuntimeError::Config(format!(
                "session {connection_id:?} closed during process-transfer socket preparation"
            )));
        }
        let (ack_tx, ack_rx) = oneshot::channel();
        handle
            .control_tx
            .send(SessionControl::PrepareProcessTransferSocket {
                target,
                arena,
                ack_tx,
            })
            .await
            .map_err(|_| {
                RuntimeError::Config(format!(
                    "session {connection_id:?} closed during process-transfer socket preparation"
                ))
            })?;
        let socket = ack_rx.await.map_err(|_| {
            RuntimeError::Config(format!(
                "session {connection_id:?} dropped process-transfer socket acknowledgement"
            ))
        })??;
        Ok((connection_id, socket))
    }

    async fn freeze_process_session(
        connection_id: ConnectionId,
        handle: SessionHandle,
    ) -> Result<FrozenProcessSessionState, RuntimeError> {
        if handle.control_tx.is_closed() {
            return Err(RuntimeError::Config(format!(
                "session {connection_id:?} closed during process-transfer freeze"
            )));
        }
        let (ack_tx, ack_rx) = oneshot::channel();
        handle
            .control_tx
            .send(SessionControl::FreezeForProcessTransfer { ack_tx })
            .await
            .map_err(|_| {
                RuntimeError::Config(format!(
                    "session {connection_id:?} closed during process-transfer freeze"
                ))
            })?;
        let state = ack_rx.await.map_err(|_| {
            RuntimeError::Config(format!(
                "session {connection_id:?} dropped process-transfer freeze acknowledgement"
            ))
        })??;
        if state.state.actor().connection_id() != connection_id {
            return Err(RuntimeError::Config(format!(
                "session {connection_id:?} returned a mismatched process-transfer snapshot"
            )));
        }
        Ok(state)
    }

    pub(crate) async fn seal_cutover(
        &self,
        prepared_directory: &PreparedSessionDirectory,
        epoch_revision: u64,
        events: &[SharedCoreEvent],
    ) -> Result<(), RuntimeError> {
        let sealed_revision = self.directory_revision();
        if sealed_revision != prepared_directory.revision {
            return Err(RuntimeError::CutoverDirectoryChanged {
                prepared_revision: prepared_directory.revision,
                sealed_revision,
            });
        }
        let mut entries = self.session_entries().await.into_iter();
        let mut pending = JoinSet::new();
        for _ in 0..SESSION_CUTOVER_FAN_OUT {
            let Some((connection_id, handle)) = entries.next() else {
                break;
            };
            let final_events = Self::events_for_session(connection_id, &handle, events);
            pending.spawn(Self::seal_session(
                connection_id,
                handle,
                epoch_revision,
                final_events,
            ));
        }
        let mut failure = None;
        while let Some(result) = pending.join_next().await {
            if let Err(error) = result.map_err(RuntimeError::from).and_then(|result| result)
                && failure.is_none()
            {
                failure = Some(error);
            }
            if let Some((connection_id, handle)) = entries.next() {
                let final_events = Self::events_for_session(connection_id, &handle, events);
                pending.spawn(Self::seal_session(
                    connection_id,
                    handle,
                    epoch_revision,
                    final_events,
                ));
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        let sealed_revision = self.directory_revision();
        if sealed_revision != prepared_directory.revision {
            return Err(RuntimeError::CutoverDirectoryChanged {
                prepared_revision: prepared_directory.revision,
                sealed_revision,
            });
        }
        Ok(())
    }

    fn events_for_session(
        connection_id: ConnectionId,
        handle: &SessionHandle,
        events: &[SharedCoreEvent],
    ) -> Vec<Arc<revy_voxel_core::CoreEvent>> {
        let player_id = Self::read_directory(handle).player_id();
        events
            .iter()
            .filter(|event| match event.target {
                EventTarget::Connection(target) => target == connection_id,
                EventTarget::Player(target) => player_id == Some(target),
                EventTarget::EveryoneExcept(excluded) => {
                    player_id.is_some_and(|current| current != excluded)
                }
            })
            .map(|event| Arc::clone(&event.event))
            .collect()
    }

    async fn seal_session(
        connection_id: ConnectionId,
        handle: SessionHandle,
        epoch_revision: u64,
        final_events: Vec<Arc<revy_voxel_core::CoreEvent>>,
    ) -> Result<(), RuntimeError> {
        if handle.control_tx.is_closed() {
            return Ok(());
        }
        let (ack_tx, ack_rx) = oneshot::channel();
        if handle
            .control_tx
            .send(SessionControl::SealCutover {
                epoch_revision,
                final_events,
                ack_tx,
            })
            .await
            .is_err()
        {
            return Ok(());
        }
        ack_rx.await.unwrap_or_else(|_| {
            Err(RuntimeError::Config(format!(
                "session {connection_id:?} dropped cutover seal acknowledgement"
            )))
        })
    }

    pub(crate) async fn live_generation_ids(&self) -> HashSet<GenerationId> {
        let mut live_generations = self
            .session_entries()
            .await
            .into_iter()
            .map(|(_, handle)| Self::read_directory(&handle).generation_id)
            .collect::<HashSet<_>>();
        live_generations.extend(self.queued_accepts.generation_ids());
        live_generations
    }

    pub(crate) async fn session_status_snapshot(&self) -> Vec<SessionStatusSnapshot> {
        let mut sessions = self
            .session_entries()
            .await
            .into_iter()
            .map(|(connection_id, handle)| {
                let view = Self::read_directory(&handle);
                let (player_id, entity_id) = match &view.phase {
                    SessionDirectoryPhase::Play {
                        player_id,
                        entity_id,
                        ..
                    } => (Some(*player_id), Some(*entity_id)),
                    _ => (None, None),
                };
                SessionStatusSnapshot {
                    connection_id,
                    generation_id: view.generation_id,
                    transport: view.transport,
                    phase: view.phase(),
                    adapter_id: view.adapter_id().map(str::to_string),
                    gameplay_profile: view
                        .gameplay_profile()
                        .map(|profile| profile.as_str().to_string()),
                    player_id,
                    entity_id,
                    protocol_generation: view.protocol_generation(),
                    gameplay_generation: view.gameplay_generation(),
                }
            })
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| session.connection_id);
        sessions
    }

    pub(crate) async fn executable_directory_snapshot(
        &self,
    ) -> Result<(u64, Vec<SessionStatusSnapshot>), RuntimeError> {
        for attempt in 1..=SESSION_PREPARE_STABILITY_LIMIT {
            let before = self.directory_revision();
            let sessions = self.session_status_snapshot().await;
            let after = self.directory_revision();
            if before == after {
                return Ok((after, sessions));
            }
            if attempt == SESSION_PREPARE_STABILITY_LIMIT {
                return Err(RuntimeError::BudgetExceeded {
                    resource: "executable session directory stability attempts",
                    requested: attempt.saturating_add(1),
                    limit: SESSION_PREPARE_STABILITY_LIMIT,
                });
            }
        }
        unreachable!("session directory snapshot returns at the configured limit")
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
                Ok((_, Ok(()))) => {}
                Ok((connection_id, Err(error))) => {
                    eprintln!("session {connection_id:?} ended with error: {error}");
                }
                Err(error) => eprintln!("session task join failed: {error}"),
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
                Ok((_, Ok(()))) => {}
                Ok((connection_id, Err(error))) => {
                    eprintln!("session {connection_id:?} ended with error: {error}");
                }
                Err(error) => eprintln!("session task join failed: {error}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn next_connection_id_respects_observed_ids() {
        let (accepted_tx, _accepted_rx) = mpsc::channel(1);
        let registry = SessionRegistry::new(accepted_tx);

        assert_eq!(registry.next_connection_id().await, ConnectionId(1));
        registry.observe_connection_id(ConnectionId(7));
        assert_eq!(registry.next_connection_id().await, ConnectionId(8));
    }

    #[tokio::test]
    async fn cutover_controls_drain_every_wave_after_an_actor_rejects()
    -> Result<(), Box<dyn std::error::Error>> {
        let (accepted_tx, _accepted_rx) = mpsc::channel(1);
        let registry = SessionRegistry::new(accepted_tx);
        let population = u64::try_from(SESSION_CUTOVER_FAN_OUT * 2 + 1)?;
        let completed = Arc::new(AtomicU64::new(0));
        let mut actors = JoinSet::new();
        for _ in 0..population {
            let connection_id = registry.next_connection_id().await;
            let (tx, _rx) = mpsc::channel(1);
            let (control_tx, mut control_rx) = mpsc::channel(1);
            let directory = registry.directory_projection(
                connection_id,
                SessionDirectoryEntry {
                    generation_id: GenerationId(1),
                    transport: mc_proto_common::TransportKind::Tcp,
                    phase: SessionDirectoryPhase::Handshaking,
                },
            );
            registry
                .insert(connection_id, tx, control_tx, directory)
                .await;
            let completed = Arc::clone(&completed);
            actors.spawn(async move {
                for step in 0..4 {
                    let ack_tx = match (step, control_rx.recv().await) {
                        (0, Some(SessionControl::SealCutover { ack_tx, .. }))
                        | (1, Some(SessionControl::AbortCutover { ack_tx, .. }))
                        | (2, Some(SessionControl::RollbackProcessTransfer { ack_tx }))
                        | (3, Some(SessionControl::CommitProcessTransfer { ack_tx })) => ack_tx,
                        _ => panic!("unexpected session control transition"),
                    };
                    let ordinal = completed.fetch_add(1, Ordering::Relaxed);
                    let result = if ordinal == 0 {
                        Err(RuntimeError::BudgetExceeded {
                            resource: "session handoff state",
                            requested: 2,
                            limit: 1,
                        })
                    } else {
                        // Let the first rejection reach the coordinator while these acks
                        // are still outstanding. Returning early must not abandon them.
                        tokio::task::yield_now().await;
                        Ok(())
                    };
                    assert!(ack_tx.send(result).is_ok(), "coordinator abandoned an ack");
                }
            });
        }

        let prepared = PreparedSessionDirectory {
            revision: registry.directory_revision(),
        };
        for step in 0..4 {
            completed.store(0, Ordering::Relaxed);
            let result = match step {
                0 => registry.seal_cutover(&prepared, 2, &[]).await,
                1 => registry.abort_cutover(2).await,
                2 => registry.rollback_process_transfer().await,
                _ => registry.commit_process_transfer().await,
            };
            assert!(matches!(result, Err(RuntimeError::BudgetExceeded { .. })));
            assert_eq!(completed.load(Ordering::Relaxed), population);
        }
        while let Some(actor) = actors.join_next().await {
            actor?;
        }
        Ok(())
    }
}
