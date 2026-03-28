use mc_plugin_api::codec::admin_surface::AdminSurfaceResource;
use revy_server_runtime::RuntimeError;
use tokio::sync::oneshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PausedAdminSurfaceResource {
    pub(crate) name: String,
    pub(crate) resource: AdminSurfaceResource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PausedAdminSurfaceInstance {
    pub(crate) instance_id: String,
    pub(crate) resume_payload: Vec<u8>,
    pub(crate) handoff_resources: Vec<PausedAdminSurfaceResource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PausedProcessSurfaces {
    pub(crate) admin_surfaces: Vec<PausedAdminSurfaceInstance>,
}

pub(crate) enum ProcessSurfaceCommand {
    PauseForUpgrade {
        ack_tx: oneshot::Sender<Result<PausedProcessSurfaces, RuntimeError>>,
    },
    ResumeAfterUpgradeRollback {
        paused: PausedProcessSurfaces,
        ack_tx: oneshot::Sender<Result<(), RuntimeError>>,
    },
    ReconcileAdminSurfaces,
}
