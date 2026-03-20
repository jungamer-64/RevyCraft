use super::*;

pub use crate::export_auth_plugin;

pub trait RustAuthPlugin: Send + Sync + 'static {
    fn descriptor(&self) -> AuthDescriptor;

    fn capability_set(&self) -> CapabilitySet {
        CapabilitySet::new()
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

#[doc(hidden)]
pub fn handle_auth_request<P: RustAuthPlugin>(
    plugin: &P,
    request: AuthRequest,
) -> Result<AuthResponse, String> {
    match request {
        AuthRequest::Describe => Ok(AuthResponse::Descriptor(plugin.descriptor())),
        AuthRequest::CapabilitySet => Ok(AuthResponse::CapabilitySet(plugin.capability_set())),
        AuthRequest::AuthenticateOffline { username } => plugin
            .authenticate_offline(&username)
            .map(AuthResponse::AuthenticatedPlayer),
        AuthRequest::AuthenticateOnline {
            username,
            server_hash,
        } => plugin
            .authenticate_online(&username, &server_hash)
            .map(AuthResponse::AuthenticatedPlayer),
        AuthRequest::AuthenticateBedrockOffline { display_name } => plugin
            .authenticate_bedrock_offline(&display_name)
            .map(AuthResponse::AuthenticatedBedrockPlayer),
        AuthRequest::AuthenticateBedrockXbl {
            chain_jwts,
            client_data_jwt,
        } => plugin
            .authenticate_bedrock_xbl(&chain_jwts, &client_data_jwt)
            .map(AuthResponse::AuthenticatedBedrockPlayer),
    }
}
