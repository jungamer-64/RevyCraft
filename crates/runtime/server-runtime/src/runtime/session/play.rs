use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SessionState};

impl RuntimeServer {
    pub(in crate::runtime::session) async fn handle_play_frame(
        &self,
        session: &SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        let Some(current_player_id) = session.player_id else {
            return Ok(true);
        };
        if let Some(command) = current.decode_play(current_player_id, frame)? {
            self.apply_command(command, Some(session)).await?;
        }
        Ok(false)
    }
}
