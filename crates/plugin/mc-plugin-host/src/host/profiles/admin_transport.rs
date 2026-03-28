use super::{
    AdminTransportCapabilitySet, AdminTransportGeneration, AdminTransportHostApiV1,
    AdminTransportPauseView, AdminTransportProfileId, AdminTransportStatusView, Arc, Path,
    PluginFailureAction, PluginFailureDispatch, PluginGenerationId, PluginKind,
    ReloadableGenerationSlot, RuntimeError,
};

pub(crate) struct HotSwappableAdminTransportProfile {
    plugin_id: String,
    profile_id: AdminTransportProfileId,
    generation: ReloadableGenerationSlot<AdminTransportGeneration>,
    failures: Arc<PluginFailureDispatch>,
}

impl HotSwappableAdminTransportProfile {
    pub(crate) const fn new(
        plugin_id: String,
        profile_id: AdminTransportProfileId,
        generation: Arc<AdminTransportGeneration>,
        failures: Arc<PluginFailureDispatch>,
    ) -> Self {
        Self {
            plugin_id,
            profile_id,
            generation: ReloadableGenerationSlot::new(
                generation,
                "admin-transport generation lock should not be poisoned",
                "admin-transport reload gate should not be poisoned",
            ),
            failures,
        }
    }

    pub(crate) fn current_generation(&self) -> Arc<AdminTransportGeneration> {
        self.generation.current()
    }

    pub(crate) fn swap_generation(&self, generation: Arc<AdminTransportGeneration>) {
        self.generation.swap(generation);
    }

    fn profile_id(&self) -> &AdminTransportProfileId {
        &self.profile_id
    }

    fn capability_set(&self) -> AdminTransportCapabilitySet {
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
                    PluginKind::AdminTransport,
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

    fn start(
        &self,
        transport_config_path: &Path,
        host_api: AdminTransportHostApiV1,
    ) -> Result<AdminTransportStatusView, RuntimeError> {
        self.generation.with_reload_read(|generation| {
            self.handle_error(generation.start(transport_config_path, host_api))
        })
    }

    fn pause_for_upgrade(
        &self,
        host_api: AdminTransportHostApiV1,
    ) -> Result<AdminTransportPauseView, RuntimeError> {
        self.generation.with_reload_read(|generation| {
            self.handle_error(generation.pause_for_upgrade(host_api))
        })
    }

    fn resume_from_upgrade(
        &self,
        transport_config_path: &Path,
        resume_payload: &[u8],
        host_api: AdminTransportHostApiV1,
    ) -> Result<AdminTransportStatusView, RuntimeError> {
        self.generation.with_reload_read(|generation| {
            self.handle_error(generation.resume_from_upgrade(
                transport_config_path,
                resume_payload,
                host_api,
            ))
        })
    }

    fn resume_after_upgrade_rollback(
        &self,
        host_api: AdminTransportHostApiV1,
    ) -> Result<AdminTransportStatusView, RuntimeError> {
        self.generation.with_reload_read(|generation| {
            self.handle_error(generation.resume_after_upgrade_rollback(host_api))
        })
    }

    fn shutdown(&self, host_api: AdminTransportHostApiV1) -> Result<(), RuntimeError> {
        self.generation
            .with_reload_read(|generation| self.handle_error(generation.shutdown(host_api)))
    }
}

impl crate::runtime::AdminTransportProfileHandle for HotSwappableAdminTransportProfile {
    fn profile_id(&self) -> &AdminTransportProfileId {
        Self::profile_id(self)
    }

    fn capability_set(&self) -> AdminTransportCapabilitySet {
        Self::capability_set(self)
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Self::plugin_generation_id(self)
    }

    fn start(
        &self,
        transport_config_path: &Path,
        host_api: AdminTransportHostApiV1,
    ) -> Result<AdminTransportStatusView, crate::PluginHostError> {
        Self::start(self, transport_config_path, host_api)
    }

    fn pause_for_upgrade(
        &self,
        host_api: AdminTransportHostApiV1,
    ) -> Result<AdminTransportPauseView, crate::PluginHostError> {
        Self::pause_for_upgrade(self, host_api)
    }

    fn resume_from_upgrade(
        &self,
        transport_config_path: &Path,
        resume_payload: &[u8],
        host_api: AdminTransportHostApiV1,
    ) -> Result<AdminTransportStatusView, crate::PluginHostError> {
        Self::resume_from_upgrade(self, transport_config_path, resume_payload, host_api)
    }

    fn resume_after_upgrade_rollback(
        &self,
        host_api: AdminTransportHostApiV1,
    ) -> Result<AdminTransportStatusView, crate::PluginHostError> {
        Self::resume_after_upgrade_rollback(self, host_api)
    }

    fn shutdown(&self, host_api: AdminTransportHostApiV1) -> Result<(), crate::PluginHostError> {
        Self::shutdown(self, host_api)
    }
}
