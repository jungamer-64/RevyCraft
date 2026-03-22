use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SessionHandle, SessionMessage, SessionState};
use crate::transport::{AcceptedTransportSession, TransportSessionIo, default_wire_codec};
use bytes::BytesMut;
use mc_core::ConnectionId;
use mc_proto_common::{ConnectionPhase, TransportKind, WireCodec};
use std::sync::Arc;
use tokio::sync::mpsc;

impl RuntimeServer {
    pub(in crate::runtime) async fn spawn_transport_session(
        self: &Arc<Self>,
        topology_generation_id: crate::runtime::TopologyGenerationId,
        transport_session: AcceptedTransportSession,
    ) {
        let Some(topology) = self.topology_generation(topology_generation_id) else {
            eprintln!(
                "dropping transport session because topology generation {:?} is no longer active",
                topology_generation_id
            );
            return;
        };
        let session = match transport_session.transport {
            TransportKind::Tcp => SessionState {
                topology_generation_id,
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
                let Some(adapter) = topology.default_bedrock_adapter.clone() else {
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
                    topology_generation_id,
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
        self.spawn_session_with_state(transport_session, session)
            .await;
    }

    async fn spawn_session_with_state(
        self: &Arc<Self>,
        transport_session: AcceptedTransportSession,
        session: SessionState,
    ) {
        let _consistency_guard = self.consistency_gate.read().await;
        self.spawn_session_with_state_guarded(transport_session, session)
            .await;
    }

    async fn spawn_session_with_state_guarded(
        self: &Arc<Self>,
        transport_session: AcceptedTransportSession,
        session: SessionState,
    ) {
        let connection_id = {
            let mut next_connection_id = self.next_connection_id.lock().await;
            let connection_id = ConnectionId(*next_connection_id);
            *next_connection_id = next_connection_id.saturating_add(1);
            connection_id
        };

        let (tx, rx) = mpsc::unbounded_channel();
        self.sessions.lock().await.insert(
            connection_id,
            SessionHandle {
                tx,
                topology_generation_id: session.topology_generation_id,
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
                    .map(|capabilities| capabilities.gameplay_profile.clone()),
                session_capabilities: session.session_capabilities.clone(),
            },
        );

        let server = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = server
                .run_session(connection_id, transport_session.io, session, rx)
                .await
            {
                eprintln!("session {connection_id:?} ended with error: {error}");
            }
        });
    }

    async fn run_session(
        self: Arc<Self>,
        connection_id: ConnectionId,
        mut transport_io: TransportSessionIo,
        mut session: SessionState,
        mut rx: mpsc::UnboundedReceiver<SessionMessage>,
    ) -> Result<(), RuntimeError> {
        let mut read_buffer = BytesMut::with_capacity(8192);

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
                            self.unregister_session(connection_id, &session).await?;
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
                        self.unregister_session(connection_id, &session).await?;
                        return Ok(());
                    }
                }
            }
        }

        self.unregister_session(connection_id, &session).await?;
        Ok(())
    }
}
