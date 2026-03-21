//! Shared reusable in-process plugin-host fixtures for workspace tests.
//!
//! This crate is the sanctioned test/dev surface for building `mc-plugin-host`
//! fixtures outside the host crate itself. Packaged-plugin harness helpers live
//! in `mc-plugin-test-support`.

pub use mc_plugin_host::__test_support_internal::{TestPluginHost, TestPluginHostBuilder};
pub use mc_plugin_host::host::{PluginAbiRange, PluginFailureAction, PluginFailureMatrix};

pub mod raw {
    pub use mc_plugin_host::__test_support_internal::{
        InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin,
        InProcessStoragePlugin,
    };
}
