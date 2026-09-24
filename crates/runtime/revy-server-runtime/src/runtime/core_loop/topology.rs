use crate::ListenerBinding;
use crate::RuntimeError;
use crate::runtime::{GenerationAdmission, GenerationId, RuntimeServer, SessionControl};

impl RuntimeServer {
    pub(in crate::runtime) fn listener_bindings(&self) -> Vec<ListenerBinding> {
        self.authority.active().topology.listener_bindings.clone()
    }

    pub(in crate::runtime) fn active_generation_id(&self) -> GenerationId {
        self.authority.active().topology.generation_id
    }

    pub(in crate::runtime) fn generation_admission(
        &self,
        generation_id: GenerationId,
    ) -> GenerationAdmission {
        self.topology_resources
            .generation_admission(&self.authority.active().topology, generation_id)
    }

    pub(in crate::runtime) async fn shutdown_listener_workers(&self) {
        self.topology_resources.shutdown_listener_workers().await;
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

    pub(in crate::runtime) async fn enforce_generation_drains(&self) -> Result<(), RuntimeError> {
        self.topology_resources
            .enforce_generation_drains(&self.sessions)
            .await
    }
}
