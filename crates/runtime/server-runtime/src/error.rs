use mc_proto_common::ProtocolError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("plugin load error: {0}")]
    PluginLoad(#[from] libloading::Error),
    #[error("fatal plugin failure: {0}")]
    PluginFatal(String),
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

impl From<mc_plugin_host::PluginHostError> for RuntimeError {
    fn from(value: mc_plugin_host::PluginHostError) -> Self {
        match value {
            mc_plugin_host::PluginHostError::Io(error) => Self::Io(error),
            mc_plugin_host::PluginHostError::PluginLoad(error) => Self::PluginLoad(error),
            mc_plugin_host::PluginHostError::PluginFatal(message) => Self::PluginFatal(message),
            mc_plugin_host::PluginHostError::Protocol(error) => Self::Protocol(error),
            mc_plugin_host::PluginHostError::Storage(error) => Self::Storage(error),
            mc_plugin_host::PluginHostError::Auth(message) => Self::Auth(message),
            mc_plugin_host::PluginHostError::Unsupported(message) => Self::Unsupported(message),
            mc_plugin_host::PluginHostError::Config(message) => Self::Config(message),
        }
    }
}
