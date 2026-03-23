#![allow(clippy::multiple_crate_versions)]
use mc_core::{CanonicalGameplayPolicy, GameplayCapability};
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::gameplay::PolicyGameplayPlugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;

#[derive(Default)]
pub struct CanonicalGameplayPlugin;

impl PolicyGameplayPlugin for CanonicalGameplayPlugin {
    type Policy = CanonicalGameplayPolicy;

    const PROFILE_ID: &'static str = "canonical";
    const EXPORT_TAG: &'static str = "canonical";
    const IMPORT_REJECT_MESSAGE: &'static str = "canonical gameplay plugin refused session import";

    fn capabilities() -> &'static [GameplayCapability] {
        &[GameplayCapability::RuntimeReload]
    }
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::gameplay(
    "gameplay-canonical",
    "Canonical Gameplay Plugin",
    "canonical",
);

export_plugin!(gameplay, CanonicalGameplayPlugin, MANIFEST);
