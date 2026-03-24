#![allow(clippy::multiple_crate_versions)]
mod adapter;
mod handshake;
mod login;
mod status;

#[doc(hidden)]
pub mod __version_support;

pub use self::adapter::{JavaEditionAdapter, JavaEditionProfile};
pub use self::status::format_text_component;
