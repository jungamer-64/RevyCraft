use crate::config::{ServerConfigSource, StaticConfig};
use mc_plugin_host::runtime::RuntimePluginHost;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{RwLock as AsyncRwLock, oneshot};

pub(crate) struct ReloadCoordinator {
    static_config: StaticConfig,
    config_source: ServerConfigSource,
    reload_host: Option<Arc<dyn RuntimePluginHost>>,
    consistency_gate: AsyncRwLock<()>,
    shutting_down: AtomicBool,
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
            consistency_gate: AsyncRwLock::new(()),
            shutting_down: AtomicBool::new(false),
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

    pub(crate) fn admin_grpc_bind_addr(&self) -> Option<SocketAddr> {
        self.static_config
            .admin_grpc
            .enabled
            .then_some(self.static_config.admin_grpc.bind_addr)
    }

    pub(crate) async fn read_consistency(&self) -> tokio::sync::RwLockReadGuard<'_, ()> {
        self.consistency_gate.read().await
    }

    pub(crate) async fn write_consistency(&self) -> tokio::sync::RwLockWriteGuard<'_, ()> {
        self.consistency_gate.write().await
    }

    pub(crate) fn install_shutdown_tx(&self, shutdown_tx: oneshot::Sender<()>) {
        *self
            .shutdown_tx
            .lock()
            .expect("shutdown mutex should not be poisoned") = Some(shutdown_tx);
    }

    pub(crate) fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    pub(crate) fn mark_shutting_down(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
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
