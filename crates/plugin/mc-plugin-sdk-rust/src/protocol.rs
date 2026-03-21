use super::*;

pub use crate::{delegate_protocol_adapter, export_protocol_plugin};

pub trait RustProtocolPlugin: HandshakeProbe + ProtocolAdapter + Send + Sync + 'static {
    /// Exports protocol plugin session state into an opaque transfer blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot serialize its protocol session state.
    fn export_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        Ok(Vec::new())
    }

    /// Imports protocol plugin session state from a previously exported blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the transfer blob is invalid for the current plugin.
    fn import_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), ProtocolError> {
        Ok(())
    }
}
