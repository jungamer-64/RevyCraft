use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SessionMessage, SessionReattachInstruction, SessionState};
use crate::transport::{TransportSessionIo, write_payload};
use mc_core::CoreEvent;
use mc_proto_common::{ConnectionPhase, PlayEncodingContext};

impl RuntimeServer {
    pub(in crate::runtime::session) async fn handle_session_reattach(
        &self,
        connection_id: mc_core::ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        instruction: SessionReattachInstruction,
    ) -> Result<(), RuntimeError> {
        session.generation = instruction.generation;
        session.adapter = instruction.adapter;
        session.gameplay = instruction.gameplay;
        session.phase = instruction.phase;
        session.player_id = instruction.player_id;
        session.entity_id = instruction.entity_id;
        Self::refresh_session_capabilities(session);
        for event in instruction.resync_events {
            let should_close = self
                .handle_outgoing_message(
                    connection_id,
                    transport_io,
                    session,
                    SessionMessage::Event(event),
                )
                .await?;
            if should_close {
                return Err(RuntimeError::Config(
                    "session terminated while applying reattach resync events".to_string(),
                ));
            }
        }
        self.sync_session_handle_direct(connection_id, session)
            .await;
        Ok(())
    }

    pub(in crate::runtime::session) async fn handle_outgoing_message(
        &self,
        connection_id: mc_core::ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        message: SessionMessage,
    ) -> Result<bool, RuntimeError> {
        match message {
            SessionMessage::Event(event) => {
                let event = event.as_ref();
                Self::refresh_session_capabilities(session);
                let current = session
                    .adapter
                    .as_ref()
                    .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
                let packets = match &event {
                    CoreEvent::LoginAccepted { player, .. } => {
                        vec![current.encode_login_success(player)?]
                    }
                    CoreEvent::Disconnect { reason } => {
                        vec![current.encode_disconnect(session.phase, reason)?]
                    }
                    _ => {
                        let player_id = session.player_id.ok_or_else(|| {
                            RuntimeError::Config(
                                "missing player id for play event encoding".to_string(),
                            )
                        })?;
                        let entity_id = session.entity_id.ok_or_else(|| {
                            RuntimeError::Config(
                                "missing entity id for play event encoding".to_string(),
                            )
                        })?;
                        let snapshot = Self::protocol_session_snapshot(connection_id, session);
                        current.encode_play_event(
                            event,
                            &snapshot,
                            &PlayEncodingContext {
                                player_id,
                                entity_id,
                            },
                        )?
                    }
                };
                for packet in packets {
                    write_payload(transport_io, current.wire_codec(), &packet).await?;
                }

                match event {
                    CoreEvent::LoginAccepted {
                        player_id: accepted_player_id,
                        entity_id: accepted_entity_id,
                        ..
                    } => {
                        session.player_id = Some(*accepted_player_id);
                        session.entity_id = Some(*accepted_entity_id);
                        session.phase = ConnectionPhase::Play;
                        Self::refresh_session_capabilities(session);
                        self.sync_session_handle(connection_id, session).await;
                    }
                    CoreEvent::Disconnect { .. } => return Ok(true),
                    _ => {}
                }
            }
            SessionMessage::Terminate { reason } => {
                if session.phase == ConnectionPhase::Play
                    && let Some(current) = session.adapter.as_ref()
                    && let Ok(packet) = current.encode_disconnect(ConnectionPhase::Play, &reason)
                {
                    let _ = write_payload(transport_io, current.wire_codec(), &packet).await;
                }
                return Ok(true);
            }
        }
        Ok(false)
    }
}
