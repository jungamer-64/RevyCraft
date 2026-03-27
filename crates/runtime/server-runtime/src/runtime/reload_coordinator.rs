use crate::RuntimeError;
use crate::config::{ServerConfigSource, StaticConfig};
use crate::{RuntimeUpgradePhase, RuntimeUpgradeRole, RuntimeUpgradeStateView};
use mc_plugin_host::runtime::RuntimePluginHost;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{
    Mutex as AsyncMutex, OwnedMutexGuard, OwnedRwLockWriteGuard, RwLock as AsyncRwLock, oneshot,
};

pub(crate) struct ReloadCoordinator {
    static_config: StaticConfig,
    config_source: ServerConfigSource,
    reload_host: Option<Arc<dyn RuntimePluginHost>>,
    reload_serial: Arc<AsyncMutex<()>>,
    consistency_gate: Arc<AsyncRwLock<()>>,
    shutting_down: AtomicBool,
    shutdown_tx: std::sync::Mutex<Option<oneshot::Sender<()>>>,
    upgrade_state: std::sync::Mutex<Option<RuntimeUpgradeStateView>>,
    child_upgrade_serial_hold: std::sync::Mutex<Option<OwnedMutexGuard<()>>>,
    child_upgrade_commit_hold: std::sync::Mutex<Option<OwnedRwLockWriteGuard<()>>>,
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
            consistency_gate: Arc::new(AsyncRwLock::new(())),
            shutting_down: AtomicBool::new(false),
            shutdown_tx: std::sync::Mutex::new(None),
            upgrade_state: std::sync::Mutex::new(None),
            child_upgrade_serial_hold: std::sync::Mutex::new(None),
            child_upgrade_commit_hold: std::sync::Mutex::new(None),
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

    pub(crate) async fn write_consistency_owned(&self) -> OwnedRwLockWriteGuard<()> {
        Arc::clone(&self.consistency_gate).write_owned().await
    }

    pub(crate) async fn lock_reload_serial(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.reload_serial.lock().await
    }

    pub(crate) fn try_lock_reload_serial(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.reload_serial.try_lock().ok()
    }

    pub(crate) async fn lock_reload_serial_owned(&self) -> OwnedMutexGuard<()> {
        Arc::clone(&self.reload_serial).lock_owned().await
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

    pub(crate) fn current_upgrade_state(&self) -> Option<RuntimeUpgradeStateView> {
        *self
            .upgrade_state
            .lock()
            .expect("upgrade state mutex should not be poisoned")
    }

    pub(crate) fn set_upgrade_state(&self, role: RuntimeUpgradeRole, phase: RuntimeUpgradePhase) {
        *self
            .upgrade_state
            .lock()
            .expect("upgrade state mutex should not be poisoned") =
            Some(RuntimeUpgradeStateView { role, phase });
    }

    pub(crate) fn clear_upgrade_state(&self) {
        *self
            .upgrade_state
            .lock()
            .expect("upgrade state mutex should not be poisoned") = None;
    }

    pub(crate) fn install_child_upgrade_commit_hold(&self, hold: OwnedRwLockWriteGuard<()>) {
        *self
            .child_upgrade_commit_hold
            .lock()
            .expect("child upgrade hold mutex should not be poisoned") = Some(hold);
    }

    pub(crate) fn install_child_upgrade_serial_hold(&self, hold: OwnedMutexGuard<()>) {
        *self
            .child_upgrade_serial_hold
            .lock()
            .expect("child upgrade serial mutex should not be poisoned") = Some(hold);
    }

    pub(crate) fn release_child_upgrade_commit_hold(&self) {
        let _ = self
            .child_upgrade_commit_hold
            .lock()
            .expect("child upgrade hold mutex should not be poisoned")
            .take();
    }

    pub(crate) fn release_child_upgrade_serial_hold(&self) {
        let _ = self
            .child_upgrade_serial_hold
            .lock()
            .expect("child upgrade serial mutex should not be poisoned")
            .take();
    }

    pub(crate) fn reject_mutating_admin_action_during_upgrade(
        &self,
        action: &str,
    ) -> Result<(), RuntimeError> {
        let Some(state) = self.current_upgrade_state() else {
            return Ok(());
        };
        Err(RuntimeError::Config(format!(
            "admin action `{action}` is unavailable during runtime upgrade: role={:?} phase={:?}",
            state.role, state.phase
        )))
    }
}
