use crate::config::{ServerConfigSource, StaticConfig};
use mc_plugin_host::runtime::RuntimePluginHost;
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, oneshot, watch};

pub(crate) struct ReloadCoordinator {
    static_config: StaticConfig,
    config_source: ServerConfigSource,
    reload_host: Option<Arc<dyn RuntimePluginHost>>,
    reload_serial: Arc<AsyncMutex<()>>,
    shutdown_requested_tx: watch::Sender<bool>,
    shutdown_tx: std::sync::Mutex<Option<oneshot::Sender<()>>>,
}

impl ReloadCoordinator {
    pub(crate) fn new(
        static_config: StaticConfig,
        config_source: ServerConfigSource,
        reload_host: Option<Arc<dyn RuntimePluginHost>>,
    ) -> Self {
        Self {
            static_config,
            config_source,
            reload_host,
            reload_serial: Arc::new(AsyncMutex::new(())),
            shutdown_requested_tx: watch::channel(false).0,
            shutdown_tx: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn static_config(&self) -> &StaticConfig {
        &self.static_config
    }

    pub(crate) fn config_source(&self) -> &ServerConfigSource {
        &self.config_source
    }

    pub(crate) fn reload_host(&self) -> Option<&Arc<dyn RuntimePluginHost>> {
        self.reload_host.as_ref()
    }

    pub(crate) async fn lock_reload_serial(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.reload_serial.lock().await
    }

    pub(crate) async fn lock_reload_serial_owned(&self) -> tokio::sync::OwnedMutexGuard<()> {
        Arc::clone(&self.reload_serial).lock_owned().await
    }

    pub(crate) fn try_lock_reload_serial(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.reload_serial.try_lock().ok()
    }

    pub(crate) fn install_shutdown_tx(&self, shutdown_tx: oneshot::Sender<()>) {
        *self
            .shutdown_tx
            .lock()
            .expect("shutdown mutex should not be poisoned") = Some(shutdown_tx);
    }

    pub(crate) fn is_shutting_down(&self) -> bool {
        *self.shutdown_requested_tx.borrow()
    }

    pub(crate) fn mark_shutting_down(&self) {
        self.shutdown_requested_tx.send_replace(true);
    }

    pub(crate) fn subscribe_shutdown_requested(&self) -> watch::Receiver<bool> {
        self.shutdown_requested_tx.subscribe()
    }

    pub(crate) fn request_shutdown(&self) -> bool {
        self.mark_shutting_down();
        self.shutdown_tx
            .lock()
            .expect("shutdown mutex should not be poisoned")
            .take()
            .is_some_and(|shutdown_tx| shutdown_tx.send(()).is_ok())
    }
}
