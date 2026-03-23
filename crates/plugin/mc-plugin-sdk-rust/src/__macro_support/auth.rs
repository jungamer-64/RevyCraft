use crate::auth::RustAuthPlugin;
use mc_plugin_api::codec::auth::{AuthRequest, AuthResponse};

pub fn handle_auth_request<P: RustAuthPlugin>(
    plugin: &P,
    request: AuthRequest,
) -> Result<AuthResponse, String> {
    match request {
        AuthRequest::Describe => Ok(AuthResponse::Descriptor(plugin.descriptor())),
        AuthRequest::CapabilitySet => Ok(AuthResponse::CapabilitySet(
            crate::capabilities::auth_announcement(&plugin.capability_set()),
        )),
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
