use super::ServerCore;
use crate::SessionCapabilitySet;
use crate::events::{CoreCommand, TargetedEvent};
use crate::gameplay::{
    CanonicalGameplayPolicy, GameplayPolicyResolver, canonical_session_capabilities,
};

impl ServerCore {
    /// Applies a command using the built-in canonical gameplay policy.
    ///
    /// # Panics
    ///
    /// Panics if the canonical gameplay policy returns an error while evaluating the command.
    pub fn apply_command(&mut self, command: CoreCommand, now_ms: u64) -> Vec<TargetedEvent> {
        let session = canonical_session_capabilities();
        self.apply_command_with_policy(command, now_ms, Some(&session), &CanonicalGameplayPolicy)
            .expect("canonical gameplay policy should not fail")
    }

    /// Applies a command using the provided gameplay policy resolver.
    ///
    /// # Errors
    ///
    /// Returns an error when the command requires session capabilities that are not present,
    /// or when the gameplay policy resolver rejects the command.
    pub fn apply_command_with_policy<R: GameplayPolicyResolver + ?Sized>(
        &mut self,
        command: CoreCommand,
        now_ms: u64,
        session: Option<&SessionCapabilitySet>,
        resolver: &R,
    ) -> Result<Vec<TargetedEvent>, String> {
        match command {
            CoreCommand::LoginStart {
                connection_id,
                username,
                player_id,
            } => self.login_player_with_policy(
                connection_id,
                username,
                player_id,
                now_ms,
                session.ok_or_else(|| "login requires session capabilities".to_string())?,
                resolver,
            ),
            CoreCommand::UpdateClientView {
                player_id,
                view_distance,
            } => Ok(self.update_client_settings(player_id, view_distance)),
            CoreCommand::ClientStatus {
                player_id: _,
                action_id: _,
            } => Ok(Vec::new()),
            CoreCommand::InventoryClick {
                player_id,
                target,
                button,
            } => Ok(self.apply_inventory_click(player_id, target, button)),
            CoreCommand::MoveIntent { .. }
            | CoreCommand::SetHeldSlot { .. }
            | CoreCommand::CreativeInventorySet { .. }
            | CoreCommand::DigBlock { .. }
            | CoreCommand::PlaceBlock { .. } => {
                let session = session.ok_or_else(|| {
                    "gameplay-owned command requires session capabilities".to_string()
                })?;
                let effect = resolver.handle_command(self, session, &command)?;
                Ok(self.apply_gameplay_effect(effect))
            }
            CoreCommand::KeepAliveResponse {
                player_id,
                keep_alive_id,
            } => {
                self.accept_keep_alive(player_id, keep_alive_id);
                Ok(Vec::new())
            }
            CoreCommand::Disconnect { player_id } => Ok(self.disconnect_player(player_id)),
        }
    }
}
