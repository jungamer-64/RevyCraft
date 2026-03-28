#![allow(clippy::multiple_crate_versions)]

mod error;

pub mod config;
pub mod host;
pub mod registry;
pub mod runtime;
#[cfg(test)]
mod test_support;

#[cfg(any(test, feature = "in-process-testing"))]
#[doc(hidden)]
pub mod __test_hooks;

pub use self::error::PluginHostError;
