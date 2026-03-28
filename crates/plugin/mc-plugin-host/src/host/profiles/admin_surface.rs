use super::{
    AdminSurfaceCapabilitySet, AdminSurfaceGeneration, AdminSurfaceHostApiV1,
    AdminSurfaceInstanceDeclaration, AdminSurfacePauseView, AdminSurfaceProfileId,
    AdminSurfaceStatusView, Arc, Path, PluginFailureAction, PluginFailureDispatch,
    PluginGenerationId, PluginKind, ReloadableGenerationSlot, RuntimeError,
};

pub(crate) struct HotSwappableAdminSurfaceProfile {
    plugin_id: String,
    profile_id: AdminSurfaceProfileId,
    generation: ReloadableGenerationSlot<AdminSurfaceGeneration>,
    failures: Arc<PluginFailureDispatch>,
}

impl HotSwappableAdminSurfaceProfile {
    pub(crate) const fn new(
        plugin_id: String,
        profile_id: AdminSurfaceProfileId,
        generation: Arc<AdminSurfaceGeneration>,
        failures: Arc<PluginFailureDispatch>,
    ) -> Self {
        Self {
            plugin_id,
            profile_id,
            generation: ReloadableGenerationSlot::new(
                generation,
                "admin-surface generation lock should not be poisoned",
                "admin-surface reload gate should not be poisoned",
            ),
            failures,
        }
    }

    pub(crate) fn current_generation(&self) -> Arc<AdminSurfaceGeneration> {
        self.generation.current()
    }

    pub(crate) fn swap_generation(&self, generation: Arc<AdminSurfaceGeneration>) {
        self.generation.swap(generation);
    }

    fn profile_id(&self) -> &AdminSurfaceProfileId {
        &self.profile_id
    }

    fn capability_set(&self) -> AdminSurfaceCapabilitySet {
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
                    PluginKind::AdminSurface,
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

    fn declare_instance(
        &self,
        instance_id: &str,
        surface_config_path: Option<&Path>,
    ) -> Result<AdminSurfaceInstanceDeclaration, RuntimeError> {
        self.generation.with_reload_read(|generation| {
            self.handle_error(generation.declare_instance(instance_id, surface_config_path))
        })
    }

    fn start(
        &self,
        instance_id: &str,
        surface_config_path: Option<&Path>,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, RuntimeError> {
        self.generation.with_reload_read(|generation| {
            self.handle_error(generation.start(instance_id, surface_config_path, host_api))
        })
    }

    fn pause_for_upgrade(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfacePauseView, RuntimeError> {
        self.generation.with_reload_read(|generation| {
            self.handle_error(generation.pause_for_upgrade(instance_id, host_api))
        })
    }

    fn resume_from_upgrade(
        &self,
        instance_id: &str,
        surface_config_path: Option<&Path>,
        resume_payload: &[u8],
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, RuntimeError> {
        self.generation.with_reload_read(|generation| {
            self.handle_error(generation.resume_from_upgrade(
                instance_id,
                surface_config_path,
                resume_payload,
                host_api,
            ))
        })
    }

    fn activate_after_upgrade_commit(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<(), RuntimeError> {
        self.generation.with_reload_read(|generation| {
            self.handle_error(generation.activate_after_upgrade_commit(instance_id, host_api))
        })
    }

    fn resume_after_upgrade_rollback(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, RuntimeError> {
        self.generation.with_reload_read(|generation| {
            self.handle_error(generation.resume_after_upgrade_rollback(instance_id, host_api))
        })
    }

    fn shutdown(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<(), RuntimeError> {
        self.generation.with_reload_read(|generation| {
            self.handle_error(generation.shutdown(instance_id, host_api))
        })
    }
}

impl crate::runtime::AdminSurfaceProfileHandle for HotSwappableAdminSurfaceProfile {
    fn profile_id(&self) -> &AdminSurfaceProfileId {
        Self::profile_id(self)
    }

    fn capability_set(&self) -> AdminSurfaceCapabilitySet {
        Self::capability_set(self)
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Self::plugin_generation_id(self)
    }

    fn declare_instance(
        &self,
        instance_id: &str,
        surface_config_path: Option<&Path>,
    ) -> Result<AdminSurfaceInstanceDeclaration, crate::PluginHostError> {
        Self::declare_instance(self, instance_id, surface_config_path)
    }

    fn start(
        &self,
        instance_id: &str,
        surface_config_path: Option<&Path>,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, crate::PluginHostError> {
        Self::start(self, instance_id, surface_config_path, host_api)
    }

    fn pause_for_upgrade(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfacePauseView, crate::PluginHostError> {
        Self::pause_for_upgrade(self, instance_id, host_api)
    }

    fn resume_from_upgrade(
        &self,
        instance_id: &str,
        surface_config_path: Option<&Path>,
        resume_payload: &[u8],
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, crate::PluginHostError> {
        Self::resume_from_upgrade(
            self,
            instance_id,
            surface_config_path,
            resume_payload,
            host_api,
        )
    }

    fn activate_after_upgrade_commit(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<(), crate::PluginHostError> {
        Self::activate_after_upgrade_commit(self, instance_id, host_api)
    }

    fn resume_after_upgrade_rollback(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, crate::PluginHostError> {
        Self::resume_after_upgrade_rollback(self, instance_id, host_api)
    }

    fn shutdown(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<(), crate::PluginHostError> {
        Self::shutdown(self, instance_id, host_api)
    }
}
