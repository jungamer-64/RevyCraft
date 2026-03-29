use super::*;
use revy_voxel_core::AuthCapabilitySet;

pub trait RustAuthPlugin: Send + Sync + 'static {
    fn descriptor(&self) -> AuthDescriptor;

    fn capability_set(&self) -> AuthCapabilitySet {
        AuthCapabilitySet::default()
    }

    /// Authenticates a Java Edition player without external services.
    ///
    /// # Errors
    ///
    /// Returns an error when the username cannot be authenticated.
    fn authenticate_offline(&self, username: &str) -> Result<PlayerId, String>;

    /// Authenticates a Java Edition player against an online service.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin does not support or rejects online auth.
    fn authenticate_online(&self, _username: &str, _server_hash: &str) -> Result<PlayerId, String> {
        Err("online auth is not implemented for this plugin".to_string())
    }

    /// Authenticates a Bedrock player without XBL validation.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin does not support or rejects Bedrock offline auth.
    fn authenticate_bedrock_offline(
        &self,
        _display_name: &str,
    ) -> Result<BedrockAuthResult, String> {
        Err("bedrock offline auth is not implemented for this plugin".to_string())
    }

    /// Authenticates a Bedrock player using the provided XBL token chain.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin does not support or rejects Bedrock XBL auth.
    fn authenticate_bedrock_xbl(
        &self,
        _chain_jwts: &[String],
        _client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, String> {
        Err("bedrock xbl auth is not implemented for this plugin".to_string())
    }
}
