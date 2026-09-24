#![allow(clippy::multiple_crate_versions)]
mod adapter;
mod handshake;
mod login;
mod status;

#[doc(hidden)]
pub mod __version_support;

pub use self::adapter::{JavaEditionAdapter, JavaEditionProfile, JavaProtocolSessionStore};
pub use self::status::format_text_component;

/// Projects the runtime admission limit into the unsigned-byte field carried by legacy Join Game
/// packets. The wire field is informational and cannot represent the runtime's full population.
#[must_use]
pub fn join_game_player_limit(max_players: u32) -> u8 {
    u8::try_from(max_players).unwrap_or(u8::MAX)
}
