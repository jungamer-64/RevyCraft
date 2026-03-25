use crate::RuntimeError;
use crate::runtime::{RuntimeServer, SessionState};
use mc_core::{CoreCommand, ProtocolCapability};

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
        let command = rewrite_bedrock_inventory_command(session, command);
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

fn rewrite_bedrock_inventory_command(session: &SessionState, command: CoreCommand) -> CoreCommand {
    let is_bedrock = session
        .session_capabilities
        .as_ref()
        .is_some_and(|capabilities| capabilities.protocol.contains(&ProtocolCapability::Bedrock));
    if !is_bedrock {
        return command;
    }

    match command {
        CoreCommand::InventoryClick {
            player_id,
            mut transaction,
            target,
            button,
            clicked_item,
        } => {
            if transaction.window_id == 0
                && let Some((active_window_id, _)) = session.active_non_player_window
            {
                transaction.window_id = active_window_id;
            }
            CoreCommand::InventoryClick {
                player_id,
                transaction,
                target,
                button,
                clicked_item,
            }
        }
        CoreCommand::CloseContainer {
            player_id,
            window_id: 0,
        } => session
            .active_non_player_window
            .map(|(active_window_id, _)| CoreCommand::CloseContainer {
                player_id,
                window_id: active_window_id,
            })
            .unwrap_or(CoreCommand::CloseContainer {
                player_id,
                window_id: 0,
            }),
        _ => command,
    }
}
