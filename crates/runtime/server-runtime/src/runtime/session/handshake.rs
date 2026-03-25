use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SessionState};
use crate::transport::{TransportSessionIo, write_payload};
use mc_core::ConnectionId;
use mc_proto_common::{ConnectionPhase, HandshakeNextState, ServerListStatus, StatusRequest};
use std::sync::Arc;

impl RuntimeServer {
    pub(in crate::runtime::session) async fn handle_incoming_frame(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        frame: Vec<u8>,
    ) -> Result<bool, RuntimeError> {
        Self::refresh_session_capabilities(session);
        match session.phase {
            ConnectionPhase::Handshaking => {
                self.handle_handshake_frame(connection_id, transport_io, session, &frame)
                    .await
            }
            ConnectionPhase::Status => {
                self.handle_status_frame(transport_io, session, &frame)
                    .await
            }
            ConnectionPhase::Login => {
                self.handle_login_frame(connection_id, transport_io, session, &frame)
                    .await
            }
            ConnectionPhase::Play => self.handle_play_frame(connection_id, session, &frame).await,
        }
    }

    async fn handle_handshake_frame(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let topology = Arc::clone(&session.generation);
        let Some(intent) = topology
            .protocol_registry
            .route_handshake(session.transport, frame)?
        else {
            return Ok(true);
        };
        let next_phase = match intent.next_state {
            HandshakeNextState::Status => ConnectionPhase::Status,
            HandshakeNextState::Login => ConnectionPhase::Login,
        };
        if let Some(next_adapter) = topology.protocol_registry.resolve_route(
            session.transport,
            intent.edition,
            intent.protocol_number,
        ) {
            let gameplay = self
                .resolve_gameplay_for_adapter(&next_adapter.descriptor().adapter_id)
                .await?;
            session.adapter = Some(next_adapter);
            session.gameplay = Some(gameplay);
            session.phase = next_phase;
            Self::refresh_session_capabilities(session);
            self.sync_session_handle(connection_id, session).await;
            return Ok(false);
        }

        let fallback = Arc::clone(&topology.default_adapter);
        let descriptor = fallback.descriptor();
        match next_phase {
            ConnectionPhase::Status => {
                let gameplay = self
                    .resolve_gameplay_for_adapter(&fallback.descriptor().adapter_id)
                    .await?;
                session.adapter = Some(fallback);
                session.gameplay = Some(gameplay);
                session.phase = ConnectionPhase::Status;
                Self::refresh_session_capabilities(session);
                self.sync_session_handle(connection_id, session).await;
                Ok(false)
            }
            ConnectionPhase::Login => {
                let disconnect = fallback.encode_disconnect(
                    ConnectionPhase::Login,
                    &format!(
                        "Unsupported protocol {}. This server supports {} (protocol {}).",
                        intent.protocol_number, descriptor.version_name, descriptor.protocol_number
                    ),
                )?;
                write_payload(transport_io, fallback.wire_codec(), &disconnect).await?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    async fn handle_status_frame(
        &self,
        transport_io: &mut TransportSessionIo,
        session: &SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let topology = Arc::clone(&session.generation);
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        match current.decode_status(frame)? {
            StatusRequest::Query => {
                let summary = self.player_summary().await;
                let response = current.encode_status_response(&ServerListStatus {
                    version: current.descriptor(),
                    players_online: summary.online_players,
                    max_players: usize::from(topology.config.network.max_players),
                    description: topology.config.network.motd.clone(),
                })?;
                write_payload(transport_io, current.wire_codec(), &response).await?;
                Ok(false)
            }
            StatusRequest::Ping { payload } => {
                let response = current.encode_status_pong(payload)?;
                write_payload(transport_io, current.wire_codec(), &response).await?;
                Ok(true)
            }
        }
    }
}
