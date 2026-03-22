use super::{
    AdminRequest, AdminResponse, AdminUiGeneration, Arc, CapabilitySet, PluginFailureAction,
    PluginFailureDispatch, PluginGenerationId, PluginKind, ReloadableGenerationSlot, RuntimeError,
};

pub(crate) struct HotSwappableAdminUiProfile {
    plugin_id: String,
    profile_id: String,
    generation: ReloadableGenerationSlot<AdminUiGeneration>,
    failures: Arc<PluginFailureDispatch>,
}

impl HotSwappableAdminUiProfile {
    pub(crate) const fn new(
        plugin_id: String,
        profile_id: String,
        generation: Arc<AdminUiGeneration>,
        failures: Arc<PluginFailureDispatch>,
    ) -> Self {
        Self {
            plugin_id,
            profile_id,
            generation: ReloadableGenerationSlot::new(
                generation,
                "admin-ui generation lock should not be poisoned",
                "admin-ui reload gate should not be poisoned",
            ),
            failures,
        }
    }

    pub(crate) fn current_generation(&self) -> Arc<AdminUiGeneration> {
        self.generation.current()
    }

    pub(crate) fn swap_generation(&self, generation: Arc<AdminUiGeneration>) {
        self.generation.swap(generation);
    }

    fn profile_id(&self) -> &str {
        &self.profile_id
    }

    fn capability_set(&self) -> CapabilitySet {
        self.generation.capability_set()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Some(self.generation.generation_id())
    }

    fn handle_error<T>(&self, result: Result<T, String>) -> Result<T, RuntimeError> {
        match result {
            Ok(value) => Ok(value),
            Err(message) => {
                match self.failures.handle_runtime_failure(
                    PluginKind::AdminUi,
                    &self.plugin_id,
                    &message,
                ) {
                    PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                        Err(RuntimeError::Config(message))
                    }
                    PluginFailureAction::FailFast => Err(RuntimeError::PluginFatal(message)),
                }
            }
        }
    }

    fn parse_line(&self, line: &str) -> Result<AdminRequest, RuntimeError> {
        self.generation
            .with_reload_read(|generation| self.handle_error(generation.parse_line(line)))
    }

    fn render_response(&self, response: &AdminResponse) -> Result<String, RuntimeError> {
        self.generation
            .with_reload_read(|generation| self.handle_error(generation.render_response(response)))
    }
}

impl crate::runtime::AdminUiProfileHandle for HotSwappableAdminUiProfile {
    fn profile_id(&self) -> &str {
        Self::profile_id(self)
    }

    fn capability_set(&self) -> CapabilitySet {
        Self::capability_set(self)
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Self::plugin_generation_id(self)
    }

    fn parse_line(&self, line: &str) -> Result<AdminRequest, RuntimeError> {
        Self::parse_line(self, line)
    }

    fn render_response(&self, response: &AdminResponse) -> Result<String, RuntimeError> {
        Self::render_response(self, response)
    }
}
