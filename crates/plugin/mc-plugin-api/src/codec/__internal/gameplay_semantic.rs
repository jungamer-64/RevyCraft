use crate::codec::__internal::binary::{Decoder, Encoder, ProtocolCodecError};
use crate::codec::__internal::shared::{
    decode_capability_announcement, decode_connection_phase, decode_core_event, decode_entity_id,
    decode_gameplay_command, decode_option, decode_player_id, encode_capability_announcement,
    encode_connection_phase, encode_core_event, encode_entity_id, encode_gameplay_command,
    encode_option, encode_player_id,
};
use crate::codec::gameplay::{
    GameplayDescriptor, GameplayOpCode, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
};
use mc_core::{
    CapabilityAnnouncement, EventTarget, GameplayProfileId, ProtocolCapability, TargetedEvent,
};

pub(crate) fn encode_gameplay_request_payload(
    encoder: &mut Encoder,
    request: &GameplayRequest,
) -> Result<(), ProtocolCodecError> {
    match request {
        GameplayRequest::Describe | GameplayRequest::CapabilitySet => Ok(()),
        GameplayRequest::HandlePlayerJoin { session, player_id } => {
            encode_gameplay_session_snapshot(encoder, session)?;
            encode_player_id(encoder, *player_id);
            Ok(())
        }
        GameplayRequest::HandleCommand { session, command } => {
            encode_gameplay_session_snapshot(encoder, session)?;
            encode_gameplay_command(encoder, command)
        }
        GameplayRequest::HandleTick { session, now_ms } => {
            encode_gameplay_session_snapshot(encoder, session)?;
            encoder.write_u64(*now_ms);
            Ok(())
        }
        GameplayRequest::SessionClosed { session }
        | GameplayRequest::ExportSessionState { session } => {
            encode_gameplay_session_snapshot(encoder, session)
        }
        GameplayRequest::ImportSessionState { session, blob } => {
            encode_gameplay_session_snapshot(encoder, session)?;
            encoder.write_bytes(blob)
        }
    }
}

pub(crate) fn decode_gameplay_request_payload(
    decoder: &mut Decoder<'_>,
    op_code: GameplayOpCode,
) -> Result<GameplayRequest, ProtocolCodecError> {
    match op_code {
        GameplayOpCode::Describe => Ok(GameplayRequest::Describe),
        GameplayOpCode::CapabilitySet => Ok(GameplayRequest::CapabilitySet),
        GameplayOpCode::HandlePlayerJoin => Ok(GameplayRequest::HandlePlayerJoin {
            session: decode_gameplay_session_snapshot(decoder)?,
            player_id: decode_player_id(decoder)?,
        }),
        GameplayOpCode::HandleCommand => Ok(GameplayRequest::HandleCommand {
            session: decode_gameplay_session_snapshot(decoder)?,
            command: decode_gameplay_command(decoder)?,
        }),
        GameplayOpCode::HandleTick => Ok(GameplayRequest::HandleTick {
            session: decode_gameplay_session_snapshot(decoder)?,
            now_ms: decoder.read_u64()?,
        }),
        GameplayOpCode::SessionClosed => Ok(GameplayRequest::SessionClosed {
            session: decode_gameplay_session_snapshot(decoder)?,
        }),
        GameplayOpCode::ExportSessionState => Ok(GameplayRequest::ExportSessionState {
            session: decode_gameplay_session_snapshot(decoder)?,
        }),
        GameplayOpCode::ImportSessionState => Ok(GameplayRequest::ImportSessionState {
            session: decode_gameplay_session_snapshot(decoder)?,
            blob: decoder.read_bytes()?,
        }),
    }
}

pub(crate) fn encode_gameplay_response_payload(
    encoder: &mut Encoder,
    op_code: GameplayOpCode,
    response: &GameplayResponse,
) -> Result<(), ProtocolCodecError> {
    match (op_code, response) {
        (GameplayOpCode::Describe, GameplayResponse::Descriptor(descriptor)) => {
            encode_gameplay_descriptor(encoder, descriptor)
        }
        (GameplayOpCode::CapabilitySet, GameplayResponse::CapabilitySet(capability_set)) => {
            encode_capability_announcement(encoder, capability_set)
        }
        (
            GameplayOpCode::HandlePlayerJoin
            | GameplayOpCode::HandleCommand
            | GameplayOpCode::HandleTick
            | GameplayOpCode::SessionClosed
            | GameplayOpCode::ImportSessionState,
            GameplayResponse::Empty,
        ) => Ok(()),
        (GameplayOpCode::ExportSessionState, GameplayResponse::SessionTransferBlob(blob)) => {
            encoder.write_bytes(blob)
        }
        _ => Err(ProtocolCodecError::InvalidValue(
            "unexpected gameplay response payload",
        )),
    }
}

pub(crate) fn decode_gameplay_response_payload(
    decoder: &mut Decoder<'_>,
    op_code: GameplayOpCode,
) -> Result<GameplayResponse, ProtocolCodecError> {
    match op_code {
        GameplayOpCode::Describe => Ok(GameplayResponse::Descriptor(decode_gameplay_descriptor(
            decoder,
        )?)),
        GameplayOpCode::CapabilitySet => Ok(GameplayResponse::CapabilitySet(
            decode_capability_announcement(decoder)?,
        )),
        GameplayOpCode::HandlePlayerJoin
        | GameplayOpCode::HandleCommand
        | GameplayOpCode::HandleTick
        | GameplayOpCode::SessionClosed
        | GameplayOpCode::ImportSessionState => Ok(GameplayResponse::Empty),
        GameplayOpCode::ExportSessionState => {
            Ok(GameplayResponse::SessionTransferBlob(decoder.read_bytes()?))
        }
    }
}

pub(crate) fn encode_gameplay_descriptor(
    encoder: &mut Encoder,
    descriptor: &GameplayDescriptor,
) -> Result<(), ProtocolCodecError> {
    encoder.write_string(descriptor.profile.as_str())
}

pub(crate) fn decode_gameplay_descriptor(
    decoder: &mut Decoder<'_>,
) -> Result<GameplayDescriptor, ProtocolCodecError> {
    Ok(GameplayDescriptor {
        profile: GameplayProfileId::new(decoder.read_string()?),
    })
}

pub(crate) fn encode_gameplay_session_snapshot(
    encoder: &mut Encoder,
    snapshot: &GameplaySessionSnapshot,
) -> Result<(), ProtocolCodecError> {
    encode_connection_phase(encoder, snapshot.phase);
    encode_option(
        encoder,
        snapshot.player_id.as_ref(),
        |encoder, player_id| {
            encode_player_id(encoder, *player_id);
            Ok(())
        },
    )?;
    encode_option(
        encoder,
        snapshot.entity_id.as_ref(),
        |encoder, entity_id| {
            encode_entity_id(encoder, *entity_id);
            Ok(())
        },
    )?;
    encode_capability_announcement(
        encoder,
        &CapabilityAnnouncement::<ProtocolCapability>::new(snapshot.protocol.clone()),
    )?;
    encoder.write_string(snapshot.gameplay_profile.as_str())?;
    encode_option(
        encoder,
        snapshot.protocol_generation.as_ref(),
        |encoder, generation| {
            encoder.write_u64(generation.0);
            Ok(())
        },
    )?;
    encode_option(
        encoder,
        snapshot.gameplay_generation.as_ref(),
        |encoder, generation| {
            encoder.write_u64(generation.0);
            Ok(())
        },
    )
}

pub(crate) fn decode_gameplay_session_snapshot(
    decoder: &mut Decoder<'_>,
) -> Result<GameplaySessionSnapshot, ProtocolCodecError> {
    Ok(GameplaySessionSnapshot {
        phase: decode_connection_phase(decoder)?,
        player_id: decode_option(decoder, decode_player_id)?,
        entity_id: decode_option(decoder, decode_entity_id)?,
        protocol: decode_capability_announcement::<ProtocolCapability>(decoder)?.capabilities,
        gameplay_profile: GameplayProfileId::new(decoder.read_string()?),
        protocol_generation: decode_option(decoder, |decoder| {
            Ok(mc_core::PluginGenerationId(decoder.read_u64()?))
        })?,
        gameplay_generation: decode_option(decoder, |decoder| {
            Ok(mc_core::PluginGenerationId(decoder.read_u64()?))
        })?,
    })
}

#[expect(
    dead_code,
    reason = "vector helpers are retained for future multi-event host payloads"
)]
pub(crate) fn encode_targeted_events(
    encoder: &mut Encoder,
    events: &[TargetedEvent],
) -> Result<(), ProtocolCodecError> {
    encoder.write_len(events.len())?;
    for event in events {
        encode_targeted_event(encoder, event)?;
    }
    Ok(())
}

#[expect(
    dead_code,
    reason = "vector helpers are retained for future multi-event host payloads"
)]
pub(crate) fn decode_targeted_events(
    decoder: &mut Decoder<'_>,
) -> Result<Vec<TargetedEvent>, ProtocolCodecError> {
    let len = decoder.read_len()?;
    let mut events = Vec::with_capacity(len);
    for _ in 0..len {
        events.push(decode_targeted_event(decoder)?);
    }
    Ok(events)
}

pub(crate) fn encode_targeted_event(
    encoder: &mut Encoder,
    event: &TargetedEvent,
) -> Result<(), ProtocolCodecError> {
    match event.target {
        EventTarget::Connection(connection_id) => {
            encoder.write_u8(1);
            encoder.write_u64(connection_id.0);
        }
        EventTarget::Player(player_id) => {
            encoder.write_u8(2);
            encode_player_id(encoder, player_id);
        }
        EventTarget::EveryoneExcept(player_id) => {
            encoder.write_u8(3);
            encode_player_id(encoder, player_id);
        }
    }
    encode_core_event(encoder, &event.event)
}

pub(crate) fn decode_targeted_event(
    decoder: &mut Decoder<'_>,
) -> Result<TargetedEvent, ProtocolCodecError> {
    let target = match decoder.read_u8()? {
        1 => EventTarget::Connection(mc_core::ConnectionId(decoder.read_u64()?)),
        2 => EventTarget::Player(decode_player_id(decoder)?),
        3 => EventTarget::EveryoneExcept(decode_player_id(decoder)?),
        _ => {
            return Err(ProtocolCodecError::InvalidValue(
                "invalid targeted event tag",
            ));
        }
    };
    Ok(TargetedEvent {
        target,
        event: decode_core_event(decoder)?,
    })
}
