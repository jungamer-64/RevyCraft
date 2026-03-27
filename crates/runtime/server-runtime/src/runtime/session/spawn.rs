use crate::RuntimeError;
use crate::runtime::{
    AcceptedGenerationSession, GenerationAdmission, QueuedAcceptGuard, RuntimeServer,
    RuntimeUpgradeLoginChallenge, RuntimeUpgradeQueuedMessage, RuntimeUpgradeSessionHandle,
    RuntimeUpgradeSessionState, SESSION_OUTBOUND_QUEUE_CAPACITY, SessionControl, SessionMessage,
    SessionState,
};
use crate::transport::{AcceptedTransportSession, TransportSessionIo, default_wire_codec};
use bytes::BytesMut;
use mc_core::ConnectionId;
use mc_proto_common::{ConnectionPhase, TransportKind, WireCodec};
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
        self.sessions.observe_connection_id(connection_id);
        self.sessions
            .insert(connection_id, tx, control_tx, &session)
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
                            session,
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
        session: &mut SessionState,
        read_buffer: &mut BytesMut,
    ) -> Result<bool, RuntimeError> {
        loop {
            let codec: &dyn WireCodec = match session.adapter.as_ref() {
                Some(current) => current.wire_codec(),
                None => default_wire_codec(session.transport)?,
            };
            let Some(frame) = codec.try_decode_frame(read_buffer)? else {
                break;
            };
            let should_close = self
                .handle_incoming_frame(connection_id, transport_io, session, frame)
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
        mut session: SessionState,
        mut read_buffer: BytesMut,
        mut rx: mpsc::Receiver<SessionMessage>,
        mut control_rx: mpsc::Receiver<SessionControl>,
    ) -> Result<(), RuntimeError> {
        let mut exported_for_upgrade = false;
        let mut transport_io = Some(transport_io);

        let result = async {
            loop {
                match control_rx.try_recv() {
                    Ok(control) => {
                        let should_exit = self
                            .handle_session_control(
                                connection_id,
                                &mut transport_io,
                                &mut session,
                                &read_buffer,
                                &mut rx,
                                control,
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
                                &mut session,
                                &read_buffer,
                                &mut rx,
                                control,
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
                                &mut session,
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
                            &mut session,
                            &read_buffer,
                            &mut rx,
                            control,
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
                                &mut session,
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
            self.unregister_session(connection_id, &session).await
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
        session: &mut SessionState,
        read_buffer: &BytesMut,
        rx: &mut mpsc::Receiver<SessionMessage>,
        control: SessionControl,
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
                        session,
                        SessionMessage::Terminate { reason },
                    )
                    .await?;
                if should_close {
                    return Ok(true);
                }
            }
            SessionControl::Reattach {
                instruction,
                ack_tx,
            } => {
                let result = self
                    .handle_session_reattach(
                        connection_id,
                        transport_io
                            .as_mut()
                            .expect("transport should exist while session is active"),
                        session,
                        instruction,
                    )
                    .await;
                let _ = ack_tx.send(result);
            }
            SessionControl::Export { ack_tx } => {
                let prepared = self
                    .prepare_session_export_for_upgrade(connection_id, session, read_buffer, rx)
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
        session: &SessionState,
        read_buffer: &BytesMut,
        rx: &mut mpsc::Receiver<SessionMessage>,
    ) -> Result<RuntimeUpgradeSessionState, RuntimeError> {
        let protocol_session_blob = match session.adapter.as_ref() {
            Some(adapter) => Some(
                adapter
                    .export_session_state(&Self::protocol_session_snapshot(connection_id, session))
                    .map_err(|error| RuntimeError::Config(error.to_string()))?,
            ),
            None => None,
        };
        let gameplay_session_blob = match (
            session.gameplay.as_ref(),
            session.session_capabilities.as_ref(),
            session.player_id,
        ) {
            (Some(gameplay), Some(session_capabilities), Some(player_id)) => Some(
                gameplay
                    .export_session_state(
                        &mc_plugin_api::codec::gameplay::GameplaySessionSnapshot {
                            phase: session.phase,
                            player_id: Some(player_id),
                            entity_id: session.entity_id,
                            protocol: session_capabilities.protocol.clone(),
                            gameplay_profile: session_capabilities.gameplay_profile.clone(),
                            protocol_generation: session_capabilities.protocol_generation,
                            gameplay_generation: session_capabilities.gameplay_generation,
                        },
                    )
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
            generation_id: session.generation.generation_id,
            transport: session.transport,
            phase: session.phase,
            adapter_id: session
                .adapter
                .as_ref()
                .map(|adapter| adapter.descriptor().adapter_id),
            player_id: session.player_id,
            entity_id: session.entity_id,
            gameplay_profile: session
                .session_capabilities
                .as_ref()
                .map(|session_capabilities| session_capabilities.gameplay_profile.clone()),
            protocol_generation: session
                .session_capabilities
                .as_ref()
                .and_then(|session_capabilities| session_capabilities.protocol_generation),
            gameplay_generation: session
                .session_capabilities
                .as_ref()
                .and_then(|session_capabilities| session_capabilities.gameplay_generation),
            login_challenge: session.login_challenge.as_ref().map(|challenge| {
                RuntimeUpgradeLoginChallenge {
                    username: challenge.username.clone(),
                    verify_token: challenge.verify_token,
                }
            }),
            read_buffer: read_buffer.to_vec(),
            queued_messages,
            encryption: None,
            protocol_session_blob,
            gameplay_session_blob,
        })
    }
}
