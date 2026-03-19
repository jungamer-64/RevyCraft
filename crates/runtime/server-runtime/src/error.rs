use mc_proto_common::ProtocolError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("plugin load error: {0}")]
    PluginLoad(#[from] libloading::Error),
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("storage error: {0}")]
    Storage(#[from] mc_proto_common::StorageError),
    #[error("auth error: {0}")]
    Auth(String),
    #[error("unsupported configuration: {0}")]
    Unsupported(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
}
