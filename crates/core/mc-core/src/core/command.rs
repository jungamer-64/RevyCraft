use super::{
    ServerCore,
    canonical::{ApplyCoreOpsOptions, CoreOp, apply_core_ops},
};
use crate::events::{CoreCommand, TargetedEvent};

impl ServerCore {
    pub fn apply_command(&mut self, command: CoreCommand, now_ms: u64) -> Vec<TargetedEvent> {
        match command {
            CoreCommand::LoginStart {
                connection_id,
                username,
                player_id,
            } => {
                let mut tx = self.begin_gameplay_transaction(now_ms);
                match tx.begin_login(connection_id, username, player_id) {
                    Ok(Some(rejection)) => rejection,
                    Ok(None) => {
                        tx.finalize_login(connection_id, player_id)
                            .expect("built-in login finalize should succeed");
                        tx.commit()
                    }
                    Err(error) => {
                        panic!("built-in login preparation should not fail: {error}");
                    }
                }
            }
            CoreCommand::UpdateClientView {
                player_id,
                view_distance,
            } => apply_core_ops(
                self,
                vec![CoreOp::SetViewDistance {
                    player_id,
                    view_distance,
                }],
                now_ms,
                ApplyCoreOpsOptions::default(),
            ),
            CoreCommand::InventoryClick {
                player_id,
                transaction,
                target,
                button,
                validation,
            } => apply_core_ops(
                self,
                vec![CoreOp::InventoryClick {
                    player_id,
                    transaction,
                    target,
                    button,
                    validation,
                }],
                now_ms,
                ApplyCoreOpsOptions::default(),
            ),
            CoreCommand::CloseContainer {
                player_id,
                window_id,
            } => apply_core_ops(
                self,
                vec![CoreOp::CloseContainer {
                    player_id,
                    window_id,
                    include_player_contents: true,
                }],
                now_ms,
                ApplyCoreOpsOptions::default(),
            ),
            command @ (CoreCommand::MoveIntent { .. }
            | CoreCommand::SetHeldSlot { .. }
            | CoreCommand::CreativeInventorySet { .. }
            | CoreCommand::DigBlock { .. }
            | CoreCommand::PlaceBlock { .. }
            | CoreCommand::UseBlock { .. }) => {
                let gameplay_command = command
                    .into_gameplay()
                    .expect("gameplay-owned command should convert to GameplayCommand");
                self.apply_builtin_gameplay_command(gameplay_command, now_ms)
            }
            CoreCommand::KeepAliveResponse {
                player_id,
                keep_alive_id,
            } => apply_core_ops(
                self,
                vec![CoreOp::AcknowledgeKeepAlive {
                    player_id,
                    keep_alive_id,
                }],
                now_ms,
                ApplyCoreOpsOptions::default(),
            ),
            CoreCommand::Disconnect { player_id } => apply_core_ops(
                self,
                vec![CoreOp::DisconnectPlayer { player_id }],
                now_ms,
                ApplyCoreOpsOptions::default(),
            ),
        }
    }
}
