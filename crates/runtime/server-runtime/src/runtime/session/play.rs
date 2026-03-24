use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SessionState};
use mc_core::CoreCommand;

impl RuntimeServer {
    pub(in crate::runtime::session) async fn handle_play_frame(
        &self,
        session: &mut SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        let Some(current_player_id) = session.player_id else {
            return Ok(true);
        };
        let Some(command) = current.decode_play(current_player_id, frame)? else {
            return Ok(false);
        };
        match command {
            CoreCommand::InventoryClick { transaction, .. }
                if session
                    .pending_rejected_inventory_transaction
                    .is_some_and(|pending| pending.window_id == transaction.window_id) =>
            {
                return Ok(false);
            }
            CoreCommand::InventoryTransactionAck {
                transaction,
                accepted,
                ..
            } => {
                if session.pending_rejected_inventory_transaction == Some(transaction) && !accepted
                {
                    session.pending_rejected_inventory_transaction = None;
                }
                self.apply_command(command, Some(session)).await?;
            }
            _ => self.apply_command(command, Some(session)).await?,
        }
        Ok(false)
    }
}
