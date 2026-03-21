#![allow(clippy::multiple_crate_versions)]

mod error;

pub mod config;
pub mod host {
    pub use crate::plugin_host::{
        AuthPluginStatusSnapshot, GameplayPluginStatusSnapshot, PluginAbiRange,
        PluginArtifactStatusSnapshot, PluginFailureAction, PluginFailureMatrix, PluginHost,
        PluginHostStatusSnapshot, ProtocolPluginStatusSnapshot, StoragePluginStatusSnapshot,
        plugin_host_from_config, plugin_reload_poll_interval_ms,
    };
}
pub mod registry;
pub mod runtime;
#[cfg(any(test, feature = "in-process-testing"))]
mod test_support;

#[cfg(feature = "in-process-testing")]
#[doc(hidden)]
pub mod __test_support_internal {
    pub use crate::test_support::{
        InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin,
        InProcessStoragePlugin, TestPluginHost, TestPluginHostBuilder,
    };
}

pub use self::error::PluginHostError;

mod plugin_host;
