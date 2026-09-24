use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SessionPhase};

impl RuntimeServer {
    pub(in crate::runtime::session) async fn handle_play_frame(
        &self,
        connection_id: revy_voxel_core::ConnectionId,
        session: &SessionPhase,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let SessionPhase::Play(play) = session else {
            return Err(RuntimeError::Config(
                "play frame reached a non-play session".to_string(),
            ));
        };
        let snapshot = session.protocol_snapshot(connection_id);
        let Some(command) = play.binding.adapter.decode_play(&snapshot, frame)? else {
            return Ok(false);
        };
        self.apply_runtime_command(command, session.command_context())
            .await?;
        Ok(false)
    }
}
