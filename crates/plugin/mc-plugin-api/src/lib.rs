#![allow(clippy::multiple_crate_versions)]
mod abi;
mod auth_codec;
mod gameplay_codec;
mod host_api;
mod manifest;
pub(crate) mod protocol_codec;
mod storage_codec;

pub mod codec {
    pub mod auth {
        pub use crate::auth_codec::*;
    }

    pub mod gameplay {
        pub use crate::gameplay_codec::*;
    }

    pub mod protocol {
        pub use crate::protocol_codec::*;
    }

    pub mod storage {
        pub use crate::storage_codec::*;
    }
}

pub use abi::*;
pub use host_api::*;
pub use manifest::*;

pub use auth_codec::{
    AuthDescriptor, AuthMode, AuthRequest, AuthResponse, BedrockAuthResult, decode_auth_request,
    decode_auth_response, encode_auth_request, encode_auth_response,
};
pub use gameplay_codec::{
    GameplayDescriptor, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
    decode_gameplay_request, decode_gameplay_response, decode_host_block_pos_blob,
    decode_host_block_state_blob, decode_host_can_edit_block_key, decode_host_player_id_blob,
    decode_host_player_snapshot_blob, decode_host_world_meta_blob, encode_gameplay_request,
    encode_gameplay_response, encode_host_block_pos_blob, encode_host_block_state_blob,
    encode_host_can_edit_block_key, encode_host_player_id_blob, encode_host_player_snapshot_blob,
    encode_host_world_meta_blob,
};
pub use protocol_codec::{
    PLUGIN_ENVELOPE_HEADER_LEN, PROTOCOL_FLAG_RESPONSE, ProtocolCodecError, ProtocolOpCode,
    ProtocolRequest, ProtocolResponse, ProtocolSessionSnapshot, WireFrameDecodeResult,
    decode_protocol_request, decode_protocol_response, encode_protocol_request,
    encode_protocol_response,
};
pub use storage_codec::{
    StorageDescriptor, StorageRequest, StorageResponse, decode_storage_request,
    decode_storage_response, encode_storage_request, encode_storage_response,
};
