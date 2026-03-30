use crate::runtime::RuntimeServer;
use tokio::sync::oneshot;

pub(crate) struct ReloadStagePauseHook {
    pub(crate) reached_tx: Option<oneshot::Sender<()>>,
    pub(crate) release_rx: oneshot::Receiver<()>,
}

pub(crate) struct ReloadStagePauseHandle {
    reached_rx: oneshot::Receiver<()>,
    release_tx: Option<oneshot::Sender<()>>,
}

pub(crate) struct LoginAcceptCommitPauseHook {
    pub(crate) reached_tx: Option<oneshot::Sender<()>>,
    pub(crate) release_rx: oneshot::Receiver<()>,
}

pub(crate) struct LoginAcceptCommitPauseHandle {
    reached_rx: oneshot::Receiver<()>,
    release_tx: Option<oneshot::Sender<()>>,
}

impl RuntimeServer {
    pub(crate) async fn arm_reload_stage_pause_for_test(&self) -> ReloadStagePauseHandle {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self.reload_stage_pause_hook.lock().await = Some(ReloadStagePauseHook {
            reached_tx: Some(reached_tx),
            release_rx,
        });
        ReloadStagePauseHandle {
            reached_rx,
            release_tx: Some(release_tx),
        }
    }

    pub(crate) async fn maybe_pause_after_reload_stage_for_test(&self) {
        let hook = self.reload_stage_pause_hook.lock().await.take();
        let Some(mut hook) = hook else {
            return;
        };
        if let Some(reached_tx) = hook.reached_tx.take() {
            let _ = reached_tx.send(());
        }
        let _ = hook.release_rx.await;
    }

    pub(crate) async fn arm_login_accept_commit_pause_for_test(
        &self,
    ) -> LoginAcceptCommitPauseHandle {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self.login_accept_commit_pause_hook.lock().await = Some(LoginAcceptCommitPauseHook {
            reached_tx: Some(reached_tx),
            release_rx,
        });
        LoginAcceptCommitPauseHandle {
            reached_rx,
            release_tx: Some(release_tx),
        }
    }

    pub(crate) async fn maybe_pause_before_login_accept_commit_for_test(&self) {
        let hook = self.login_accept_commit_pause_hook.lock().await.take();
        let Some(mut hook) = hook else {
            return;
        };
        if let Some(reached_tx) = hook.reached_tx.take() {
            let _ = reached_tx.send(());
        }
        let _ = hook.release_rx.await;
    }
}

impl ReloadStagePauseHandle {
    pub(crate) async fn wait_until_reached(&mut self) {
        let _ = (&mut self.reached_rx).await;
    }

    pub(crate) fn release(mut self) {
        if let Some(release_tx) = self.release_tx.take() {
            let _ = release_tx.send(());
        }
    }
}

impl LoginAcceptCommitPauseHandle {
    pub(crate) async fn wait_until_reached(&mut self) {
        let _ = (&mut self.reached_rx).await;
    }

    pub(crate) fn release(mut self) {
        if let Some(release_tx) = self.release_tx.take() {
            let _ = release_tx.send(());
        }
    }
}
