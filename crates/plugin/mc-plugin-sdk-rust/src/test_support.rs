use mc_plugin_api::host_api::{
    AuthPluginApiV1, GameplayPluginApiV1, ProtocolPluginApiV1, StoragePluginApiV1,
};
use mc_plugin_api::manifest::PluginManifestV1;

#[derive(Clone, Copy)]
pub struct InProcessProtocolEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub api: &'static ProtocolPluginApiV1,
}

#[derive(Clone, Copy)]
pub struct InProcessGameplayEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub api: &'static GameplayPluginApiV1,
}

#[derive(Clone, Copy)]
pub struct InProcessStorageEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub api: &'static StoragePluginApiV1,
}

#[derive(Clone, Copy)]
pub struct InProcessAuthEntrypoints {
    pub manifest: &'static PluginManifestV1,
    pub api: &'static AuthPluginApiV1,
}
