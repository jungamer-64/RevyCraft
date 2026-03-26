use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SessionState};

impl RuntimeServer {
    pub(in crate::runtime::session) async fn handle_play_frame(
        &self,
        connection_id: mc_core::ConnectionId,
        session: &mut SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        let snapshot = Self::protocol_session_snapshot(connection_id, session);
        let Some(command) = current.decode_play(&snapshot, frame)? else {
            return Ok(false);
        };
        self.apply_runtime_command(command, Some(session)).await?;
        Ok(false)
    }
}
