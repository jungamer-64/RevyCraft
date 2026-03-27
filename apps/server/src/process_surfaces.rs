use server_runtime::RuntimeError;
use tokio::sync::oneshot;

pub(crate) struct PausedProcessSurfaces {
    pub(crate) admin_listener_for_child: Option<std::net::TcpListener>,
    pub(crate) console_was_paused: bool,
    pub(crate) grpc_accept_was_paused: bool,
}

pub(crate) enum ProcessSurfaceCommand {
    PauseForUpgrade {
        skip_console: bool,
        ack_tx: oneshot::Sender<Result<PausedProcessSurfaces, RuntimeError>>,
    },
    ResumeAfterUpgradeRollback {
        paused: PausedProcessSurfaces,
        ack_tx: oneshot::Sender<Result<(), RuntimeError>>,
    },
}

pub(crate) enum ConsoleControl {
    PauseForUpgrade { ack_tx: oneshot::Sender<()> },
}
