use revy_voxel_core::CoreEvent;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub(crate) struct SessionHandle {
    pub(crate) tx: mpsc::Sender<SessionMessage>,
    pub(crate) control_tx: mpsc::Sender<SessionControl>,
}

#[derive(Clone)]
pub(crate) struct SessionRecipient {
    pub(crate) tx: mpsc::Sender<SessionMessage>,
    pub(crate) control_tx: mpsc::Sender<SessionControl>,
}

#[derive(Clone, Debug)]
pub(crate) enum SessionMessage {
    Event(Arc<CoreEvent>),
    Terminate { reason: String },
}

pub(crate) enum SessionControl {
    Terminate { reason: String },
}
