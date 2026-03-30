use crate::error::ServerConfigError;
use crate::schema::ServerConfigDocument;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

pub(crate) fn load_server_config_document(
    path: &Path,
) -> Result<ServerConfigDocument, ServerConfigError> {
    let contents = fs::read_to_string(path).map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            ServerConfigError::Config(format!(
                "server config path `{}` was not found",
                path.display()
            ))
        } else {
            ServerConfigError::Io(error)
        }
    })?;
    toml::from_str(&contents).map_err(|error| {
        ServerConfigError::Config(format!(
            "failed to parse config {}: {error}",
            path.display()
        ))
    })
}
