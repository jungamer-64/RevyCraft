use mc_plugin_api::manifest::PluginManifestV1;

#[derive(Clone, Copy)]
pub struct InProcessPluginEntrypoints<Api: 'static> {
    pub manifest: &'static PluginManifestV1,
    pub api: &'static Api,
}

impl<Api: 'static> InProcessPluginEntrypoints<Api> {
    #[must_use]
    pub const fn new(manifest: &'static PluginManifestV1, api: &'static Api) -> Self {
        Self { manifest, api }
    }
}
