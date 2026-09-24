use crate::RuntimeError;
use crate::runtime::{BoundSession, LoginSession, RuntimeServer, SessionPhase};
use crate::transport::{TransportSessionIo, write_payload_confirmed};
use mc_proto_common::{HandshakeNextState, ServerListStatus, StatusRequest, TransportKind};
use revy_voxel_core::ConnectionId;
use std::sync::Arc;

impl RuntimeServer {
    pub(in crate::runtime::session) async fn handle_incoming_frame(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionPhase,
        frame: Vec<u8>,
    ) -> Result<bool, RuntimeError> {
        match session {
            SessionPhase::Handshaking { .. } => {
                self.handle_handshake_frame(transport_io, session, &frame)
                    .await
            }
            SessionPhase::Status(_) => {
                self.handle_status_frame(transport_io, session, &frame)
                    .await
            }
            SessionPhase::Login(_) => {
                self.handle_login_frame(connection_id, transport_io, session, &frame)
                    .await
            }
            SessionPhase::Play(_) => self.handle_play_frame(connection_id, session, &frame).await,
            SessionPhase::Closing { .. } => Ok(true),
        }
    }

    async fn handle_handshake_frame(
        &self,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionPhase,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let SessionPhase::Handshaking { generation } = session else {
            return Err(RuntimeError::Config(
                "handshake frame reached a non-handshaking session".to_string(),
            ));
        };
        let generation = Arc::clone(generation);
        let Some(intent) = generation
            .protocol_registry
            .route_handshake(TransportKind::Tcp, frame)?
        else {
            return Ok(true);
        };
        let next_adapter = generation.protocol_registry.resolve_route(
            TransportKind::Tcp,
            intent.edition,
            intent.protocol_number,
        );
        if let Some(adapter) = next_adapter {
            let gameplay = self
                .resolve_gameplay_for_adapter(&adapter.descriptor().adapter_id)
                .await?;
            let binding =
                BoundSession::new(generation, TransportKind::Tcp, adapter, gameplay, None);
            *session = match intent.next_state {
                HandshakeNextState::Status => SessionPhase::Status(binding),
                HandshakeNextState::Login => {
                    SessionPhase::Login(LoginSession::Negotiating(binding))
                }
            };
            return Ok(false);
        }

        let fallback = Arc::clone(&generation.default_adapter);
        let descriptor = fallback.descriptor();
        match intent.next_state {
            HandshakeNextState::Status => {
                let gameplay = self
                    .resolve_gameplay_for_adapter(&fallback.descriptor().adapter_id)
                    .await?;
                *session = SessionPhase::Status(BoundSession::new(
                    generation,
                    TransportKind::Tcp,
                    fallback,
                    gameplay,
                    None,
                ));
                Ok(false)
            }
            HandshakeNextState::Login => {
                let disconnect = fallback.encode_disconnect(
                    mc_proto_common::ConnectionPhase::Login,
                    &format!(
                        "Unsupported protocol {}. This server supports {} (protocol {}).",
                        intent.protocol_number, descriptor.version_name, descriptor.protocol_number
                    ),
                )?;
                write_payload_confirmed(transport_io, fallback.wire_codec(), &disconnect).await?;
                Ok(true)
            }
        }
    }

    async fn handle_status_frame(
        &self,
        transport_io: &mut TransportSessionIo,
        session: &SessionPhase,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let SessionPhase::Status(binding) = session else {
            return Err(RuntimeError::Config(
                "status frame reached a non-status session".to_string(),
            ));
        };
        match binding.adapter.decode_status(frame)? {
            StatusRequest::Query => {
                let summary = self.player_summary().await;
                let response = binding.adapter.encode_status_response(&ServerListStatus {
                    version: binding.adapter.descriptor(),
                    players_online: summary.online_players,
                    max_players: usize::try_from(binding.generation.config.network.max_players)
                        .expect("u32 player limit fits the supported process address space"),
                    description: binding.generation.config.network.motd.clone(),
                })?;
                write_payload_confirmed(transport_io, binding.adapter.wire_codec(), &response)
                    .await?;
                Ok(false)
            }
            StatusRequest::Ping { payload } => {
                let response = binding.adapter.encode_status_pong(payload)?;
                write_payload_confirmed(transport_io, binding.adapter.wire_codec(), &response)
                    .await?;
                Ok(true)
            }
        }
    }
}
