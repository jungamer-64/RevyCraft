use crate::RuntimeError;
use crate::runtime::{LoginSession, PlaySession, RuntimeServer, SessionMessage, SessionPhase};
use crate::transport::{TransportSessionIo, write_payload_batch, write_payload_confirmed};
use mc_proto_common::{ConnectionPhase, PlayEncodingContext};
use revy_voxel_core::CoreEvent;
use std::sync::Arc;

impl RuntimeServer {
    pub(in crate::runtime::session) async fn handle_outgoing_message(
        &self,
        connection_id: revy_voxel_core::ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionPhase,
        message: SessionMessage,
    ) -> Result<bool, RuntimeError> {
        match message {
            SessionMessage::Events(events) => {
                let mut pending_payloads = Vec::new();
                for event in events {
                    if self
                        .handle_outgoing_event(
                            connection_id,
                            transport_io,
                            session,
                            event,
                            &mut pending_payloads,
                        )
                        .await?
                    {
                        return Ok(true);
                    }
                }
                if !pending_payloads.is_empty() {
                    let codec = session
                        .binding()
                        .ok_or_else(|| {
                            RuntimeError::Config(
                                "outgoing event batch ended in an unbound session phase"
                                    .to_string(),
                            )
                        })?
                        .adapter
                        .wire_codec();
                    write_payload_batch(transport_io, codec, pending_payloads).await?;
                }
            }
            SessionMessage::Terminate { reason } => {
                if let SessionPhase::Play(play) = session
                    && let Ok(packet) = play
                        .binding
                        .adapter
                        .encode_disconnect(ConnectionPhase::Play, &reason)
                {
                    let _ = write_payload_confirmed(
                        transport_io,
                        play.binding.adapter.wire_codec(),
                        &packet,
                    )
                    .await;
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn handle_outgoing_event(
        &self,
        connection_id: revy_voxel_core::ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionPhase,
        event: Arc<CoreEvent>,
        pending_payloads: &mut Vec<Vec<u8>>,
    ) -> Result<bool, RuntimeError> {
        let adapter = session
            .binding()
            .map(|binding| Arc::clone(&binding.adapter))
            .ok_or_else(|| {
                RuntimeError::Config("outgoing event reached an unbound session phase".to_string())
            })?;
        let packets = match event.as_ref() {
            CoreEvent::LoginAccepted {
                player,
                player_id,
                entity_id,
            } => {
                let SessionPhase::Login(login) = session else {
                    return Err(RuntimeError::Config(
                        "login acceptance reached a non-login session".to_string(),
                    ));
                };
                let binding = login.binding().with_entity_id(*entity_id);
                *session = SessionPhase::Login(LoginSession::AcceptedWritePending {
                    binding,
                    player_id: *player_id,
                    entity_id: *entity_id,
                });
                vec![adapter.encode_login_success(player)?]
            }
            CoreEvent::Disconnect { reason } => {
                vec![adapter.encode_disconnect(session.phase(), reason)?]
            }
            event => {
                let SessionPhase::Play(play) = session else {
                    return Err(RuntimeError::Config(
                        "play event reached a session that is not in play".to_string(),
                    ));
                };
                let snapshot = mc_proto_common::ProtocolSessionSnapshot {
                    connection_id,
                    phase: ConnectionPhase::Play,
                    player_id: Some(play.player_id),
                    entity_id: Some(play.entity_id),
                };
                adapter.encode_play_event(
                    event,
                    &snapshot,
                    &PlayEncodingContext {
                        player_id: play.player_id,
                        entity_id: play.entity_id,
                    },
                )?
            }
        };
        let confirm_write = matches!(
            event.as_ref(),
            CoreEvent::LoginAccepted { .. } | CoreEvent::Disconnect { .. }
        );
        if confirm_write {
            if !pending_payloads.is_empty() {
                write_payload_batch(
                    transport_io,
                    adapter.wire_codec(),
                    std::mem::take(pending_payloads),
                )
                .await?;
            }
            for packet in packets {
                write_payload_confirmed(transport_io, adapter.wire_codec(), &packet).await?;
            }
        } else {
            pending_payloads.extend(packets);
        }

        match event.as_ref() {
            CoreEvent::LoginAccepted { .. } => {
                let SessionPhase::Login(LoginSession::AcceptedWritePending {
                    binding,
                    player_id,
                    entity_id,
                }) = session
                else {
                    return Err(RuntimeError::Config(
                        "login success write completed without pending acceptance".to_string(),
                    ));
                };
                *session = SessionPhase::Play(PlaySession {
                    binding: binding.clone(),
                    player_id: *player_id,
                    entity_id: *entity_id,
                });
            }
            CoreEvent::Disconnect { .. } => return Ok(true),
            _ => {}
        }
        Ok(false)
    }
}
