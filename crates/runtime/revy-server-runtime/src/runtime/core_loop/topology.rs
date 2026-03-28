use crate::ListenerBinding;
use crate::RuntimeError;
#[cfg(test)]
use crate::runtime::ActiveGeneration;
use crate::runtime::{
    GenerationAdmission, GenerationId, RuntimeServer, SessionControl, TopologyReloadResult,
};
use mc_plugin_host::runtime::RuntimePluginHost;
#[cfg(test)]
use std::sync::Arc;

impl RuntimeServer {
    pub(in crate::runtime) fn listener_bindings(&self) -> Vec<ListenerBinding> {
        self.topology.listener_bindings()
    }

    #[cfg(test)]
    pub(in crate::runtime) fn active_generation(&self) -> Arc<ActiveGeneration> {
        self.topology.active_generation()
    }

    pub(in crate::runtime) fn active_generation_id(&self) -> GenerationId {
        self.topology.active_generation_id()
    }

    #[cfg(test)]
    pub(in crate::runtime) fn generation(
        &self,
        generation_id: GenerationId,
    ) -> Option<Arc<ActiveGeneration>> {
        self.topology.generation(generation_id)
    }

    pub(in crate::runtime) fn generation_admission(
        &self,
        generation_id: GenerationId,
    ) -> GenerationAdmission {
        self.topology.generation_admission(generation_id)
    }

    pub(in crate::runtime) async fn shutdown_listener_workers(&self) {
        self.topology.shutdown_listener_workers().await;
    }

    pub(in crate::runtime) async fn terminate_all_sessions(&self, reason: &str) {
        let handles = self.sessions.all_handles().await;
        for handle in handles {
            let _ = handle
                .control_tx
                .send(SessionControl::Terminate {
                    reason: reason.to_string(),
                })
                .await;
        }
    }

    pub(in crate::runtime) async fn reload_generation_with_config(
        &self,
        reload_host: &dyn RuntimePluginHost,
        candidate_config: crate::config::ServerConfig,
        force_generation: bool,
    ) -> Result<TopologyReloadResult, RuntimeError> {
        let protocol_topology = reload_host.prepare_protocol_topology_for_reload()?;
        let result = self
            .topology
            .reload_generation_with_config(
                candidate_config,
                force_generation,
                &protocol_topology,
                &self.kernel,
                &self.sessions,
            )
            .await?;
        reload_host.activate_protocol_topology(protocol_topology);
        Ok(result)
    }

    pub(in crate::runtime) async fn enforce_generation_drains(&self) -> Result<(), RuntimeError> {
        self.topology
            .enforce_generation_drains(&self.sessions)
            .await
    }

    #[cfg(test)]
    pub(in crate::runtime) async fn retire_drained_generations(&self) -> Vec<GenerationId> {
        self.topology
            .retire_drained_generations(&self.sessions)
            .await
    }
}
