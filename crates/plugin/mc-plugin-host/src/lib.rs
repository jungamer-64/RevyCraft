#![allow(clippy::multiple_crate_versions)]

mod error;

pub mod config;
pub mod host {
    pub use crate::plugin_host::{
        AdminTransportPluginStatusSnapshot, AdminUiPluginStatusSnapshot, AuthPluginStatusSnapshot,
        GameplayPluginStatusSnapshot, PluginAbiRange, PluginArtifactStatusSnapshot,
        PluginFailureAction, PluginFailureMatrix, PluginHost, PluginHostStatusSnapshot,
        ProtocolPluginStatusSnapshot, StoragePluginStatusSnapshot, plugin_host_from_config,
        plugin_reload_poll_interval_ms,
    };
}
pub mod registry;
pub mod runtime;
#[cfg(test)]
mod test_support;

#[cfg(any(test, feature = "in-process-testing"))]
#[doc(hidden)]
pub mod __test_hooks;

pub use self::error::PluginHostError;

mod plugin_host;
