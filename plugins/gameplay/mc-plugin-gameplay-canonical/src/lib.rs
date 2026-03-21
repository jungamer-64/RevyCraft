#![allow(clippy::multiple_crate_versions)]
use mc_core::CanonicalGameplayPolicy;
use mc_plugin_sdk_rust::gameplay::{PolicyGameplayPlugin, export_gameplay_plugin};
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;

#[derive(Default)]
pub struct CanonicalGameplayPlugin;

impl PolicyGameplayPlugin for CanonicalGameplayPlugin {
    type Policy = CanonicalGameplayPolicy;

    const PROFILE_ID: &'static str = "canonical";
    const EXPORT_TAG: &'static str = "canonical";
    const IMPORT_REJECT_MESSAGE: &'static str =
        "canonical gameplay plugin refused session import";

    fn capability_names() -> &'static [&'static str] {
        &["gameplay.profile.canonical", "runtime.reload.gameplay"]
    }
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::gameplay(
    "gameplay-canonical",
    "Canonical Gameplay Plugin",
    &["gameplay.profile:canonical", "runtime.reload.gameplay"],
);

export_gameplay_plugin!(CanonicalGameplayPlugin, MANIFEST);
