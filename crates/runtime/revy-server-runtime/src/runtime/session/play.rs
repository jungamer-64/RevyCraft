use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SharedSessionState};

impl RuntimeServer {
    pub(in crate::runtime::session) async fn handle_play_frame(
        &self,
        connection_id: revy_voxel_core::ConnectionId,
        shared_state: &SharedSessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let (current, snapshot, context) = {
            let session = shared_state.read().await;
            let current = session
                .adapter
                .clone()
                .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
            let snapshot =
                Self::protocol_session_snapshot(connection_id, &Self::session_view(&session));
            let context = Self::session_runtime_context(&session);
            (current, snapshot, context)
        };
        let Some(command) = current.decode_play(&snapshot, frame)? else {
            return Ok(false);
        };
        self.apply_runtime_command(command, Some(context)).await?;
        Ok(false)
    }
}
