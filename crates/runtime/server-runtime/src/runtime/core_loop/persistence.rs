use crate::RuntimeError;
use crate::runtime::RuntimeServer;
use std::sync::Arc;

impl RuntimeServer {
    pub(in crate::runtime) async fn maybe_save(&self) -> Result<(), RuntimeError> {
        self.kernel
            .maybe_save(self.reload.reload_host().map(Arc::as_ref))
            .await
    }
}
