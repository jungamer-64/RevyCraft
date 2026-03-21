use crate::RuntimeError;
use crate::runtime::RuntimeServer;
use mc_plugin_api::abi::PluginKind;
use mc_plugin_host::host::PluginFailureAction;

impl RuntimeServer {
    pub(in crate::runtime) async fn maybe_save(&self) -> Result<(), RuntimeError> {
        let snapshot = {
            let state = self.state.lock().await;
            if !state.dirty {
                return Ok(());
            }
            state.core.snapshot()
        };
        match self
            .storage_profile
            .save_snapshot(&self.config.world_dir, &snapshot)
        {
            Ok(()) => {
                let mut state = self.state.lock().await;
                state.dirty = false;
                Ok(())
            }
            Err(mc_proto_common::StorageError::Plugin(message)) => {
                let action = self.reload_host.as_ref().map_or(
                    PluginFailureAction::FailFast,
                    |reload_host| {
                        reload_host.handle_runtime_failure(
                            PluginKind::Storage,
                            self.storage_profile.plugin_id(),
                            &message,
                        )
                    },
                );
                let mut state = self.state.lock().await;
                state.dirty = true;
                match action {
                    PluginFailureAction::Skip => {
                        eprintln!(
                            "storage runtime failure for `{}` skipped: {message}",
                            self.storage_profile.plugin_id()
                        );
                        Ok(())
                    }
                    PluginFailureAction::FailFast => Err(RuntimeError::PluginFatal(format!(
                        "storage plugin `{}` failed during runtime: {message}",
                        self.storage_profile.plugin_id()
                    ))),
                    PluginFailureAction::Quarantine => Err(RuntimeError::Storage(
                        mc_proto_common::StorageError::Plugin(message),
                    )),
                }
            }
            Err(error) => Err(RuntimeError::Storage(error)),
        }
    }
}
