#![allow(dead_code, clippy::multiple_crate_versions)]

mod error;

pub mod config;
pub mod host {
    pub use crate::plugin_host::{
        AuthGeneration, AuthPluginStatusSnapshot, GameplayGeneration, GameplayPluginStatusSnapshot,
        HotSwappableAuthProfile, HotSwappableGameplayProfile, HotSwappableStorageProfile,
        InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin,
        InProcessStoragePlugin, PluginAbiRange, PluginArtifactStatusSnapshot, PluginCatalog,
        PluginFailureAction, PluginFailureMatrix, PluginHost, PluginHostStatusSnapshot,
        ProtocolPluginStatusSnapshot, StoragePluginStatusSnapshot, plugin_host_from_config,
        plugin_reload_poll_interval_ms,
    };
}
pub mod registry;
pub mod runtime;

pub use self::error::PluginHostError;
#[cfg(test)]
pub(crate) use self::error::PluginHostError as RuntimeError;

#[cfg(test)]
#[path = "../../../runtime/server-runtime/src/test_harness.rs"]
mod test_harness;

mod plugin_host;
#[cfg(test)]
pub(crate) use self::test_harness::*;
