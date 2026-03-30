use crate::types::ServerConfig;
use crate::validate::ValidatedServerConfig;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerConfigError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported configuration: {0}")]
    Unsupported(String),
    #[error("configuration error: {0}")]
    Config(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerConfigSource {
    Inline(ServerConfig),
    Toml(PathBuf),
}

impl ServerConfigSource {
    /// # Errors
    ///
    /// Returns [`ServerConfigError`] when the source cannot be materialized.
    pub fn load(&self) -> Result<ValidatedServerConfig, ServerConfigError> {
        match self {
            Self::Inline(config) => config.clone().validate_owned(),
            Self::Toml(path) => ServerConfig::from_toml(path),
        }
    }
}
