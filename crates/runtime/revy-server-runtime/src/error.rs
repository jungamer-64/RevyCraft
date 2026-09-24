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
    Storage(#[from] mc_storage_common::StorageError),
    #[error("core transfer error: {0}")]
    CoreTransfer(#[from] revy_voxel_core::CoreTransferError),
    #[error("cutover event encoding failed: {0}")]
    CutoverEventEncoding(#[source] serde_json::Error),
    #[error("{resource} allocation of {requested} bytes failed")]
    Allocation {
        resource: &'static str,
        requested: usize,
    },
    #[error("auth error: {0}")]
    Auth(String),
    #[error("raknet error: {0}")]
    RakNet(String),
    #[error("socket transfer error: {0}")]
    SocketTransfer(#[from] revy_runtime_transfer::SocketTransferError),
    #[error("unsupported configuration: {0}")]
    Unsupported(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error(
        "cutover {resource} pre-copy at revision {staged_revision} was outpaced; earliest available revision is {earliest_revision}"
    )]
    CutoverOutpaced {
        resource: &'static str,
        staged_revision: u64,
        earliest_revision: u64,
    },
    #[error(
        "cutover session directory changed from prepared revision {prepared_revision} to sealed revision {sealed_revision}"
    )]
    CutoverDirectoryChanged {
        prepared_revision: u64,
        sealed_revision: u64,
    },
    #[error(
        "cutover abort after {cause}; session recovery: {sessions:?}; ingress recovery: {ingress:?}"
    )]
    CutoverAbortFailed {
        cause: Box<RuntimeError>,
        sessions: Option<Box<RuntimeError>>,
        ingress: Option<Box<RuntimeError>>,
    },
    #[error("{resource} budget exceeded: requested {requested}, limit {limit}")]
    BudgetExceeded {
        resource: &'static str,
        requested: usize,
        limit: usize,
    },
    #[error("task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl From<revy_raknet::RakNetError> for RuntimeError {
    fn from(value: revy_raknet::RakNetError) -> Self {
        match value {
            revy_raknet::RakNetError::Budget(error) => Self::BudgetExceeded {
                resource: error.resource,
                requested: error.requested,
                limit: error.limit,
            },
            revy_raknet::RakNetError::Io(error) => Self::Io(error),
            revy_raknet::RakNetError::Wire(message) => Self::RakNet(message),
            revy_raknet::RakNetError::Closed => Self::RakNet("raknet peer is closed".to_string()),
            revy_raknet::RakNetError::PeerTimeout => {
                Self::RakNet("raknet peer activity timed out".to_string())
            }
            revy_raknet::RakNetError::RetransmitExhausted { sequence, attempts } => {
                Self::RakNet(format!(
                    "raknet datagram sequence {sequence} exhausted its retransmit budget after {attempts} attempts"
                ))
            }
            revy_raknet::RakNetError::QueueFull => {
                Self::RakNet("raknet peer command queue is full".to_string())
            }
        }
    }
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

impl From<revy_server_config::ServerConfigError> for RuntimeError {
    fn from(value: revy_server_config::ServerConfigError) -> Self {
        match value {
            revy_server_config::ServerConfigError::Io(error) => Self::Io(error),
            revy_server_config::ServerConfigError::Unsupported(message) => {
                Self::Unsupported(message)
            }
            revy_server_config::ServerConfigError::Config(message) => Self::Config(message),
        }
    }
}
