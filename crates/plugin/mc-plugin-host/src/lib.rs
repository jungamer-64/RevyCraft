#![allow(dead_code, clippy::multiple_crate_versions)]

mod error;

pub mod config;
pub mod runtime;

pub use self::error::PluginHostError;
pub use self::error::PluginHostError as RuntimeError;

#[cfg(test)]
#[path = "../../../runtime/server-runtime/src/test_harness.rs"]
mod test_harness;

#[cfg(test)]
pub(crate) use self::test_harness::*;

pub mod registry;

#[path = "../../../runtime/server-runtime/src/plugin_host.rs"]
mod plugin_host;

pub mod host {
    pub use crate::plugin_host::{
        AuthGeneration, AuthPluginStatusSnapshot, GameplayGeneration,
        GameplayPluginStatusSnapshot, HotSwappableAuthProfile, HotSwappableGameplayProfile,
        HotSwappableStorageProfile, InProcessAuthPlugin, InProcessGameplayPlugin,
        InProcessProtocolPlugin, InProcessStoragePlugin, PluginAbiRange,
        PluginArtifactStatusSnapshot, PluginCatalog, PluginFailureAction, PluginFailureMatrix,
        PluginHost, PluginHostStatusSnapshot, ProtocolPluginStatusSnapshot,
        StoragePluginStatusSnapshot, plugin_host_from_config, plugin_reload_poll_interval_ms,
    };
}

pub use self::host::*;
