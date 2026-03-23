use crate::abi::{CURRENT_PLUGIN_ABI, PluginKind};
use crate::codec::__internal::binary::{
    Decoder, Encoder, EnvelopeHeader, PROTOCOL_FLAG_RESPONSE, ProtocolCodecError, decode_envelope,
    encode_envelope,
};
use crate::codec::__internal::storage_semantic::{
    decode_storage_request_payload, decode_storage_response_payload,
    encode_storage_request_payload, encode_storage_response_payload,
};
use mc_core::{CapabilityAnnouncement, StorageCapability, StorageProfileId, WorldSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum StorageOpCode {
    Describe = 1,
    CapabilitySet = 2,
    LoadSnapshot = 3,
    SaveSnapshot = 4,
    ExportRuntimeState = 5,
    ImportRuntimeState = 6,
}

impl TryFrom<u8> for StorageOpCode {
    type Error = ProtocolCodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Describe),
            2 => Ok(Self::CapabilitySet),
            3 => Ok(Self::LoadSnapshot),
            4 => Ok(Self::SaveSnapshot),
            5 => Ok(Self::ExportRuntimeState),
            6 => Ok(Self::ImportRuntimeState),
            _ => Err(ProtocolCodecError::InvalidValue("invalid storage op code")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageDescriptor {
    pub storage_profile: StorageProfileId,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StorageRequest {
    Describe,
    CapabilitySet,
    LoadSnapshot {
        world_dir: String,
    },
    SaveSnapshot {
        world_dir: String,
        snapshot: WorldSnapshot,
    },
    ExportRuntimeState {
        world_dir: String,
    },
    ImportRuntimeState {
        world_dir: String,
        snapshot: WorldSnapshot,
    },
}

impl StorageRequest {
    #[must_use]
    pub const fn op_code(&self) -> StorageOpCode {
        match self {
            Self::Describe => StorageOpCode::Describe,
            Self::CapabilitySet => StorageOpCode::CapabilitySet,
            Self::LoadSnapshot { .. } => StorageOpCode::LoadSnapshot,
            Self::SaveSnapshot { .. } => StorageOpCode::SaveSnapshot,
            Self::ExportRuntimeState { .. } => StorageOpCode::ExportRuntimeState,
            Self::ImportRuntimeState { .. } => StorageOpCode::ImportRuntimeState,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StorageResponse {
    Descriptor(StorageDescriptor),
    CapabilitySet(CapabilityAnnouncement<StorageCapability>),
    Snapshot(Option<WorldSnapshot>),
    Empty,
}

/// Encodes a storage request into the plugin protocol envelope.
///
/// # Errors
///
/// Returns an error when the request payload exceeds protocol length limits or contains values
/// that cannot be serialized.
pub fn encode_storage_request(request: &StorageRequest) -> Result<Vec<u8>, ProtocolCodecError> {
    let mut payload = Encoder::default();
    encode_storage_request_payload(&mut payload, request)?;
    let payload = payload.into_inner();
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::Storage,
            op_code: request.op_code() as u8,
            flags: 0,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

/// Decodes a storage request from the plugin protocol envelope.
///
/// # Errors
///
/// Returns an error when the envelope is malformed, the plugin kind/opcode is invalid, or the
/// storage payload cannot be decoded.
pub fn decode_storage_request(bytes: &[u8]) -> Result<StorageRequest, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::Storage {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "storage request had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE != 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "storage request unexpectedly set response flag",
        ));
    }
    let mut decoder = Decoder::new(payload);
    let request =
        decode_storage_request_payload(&mut decoder, StorageOpCode::try_from(header.op_code)?)?;
    decoder.finish()?;
    Ok(request)
}

/// Encodes a storage response for the provided storage request.
///
/// # Errors
///
/// Returns an error when the response does not match the request opcode, exceeds protocol
/// length limits, or contains values that cannot be serialized.
pub fn encode_storage_response(
    request: &StorageRequest,
    response: &StorageResponse,
) -> Result<Vec<u8>, ProtocolCodecError> {
    let mut payload = Encoder::default();
    encode_storage_response_payload(&mut payload, request.op_code(), response)?;
    let payload = payload.into_inner();
    encode_envelope(
        EnvelopeHeader {
            abi: CURRENT_PLUGIN_ABI,
            plugin_kind: PluginKind::Storage,
            op_code: request.op_code() as u8,
            flags: PROTOCOL_FLAG_RESPONSE,
            payload_len: u32::try_from(payload.len())
                .map_err(|_| ProtocolCodecError::LengthOverflow)?,
        },
        &payload,
    )
}

/// Decodes a storage response for the provided storage request.
///
/// # Errors
///
/// Returns an error when the envelope is malformed, the response opcode does not match the
/// request, or the storage payload cannot be decoded.
pub fn decode_storage_response(
    request: &StorageRequest,
    bytes: &[u8],
) -> Result<StorageResponse, ProtocolCodecError> {
    let (header, payload) = decode_envelope(bytes)?;
    if header.plugin_kind != PluginKind::Storage {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "storage response had wrong plugin kind",
        ));
    }
    if header.flags & PROTOCOL_FLAG_RESPONSE == 0 {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "storage response was missing response flag",
        ));
    }
    if StorageOpCode::try_from(header.op_code)? != request.op_code() {
        return Err(ProtocolCodecError::InvalidEnvelope(
            "storage response opcode did not match request",
        ));
    }
    let mut decoder = Decoder::new(payload);
    let response = decode_storage_response_payload(&mut decoder, request.op_code())?;
    decoder.finish()?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::{
        StorageDescriptor, StorageRequest, StorageResponse, decode_storage_request,
        decode_storage_response, encode_storage_request, encode_storage_response,
    };
    use mc_core::{BlockPos, CoreCommand, CoreConfig, PlayerId, ServerCore};
    use uuid::Uuid;

    fn sample_snapshot() -> mc_core::WorldSnapshot {
        let mut core = ServerCore::new(CoreConfig::default());
        let _ = core.apply_command(
            CoreCommand::LoginStart {
                connection_id: mc_core::ConnectionId(1),
                username: "alice".to_string(),
                player_id: PlayerId(Uuid::from_u128(7)),
            },
            0,
        );
        let mut snapshot = core.snapshot();
        snapshot.meta.spawn = BlockPos::new(1, 5, -2);
        snapshot
    }

    #[test]
    fn storage_request_roundtrip() {
        let request = StorageRequest::ImportRuntimeState {
            world_dir: "/tmp/world".to_string(),
            snapshot: sample_snapshot(),
        };
        let encoded = encode_storage_request(&request).expect("request should encode");
        let decoded = decode_storage_request(&encoded).expect("request should decode");
        assert_eq!(decoded, request);
    }

    #[test]
    fn storage_response_roundtrip() {
        let request = StorageRequest::LoadSnapshot {
            world_dir: "/tmp/world".to_string(),
        };
        let response = StorageResponse::Snapshot(Some(sample_snapshot()));
        let encoded = encode_storage_response(&request, &response).expect("response should encode");
        let decoded = decode_storage_response(&request, &encoded).expect("response should decode");
        assert_eq!(decoded, response);
    }

    #[test]
    fn storage_descriptor_roundtrip() {
        let request = StorageRequest::Describe;
        let response = StorageResponse::Descriptor(StorageDescriptor {
            storage_profile: "je-anvil-1_7_10".into(),
        });
        let encoded = encode_storage_response(&request, &response).expect("response should encode");
        let decoded = decode_storage_response(&request, &encoded).expect("response should decode");
        assert_eq!(decoded, response);
    }
}
