use crate::RuntimeError;
use crate::runtime::{
    AcceptedGenerationSession, GenerationAdmission, QueuedAcceptGuard, RuntimeServer,
    RuntimeUpgradeLoginChallenge, RuntimeUpgradePhase, RuntimeUpgradeQueuedMessage,
    RuntimeUpgradeRole, RuntimeUpgradeSessionHandle, RuntimeUpgradeSessionState,
    RuntimeUpgradeStateView, SESSION_OUTBOUND_QUEUE_CAPACITY, SessionControl, SessionMessage,
    SessionState, SharedSessionState,
};
use crate::transport::{AcceptedTransportSession, TransportSessionIo, default_wire_codec};
use bytes::BytesMut;
use mc_proto_common::{ConnectionPhase, TransportKind, WireCodec};
use revy_voxel_core::ConnectionId;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

impl RuntimeServer {
    pub(in crate::runtime) async fn spawn_accepted_transport_session(
        self: &Arc<Self>,
        accepted: AcceptedGenerationSession,
    ) {
        self.spawn_transport_session_inner(
            accepted.generation_id,
            accepted.session,
            Some(accepted.queued_accept),
        )
        .await;
    }

    async fn spawn_transport_session_inner(
        self: &Arc<Self>,
        generation_id: crate::runtime::GenerationId,
        transport_session: AcceptedTransportSession,
        queued_accept_guard: Option<QueuedAcceptGuard>,
    ) {
        let queued_accept_guard = queued_accept_guard;
        if self.reload.is_shutting_down() {
            return;
        }
        let _consistency_guard = self.reload.read_consistency().await;
        let generation = match self.generation_admission(generation_id) {
            GenerationAdmission::Active(generation) | GenerationAdmission::Draining(generation) => {
                generation
            }
            GenerationAdmission::ExpiredDraining => {
                eprintln!(
                    "dropping transport session because generation {:?} finished draining before the session was admitted",
                    generation_id
                );
                return;
            }
            GenerationAdmission::Missing => {
                eprintln!(
                    "dropping transport session because generation {:?} has already been retired",
                    generation_id
                );
                return;
            }
        };
        let session = match transport_session.transport {
            TransportKind::Tcp => SessionState {
                generation: Arc::clone(&generation),
                transport: TransportKind::Tcp,
                phase: ConnectionPhase::Handshaking,
                adapter: None,
                gameplay: None,
                login_challenge: None,
                player_id: None,
                entity_id: None,
                session_capabilities: None,
            },
            TransportKind::Udp => {
                let Some(adapter) = generation.default_bedrock_adapter.clone() else {
                    eprintln!(
                        "dropping bedrock session because no default bedrock adapter is active"
                    );
                    return;
                };
                let gameplay = match self
                    .resolve_gameplay_for_adapter(&adapter.descriptor().adapter_id)
                    .await
                {
                    Ok(gameplay) => gameplay,
                    Err(error) => {
                        eprintln!(
                            "dropping bedrock session because gameplay profile could not resolve: {error}"
                        );
                        return;
                    }
                };
                let mut session = SessionState {
                    generation: Arc::clone(&generation),
                    transport: TransportKind::Udp,
                    phase: ConnectionPhase::Login,
                    adapter: Some(adapter),
                    gameplay: Some(gameplay),
                    login_challenge: None,
                    player_id: None,
                    entity_id: None,
                    session_capabilities: None,
                };
                Self::refresh_session_capabilities(&mut session);
                session
            }
        };
        self.spawn_session_with_state_guarded(transport_session, session)
            .await;
        drop(queued_accept_guard);
    }

    async fn spawn_session_with_state_guarded(
        self: &Arc<Self>,
        transport_session: AcceptedTransportSession,
        session: SessionState,
    ) {
        let connection_id = self.sessions.next_connection_id().await;
        self.spawn_session_with_fixed_connection_id(
            connection_id,
            transport_session,
            session,
            BytesMut::with_capacity(8192),
            Vec::new(),
        )
        .await;
    }

    pub(in crate::runtime) async fn spawn_session_with_fixed_connection_id(
        self: &Arc<Self>,
        connection_id: ConnectionId,
        transport_session: AcceptedTransportSession,
        session: SessionState,
        initial_read_buffer: BytesMut,
        queued_messages: Vec<SessionMessage>,
    ) {
        let (tx, rx) = mpsc::channel(SESSION_OUTBOUND_QUEUE_CAPACITY);
        let (control_tx, control_rx) = mpsc::channel(8);
        for message in queued_messages {
            let _ = tx.try_send(message);
        }
        let shared_state: SharedSessionState = Arc::new(tokio::sync::RwLock::new(session));
        self.sessions.observe_connection_id(connection_id);
        self.sessions
            .insert(connection_id, tx, control_tx, Arc::clone(&shared_state))
            .await;

        let server = Arc::clone(self);
        self.sessions
            .spawn_task(async move {
                (
                    connection_id,
                    server
                        .run_session(
                            connection_id,
                            transport_session.io,
                            shared_state,
                            initial_read_buffer,
                            rx,
                            control_rx,
                        )
                        .await,
                )
            })
            .await;
    }

    async fn process_read_buffer(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        shared_state: &SharedSessionState,
        read_buffer: &mut BytesMut,
    ) -> Result<bool, RuntimeError> {
        loop {
            let (adapter, transport) = {
                let session = shared_state.read().await;
                (session.adapter.clone(), session.transport)
            };
            let codec: &dyn WireCodec = match adapter.as_ref() {
                Some(current) => current.wire_codec(),
                None => default_wire_codec(transport)?,
            };
            let Some(frame) = codec.try_decode_frame(read_buffer)? else {
                break;
            };
            let should_close = self
                .handle_incoming_frame(connection_id, transport_io, shared_state, frame)
                .await?;
            if should_close {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn run_session(
        self: Arc<Self>,
        connection_id: ConnectionId,
        transport_io: TransportSessionIo,
        shared_state: SharedSessionState,
        mut read_buffer: BytesMut,
        mut rx: mpsc::Receiver<SessionMessage>,
        mut control_rx: mpsc::Receiver<SessionControl>,
    ) -> Result<(), RuntimeError> {
        let mut exported_for_upgrade = false;
        let mut frozen_for_upgrade = matches!(
            self.reload.current_upgrade_state(),
            Some(RuntimeUpgradeStateView {
                role: RuntimeUpgradeRole::Child,
                phase: RuntimeUpgradePhase::ChildWaitingCommit,
            })
        );
        let mut transport_io = Some(transport_io);

        let result = async {
            loop {
                if frozen_for_upgrade {
                    let Some(control) = control_rx.recv().await else {
                        break;
                    };
                    let should_exit = self
                        .handle_session_control(
                            connection_id,
                            &mut transport_io,
                            &shared_state,
                            &read_buffer,
                            &mut rx,
                            control,
                            &mut frozen_for_upgrade,
                            &mut exported_for_upgrade,
                        )
                        .await?;
                    if should_exit {
                        if exported_for_upgrade {
                            break;
                        }
                        return Ok(());
                    }
                    if exported_for_upgrade {
                        break;
                    }
                    continue;
                }
                match control_rx.try_recv() {
                    Ok(control) => {
                        let should_exit = self
                            .handle_session_control(
                                connection_id,
                                &mut transport_io,
                                &shared_state,
                                &read_buffer,
                                &mut rx,
                                control,
                                &mut frozen_for_upgrade,
                                &mut exported_for_upgrade,
                            )
                            .await?;
                        if should_exit {
                            if exported_for_upgrade {
                                break;
                            }
                            return Ok(());
                        }
                        if exported_for_upgrade {
                            break;
                        }
                        continue;
                    }
                    Err(TryRecvError::Disconnected) => break,
                    Err(TryRecvError::Empty) => {}
                }
                if !read_buffer.is_empty() {
                    tokio::select! {
                        Some(control) = control_rx.recv() => {
                            let should_exit = self.handle_session_control(
                                connection_id,
                                &mut transport_io,
                                &shared_state,
                                &read_buffer,
                                &mut rx,
                                control,
                                &mut frozen_for_upgrade,
                                &mut exported_for_upgrade,
                            ).await?;
                            if should_exit {
                                if exported_for_upgrade {
                                    break;
                                }
                                return Ok(());
                            }
                            if exported_for_upgrade {
                                break;
                            }
                        }
                        process_buffer = async {
                            let _consistency_guard = self.reload.read_consistency().await;
                            self.process_read_buffer(
                                connection_id,
                                transport_io
                                    .as_mut()
                                    .expect("transport should exist while session is active"),
                                &shared_state,
                                &mut read_buffer,
                            )
                            .await
                        } => {
                            let should_close = process_buffer?;
                            if should_close {
                                return Ok(());
                            }
                        }
                    }
                    continue;
                }
                tokio::select! {
                    Some(control) = control_rx.recv() => {
                        let should_exit = self.handle_session_control(
                            connection_id,
                            &mut transport_io,
                            &shared_state,
                            &read_buffer,
                            &mut rx,
                            control,
                            &mut frozen_for_upgrade,
                            &mut exported_for_upgrade,
                        ).await?;
                        if should_exit {
                            if exported_for_upgrade {
                                break;
                            }
                            return Ok(());
                        }
                        if exported_for_upgrade {
                            break;
                        }
                    }
                    read = async {
                        transport_io
                            .as_mut()
                            .expect("transport should exist while session is active")
                            .read_into(&mut read_buffer)
                            .await
                    } => {
                        let bytes_read = read?;
                        if bytes_read == 0 {
                            break;
                        }
                    }
                    maybe_message = rx.recv() => {
                        let Some(message) = maybe_message else {
                            break;
                        };
                        let _consistency_guard = self.reload.read_consistency().await;
                        let should_close = self
                            .handle_outgoing_message(
                                connection_id,
                                transport_io
                                    .as_mut()
                                    .expect("transport should exist while session is active"),
                                &shared_state,
                                message,
                            )
                            .await?;
                        if should_close {
                            return Ok(());
                        }
                    }
                }
            }
            Ok(())
        }
        .await;

        let cleanup = if exported_for_upgrade {
            self.sessions.remove(connection_id).await;
            Ok(())
        } else {
            self.unregister_session(connection_id, &shared_state).await
        };
        match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(error)) | (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup_error)) => Err(RuntimeError::Config(format!(
                "session {connection_id:?} ended with error: {error}; cleanup failed: {cleanup_error}"
            ))),
        }
    }

    pub(in crate::runtime) async fn reap_completed_session_tasks(&self) {
        self.sessions.reap_completed_tasks().await;
    }

    pub(in crate::runtime) async fn join_all_session_tasks(&self) {
        self.sessions.join_all_tasks().await;
    }

    async fn handle_session_control(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut Option<TransportSessionIo>,
        shared_state: &SharedSessionState,
        read_buffer: &BytesMut,
        rx: &mut mpsc::Receiver<SessionMessage>,
        control: SessionControl,
        frozen_for_upgrade: &mut bool,
        exported_for_upgrade: &mut bool,
    ) -> Result<bool, RuntimeError> {
        match control {
            SessionControl::Terminate { reason } => {
                let should_close = self
                    .handle_outgoing_message(
                        connection_id,
                        transport_io
                            .as_mut()
                            .expect("transport should exist while session is active"),
                        shared_state,
                        SessionMessage::Terminate { reason },
                    )
                    .await?;
                if should_close {
                    return Ok(true);
                }
            }
            SessionControl::FreezeForUpgrade { ack_tx } => {
                *frozen_for_upgrade = true;
                let _ = ack_tx.send(Ok(()));
            }
            SessionControl::ResumeAfterUpgradeRollback { ack_tx } => {
                *frozen_for_upgrade = false;
                let _ = ack_tx.send(Ok(()));
            }
            SessionControl::Reattach {
                instruction,
                ack_tx,
            } => {
                if *frozen_for_upgrade {
                    let _ = ack_tx.send(Err(RuntimeError::Config(
                        "session reattach is unavailable while runtime upgrade freeze is active"
                            .to_string(),
                    )));
                    return Ok(false);
                }
                let result = self
                    .handle_session_reattach(
                        connection_id,
                        transport_io
                            .as_mut()
                            .expect("transport should exist while session is active"),
                        shared_state,
                        instruction,
                    )
                    .await;
                let _ = ack_tx.send(result);
            }
            SessionControl::Export { ack_tx } => {
                if !*frozen_for_upgrade {
                    let _ = ack_tx.send(Err(RuntimeError::Config(
                        "session export requested before upgrade freeze completed".to_string(),
                    )));
                    return Ok(false);
                }
                let prepared = self
                    .prepare_session_export_for_upgrade(
                        connection_id,
                        shared_state,
                        read_buffer,
                        rx,
                    )
                    .await;
                let result = prepared.map(|mut state| {
                    let exported_transport = transport_io
                        .take()
                        .expect("transport should exist while session is active")
                        .export_tcp_for_upgrade();
                    state.encryption = exported_transport.encryption;
                    RuntimeUpgradeSessionHandle {
                        state,
                        stream: exported_transport.stream,
                    }
                });
                if let Ok(handle) = result {
                    *exported_for_upgrade = true;
                    let _ = ack_tx.send(Ok(handle));
                    return Ok(true);
                } else {
                    let _ = ack_tx.send(result);
                }
            }
        }
        Ok(false)
    }

    async fn prepare_session_export_for_upgrade(
        &self,
        connection_id: ConnectionId,
        shared_state: &SharedSessionState,
        read_buffer: &BytesMut,
        rx: &mut mpsc::Receiver<SessionMessage>,
    ) -> Result<RuntimeUpgradeSessionState, RuntimeError> {
        let (view, context, adapter, login_challenge) = {
            let session = shared_state.read().await;
            (
                Self::session_view(&session),
                Self::session_runtime_context(&session),
                session.adapter.clone(),
                session
                    .login_challenge
                    .as_ref()
                    .map(|challenge| RuntimeUpgradeLoginChallenge {
                        username: challenge.username.clone(),
                        verify_token: challenge.verify_token,
                    }),
            )
        };
        let protocol_session_blob = match adapter.as_ref() {
            Some(adapter) => Some(
                adapter
                    .export_session_state(&Self::protocol_session_snapshot(connection_id, &view))
                    .map_err(|error| RuntimeError::Config(error.to_string()))?,
            ),
            None => None,
        };
        let gameplay_session_blob = match (
            context.gameplay.as_ref(),
            Self::gameplay_session_snapshot(&view, &context),
        ) {
            (Some(gameplay), Some(snapshot)) => Some(
                gameplay
                    .export_session_state(&snapshot)
                    .map_err(|error| RuntimeError::Config(error.to_string()))?,
            ),
            _ => None,
        };
        let mut queued_messages = Vec::new();
        while let Ok(message) = rx.try_recv() {
            queued_messages.push(match message {
                SessionMessage::Event(event) => {
                    RuntimeUpgradeQueuedMessage::Event((*event).clone())
                }
                SessionMessage::Terminate { reason } => {
                    RuntimeUpgradeQueuedMessage::Terminate { reason }
                }
            });
        }
        Ok(RuntimeUpgradeSessionState {
            connection_id,
            generation_id: view.generation_id,
            transport: view.transport,
            phase: view.phase,
            adapter_id: view.adapter_id,
            player_id: view.player_id,
            entity_id: view.entity_id,
            gameplay_profile: view.gameplay_profile,
            protocol_generation: view.protocol_generation,
            gameplay_generation: view.gameplay_generation,
            login_challenge,
            read_buffer: read_buffer.to_vec(),
            queued_messages,
            encryption: None,
            protocol_session_blob,
            gameplay_session_blob,
        })
    }
}
