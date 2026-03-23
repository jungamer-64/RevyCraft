use crate::codec::__internal::binary::{Decoder, Encoder, ProtocolCodecError};
use crate::codec::__internal::shared::{
    decode_capability_announcement, decode_option, decode_world_snapshot,
    encode_capability_announcement, encode_option, encode_world_snapshot,
};
use crate::codec::storage::{StorageDescriptor, StorageOpCode, StorageRequest, StorageResponse};
use mc_core::StorageProfileId;

pub(crate) fn encode_storage_request_payload(
    encoder: &mut Encoder,
    request: &StorageRequest,
) -> Result<(), ProtocolCodecError> {
    match request {
        StorageRequest::Describe | StorageRequest::CapabilitySet => Ok(()),
        StorageRequest::LoadSnapshot { world_dir }
        | StorageRequest::ExportRuntimeState { world_dir } => encoder.write_string(world_dir),
        StorageRequest::SaveSnapshot {
            world_dir,
            snapshot,
        }
        | StorageRequest::ImportRuntimeState {
            world_dir,
            snapshot,
        } => {
            encoder.write_string(world_dir)?;
            encode_world_snapshot(encoder, snapshot)
        }
    }
}

pub(crate) fn decode_storage_request_payload(
    decoder: &mut Decoder<'_>,
    op_code: StorageOpCode,
) -> Result<StorageRequest, ProtocolCodecError> {
    match op_code {
        StorageOpCode::Describe => Ok(StorageRequest::Describe),
        StorageOpCode::CapabilitySet => Ok(StorageRequest::CapabilitySet),
        StorageOpCode::LoadSnapshot => Ok(StorageRequest::LoadSnapshot {
            world_dir: decoder.read_string()?,
        }),
        StorageOpCode::SaveSnapshot => Ok(StorageRequest::SaveSnapshot {
            world_dir: decoder.read_string()?,
            snapshot: decode_world_snapshot(decoder)?,
        }),
        StorageOpCode::ExportRuntimeState => Ok(StorageRequest::ExportRuntimeState {
            world_dir: decoder.read_string()?,
        }),
        StorageOpCode::ImportRuntimeState => Ok(StorageRequest::ImportRuntimeState {
            world_dir: decoder.read_string()?,
            snapshot: decode_world_snapshot(decoder)?,
        }),
    }
}

pub(crate) fn encode_storage_response_payload(
    encoder: &mut Encoder,
    op_code: StorageOpCode,
    response: &StorageResponse,
) -> Result<(), ProtocolCodecError> {
    match (op_code, response) {
        (StorageOpCode::Describe, StorageResponse::Descriptor(descriptor)) => {
            encoder.write_string(descriptor.storage_profile.as_str())
        }
        (StorageOpCode::CapabilitySet, StorageResponse::CapabilitySet(capabilities)) => {
            encode_capability_announcement(encoder, capabilities)
        }
        (
            StorageOpCode::LoadSnapshot | StorageOpCode::ExportRuntimeState,
            StorageResponse::Snapshot(snapshot),
        ) => encode_option(encoder, snapshot.as_ref(), encode_world_snapshot),
        (
            StorageOpCode::SaveSnapshot | StorageOpCode::ImportRuntimeState,
            StorageResponse::Empty,
        ) => Ok(()),
        _ => Err(ProtocolCodecError::InvalidValue(
            "storage response did not match opcode",
        )),
    }
}

pub(crate) fn decode_storage_response_payload(
    decoder: &mut Decoder<'_>,
    op_code: StorageOpCode,
) -> Result<StorageResponse, ProtocolCodecError> {
    match op_code {
        StorageOpCode::Describe => Ok(StorageResponse::Descriptor(StorageDescriptor {
            storage_profile: StorageProfileId::new(decoder.read_string()?),
        })),
        StorageOpCode::CapabilitySet => Ok(StorageResponse::CapabilitySet(
            decode_capability_announcement(decoder)?,
        )),
        StorageOpCode::LoadSnapshot | StorageOpCode::ExportRuntimeState => Ok(
            StorageResponse::Snapshot(decode_option(decoder, decode_world_snapshot)?),
        ),
        StorageOpCode::SaveSnapshot | StorageOpCode::ImportRuntimeState => {
            Ok(StorageResponse::Empty)
        }
    }
}
