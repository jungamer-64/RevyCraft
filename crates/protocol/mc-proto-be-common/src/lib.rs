#![allow(clippy::multiple_crate_versions)]
mod adapter;
mod login;
mod probe;
mod wire;

#[cfg(test)]
mod tests;

mod world;

#[doc(hidden)]
pub mod __version_support;

pub use self::adapter::{BedrockAdapter, BedrockProfile};
pub use self::wire::{
    BEDROCK_GAME_PACKET_ID, BEDROCK_RAKNET_MAGIC, BedrockCompression, BedrockWireError,
    decode_packet_batch, encode_packet_batch,
};
