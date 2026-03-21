#![allow(clippy::multiple_crate_versions)]
mod adapter;
mod login;
mod probe;

#[cfg(test)]
mod tests;

mod world;

#[doc(hidden)]
pub mod __version_support;

pub use self::adapter::{BedrockAdapter, BedrockProfile};
