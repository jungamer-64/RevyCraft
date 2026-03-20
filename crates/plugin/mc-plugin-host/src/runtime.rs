use mc_core::WorldSnapshot;
use mc_plugin_api::{GameplaySessionSnapshot, ProtocolSessionSnapshot};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolReloadSession {
    pub adapter_id: String,
    pub session: ProtocolSessionSnapshot,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeReloadContext {
    pub protocol_sessions: Vec<ProtocolReloadSession>,
    pub gameplay_sessions: Vec<GameplaySessionSnapshot>,
    pub snapshot: WorldSnapshot,
    pub world_dir: PathBuf,
}
