use crate::RuntimeError;
use crate::runtime::RuntimeServer;

impl RuntimeServer {
    pub(in crate::runtime::session) async fn resolve_gameplay_for_adapter(
        &self,
        adapter_id: &str,
    ) -> Result<std::sync::Arc<dyn mc_plugin_host::runtime::GameplayProfileHandle>, RuntimeError>
    {
        let epoch = self.authority.active();
        let profile_id = super::super::selection::SelectionResolver::gameplay_profile_for_adapter(
            &epoch.selection.config,
            adapter_id,
        );
        epoch
            .selection
            .loaded_plugins
            .resolve_gameplay_profile(profile_id.as_str())
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "gameplay profile `{}` for adapter `{adapter_id}` is not active",
                    profile_id.as_str()
                ))
            })
    }

    pub(in crate::runtime::session) async fn resolve_bedrock_auth_profile(
        &self,
    ) -> Result<std::sync::Arc<dyn mc_plugin_host::runtime::AuthProfileHandle>, RuntimeError> {
        self.authority
            .active()
            .selection
            .bedrock_auth_profile
            .clone()
            .ok_or_else(|| RuntimeError::Config("bedrock auth profile is not active".to_string()))
    }
}
