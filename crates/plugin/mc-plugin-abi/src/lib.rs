#![allow(clippy::multiple_crate_versions)]

pub mod host;
pub mod manifest;
pub mod raw;

pub use mc_plugin_contract::plugin::{CURRENT_PLUGIN_ABI, PluginAbiVersion};
