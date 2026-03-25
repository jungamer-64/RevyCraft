use crate::RuntimeError;
use crate::runtime::{
    GenerationAdmission, RuntimeServer, SESSION_OUTBOUND_QUEUE_CAPACITY, SessionMessage,
    SessionState,
};
use crate::transport::{AcceptedTransportSession, TransportSessionIo, default_wire_codec};
use bytes::BytesMut;
use mc_core::ConnectionId;
use mc_proto_common::{ConnectionPhase, TransportKind, WireCodec};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

impl RuntimeServer {
    pub(in crate::runtime) async fn spawn_transport_session(
        self: &Arc<Self>,
        generation_id: crate::runtime::GenerationId,
        transport_session: AcceptedTransportSession,
    ) {
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
    }

    async fn spawn_session_with_state_guarded(
        self: &Arc<Self>,
        transport_session: AcceptedTransportSession,
        session: SessionState,
    ) {
        let connection_id = self.sessions.next_connection_id().await;

        let (tx, rx) = mpsc::channel(SESSION_OUTBOUND_QUEUE_CAPACITY);
        let (control_tx, control_rx) = watch::channel(None);
        self.sessions
            .insert(connection_id, tx, control_tx, &session)
            .await;

        let server = Arc::clone(self);
        self.sessions
            .spawn_task(async move {
                (
                    connection_id,
                    server
                        .run_session(connection_id, transport_session.io, session, rx, control_rx)
                        .await,
                )
            })
            .await;
    }

    async fn run_session(
        self: Arc<Self>,
        connection_id: ConnectionId,
        mut transport_io: TransportSessionIo,
        mut session: SessionState,
        mut rx: mpsc::Receiver<SessionMessage>,
        mut control_rx: watch::Receiver<Option<String>>,
    ) -> Result<(), RuntimeError> {
        let mut read_buffer = BytesMut::with_capacity(8192);

        let result = async {
            loop {
                tokio::select! {
                    read = transport_io.read_into(&mut read_buffer) => {
                        let bytes_read = read?;
                        if bytes_read == 0 {
                            break;
                        }
                        loop {
                            let codec: &dyn WireCodec = match session.adapter.as_ref() {
                                Some(current) => current.wire_codec(),
                                None => default_wire_codec(session.transport)?,
                            };
                            let Some(frame) = codec.try_decode_frame(&mut read_buffer)? else {
                                break;
                            };
                            let should_close = self
                                .handle_incoming_frame(
                                    connection_id,
                                    &mut transport_io,
                                    &mut session,
                                    frame,
                                )
                                .await?;
                            if should_close {
                                return Ok(());
                            }
                        }
                    }
                    control = control_rx.changed() => {
                        if control.is_err() {
                            break;
                        }
                        let reason = { control_rx.borrow().clone() };
                        if let Some(reason) = reason {
                            let should_close = self
                                .handle_outgoing_message(
                                    connection_id,
                                    &mut transport_io,
                                    &mut session,
                                    SessionMessage::Terminate { reason },
                                )
                                .await?;
                            if should_close {
                                return Ok(());
                            }
                        }
                    }
                    maybe_message = rx.recv() => {
                        let Some(message) = maybe_message else {
                            break;
                        };
                        let should_close = self
                            .handle_outgoing_message(
                                connection_id,
                                &mut transport_io,
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

        let cleanup = self.unregister_session(connection_id, &session).await;
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
}
