#![allow(clippy::multiple_crate_versions)]
use mc_core::ReadonlyGameplayPolicy;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::gameplay::PolicyGameplayPlugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;

#[derive(Default)]
pub struct ReadonlyGameplayPlugin;

impl PolicyGameplayPlugin for ReadonlyGameplayPlugin {
    type Policy = ReadonlyGameplayPolicy;

    const PROFILE_ID: &'static str = "readonly";
    const EXPORT_TAG: &'static str = "readonly";
    const IMPORT_REJECT_MESSAGE: &'static str = "readonly gameplay plugin refused session import";

    fn capability_names() -> &'static [&'static str] {
        &["gameplay.profile.readonly", "runtime.reload.gameplay"]
    }
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::gameplay(
    "gameplay-readonly",
    "Readonly Gameplay Plugin",
    &["gameplay.profile:readonly", "runtime.reload.gameplay"],
);

export_plugin!(gameplay, ReadonlyGameplayPlugin, MANIFEST);
