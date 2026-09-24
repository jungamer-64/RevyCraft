use super::crypto::{decrypt_login_blob, minecraft_server_hash, random_verify_token};
use crate::RuntimeError;
use crate::runtime::{
    BoundSession, LOGIN_SERVER_ID, LoginChallenge, LoginSession, RuntimeServer,
    SessionCommandContext, SessionPhase,
};
use crate::transport::{TransportSessionIo, write_payload_confirmed};
use mc_plugin_contract::codec::auth::{AuthMode, BedrockAuthResult};
use mc_proto_common::{Edition, LoginRequest, ProtocolAdapter, TransportKind};
use revy_voxel_core::{ConnectionId, CoreCommand};
use std::sync::Arc;

impl RuntimeServer {
    async fn disconnect_login(
        transport_io: &mut TransportSessionIo,
        current: &Arc<dyn ProtocolAdapter>,
        reason: &str,
    ) -> Result<bool, RuntimeError> {
        let disconnect =
            current.encode_disconnect(mc_proto_common::ConnectionPhase::Login, reason)?;
        write_payload_confirmed(transport_io, current.wire_codec(), &disconnect).await?;
        Ok(true)
    }

    fn login_binding(session: &SessionPhase) -> Result<&BoundSession, RuntimeError> {
        match session {
            SessionPhase::Login(login) => Ok(login.binding()),
            _ => Err(RuntimeError::Config(
                "login operation reached a non-login session".to_string(),
            )),
        }
    }

    fn login_context(session: &SessionPhase) -> Result<SessionCommandContext, RuntimeError> {
        session.command_context().ok_or_else(|| {
            RuntimeError::Config("login session has no command capability".to_string())
        })
    }

    async fn handle_bedrock_network_settings_request(
        &self,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionPhase,
        protocol_number: i32,
    ) -> Result<bool, RuntimeError> {
        let binding = Self::login_binding(session)?.clone();
        let Some(adapter) = binding.generation.protocol_registry.resolve_route(
            TransportKind::Udp,
            Edition::Be,
            protocol_number,
        ) else {
            return Self::disconnect_login(
                transport_io,
                &binding.adapter,
                &format!("Unsupported Bedrock protocol {protocol_number}"),
            )
            .await;
        };
        let gameplay = self
            .resolve_gameplay_for_adapter(&adapter.descriptor().adapter_id)
            .await?;
        let binding = BoundSession::new(
            Arc::clone(&binding.generation),
            TransportKind::Udp,
            Arc::clone(&adapter),
            gameplay,
            None,
        );
        *session = SessionPhase::Login(LoginSession::Negotiating(binding));

        let response = adapter.encode_network_settings(1)?;
        write_payload_confirmed(transport_io, adapter.wire_codec(), &response).await?;
        transport_io.enable_bedrock_compression(1)?;
        Ok(false)
    }

    async fn handle_bedrock_login(
        &self,
        connection_id: ConnectionId,
        session: &mut SessionPhase,
        protocol_number: i32,
        display_name: String,
        chain_jwts: Vec<String>,
        client_data_jwt: String,
    ) -> Result<bool, RuntimeError> {
        let current = Self::login_binding(session)?.clone();
        let adapter = if current.adapter.descriptor().edition == Edition::Be
            && current.adapter.descriptor().protocol_number == protocol_number
        {
            Arc::clone(&current.adapter)
        } else {
            current
                .generation
                .protocol_registry
                .resolve_route(TransportKind::Udp, Edition::Be, protocol_number)
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "no active bedrock adapter for protocol {protocol_number}"
                    ))
                })?
        };
        let gameplay = self
            .resolve_gameplay_for_adapter(&adapter.descriptor().adapter_id)
            .await?;
        let binding = BoundSession::new(
            Arc::clone(&current.generation),
            TransportKind::Udp,
            adapter,
            gameplay,
            None,
        );
        *session = SessionPhase::Login(LoginSession::Negotiating(binding));

        let auth_profile = self.resolve_bedrock_auth_profile().await?;
        let authenticated = match auth_profile.mode()? {
            AuthMode::BedrockOffline => auth_profile.authenticate_bedrock_offline(&display_name)?,
            AuthMode::BedrockXbl => {
                auth_profile.authenticate_bedrock_xbl(&chain_jwts, &client_data_jwt)?
            }
            mode => {
                return Err(RuntimeError::Config(format!(
                    "bedrock listener requires a bedrock auth profile, got {mode:?}"
                )));
            }
        };
        self.apply_bedrock_login(connection_id, session, authenticated, display_name)
            .await?;
        Ok(false)
    }

    async fn handle_login_start(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionPhase,
        username: String,
    ) -> Result<bool, RuntimeError> {
        let binding = Self::login_binding(session)?.clone();
        if self.reload.static_config().bootstrap.online_mode {
            if matches!(
                session,
                SessionPhase::Login(LoginSession::Authenticating { .. })
            ) {
                return Self::disconnect_login(
                    transport_io,
                    &binding.adapter,
                    "Login encryption is already in progress",
                )
                .await;
            }
            let epoch = self.authority.active();
            let Some(online_auth_keys) = epoch.online_auth_keys.clone() else {
                return Err(RuntimeError::Config(
                    "online-mode=true requires generated auth keys".to_string(),
                ));
            };
            let verify_token = random_verify_token();
            let auth_profile = epoch.selection.auth_profile.clone();
            let auth_generation = auth_profile.capture_generation()?;
            let encryption_request = binding.adapter.encode_encryption_request(
                LOGIN_SERVER_ID,
                &online_auth_keys.public_key_der,
                &verify_token,
            )?;
            *session = SessionPhase::Login(LoginSession::Authenticating {
                binding: binding.clone(),
                challenge: LoginChallenge {
                    username,
                    verify_token,
                    auth_generation,
                },
            });
            write_payload_confirmed(
                transport_io,
                binding.adapter.wire_codec(),
                &encryption_request,
            )
            .await?;
            return Ok(false);
        }

        let auth_profile = self.authority.active().selection.auth_profile.clone();
        let authenticated = auth_profile.authenticate_offline(&username)?;
        let context = Self::login_context(session)?;
        self.apply_command(
            CoreCommand::LoginStart {
                connection_id,
                username,
                player_id: authenticated,
            },
            Some(context),
        )
        .await?;
        Ok(false)
    }

    async fn handle_encryption_response(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionPhase,
        shared_secret_encrypted: Vec<u8>,
        verify_token_encrypted: Vec<u8>,
    ) -> Result<bool, RuntimeError> {
        let (binding, challenge) = match session {
            SessionPhase::Login(LoginSession::Authenticating { binding, challenge }) => {
                (binding.clone(), challenge.clone())
            }
            SessionPhase::Login(login) => {
                return Self::disconnect_login(
                    transport_io,
                    &login.binding().adapter,
                    "Unexpected encryption response",
                )
                .await;
            }
            _ => {
                return Err(RuntimeError::Config(
                    "encryption response reached a non-login session".to_string(),
                ));
            }
        };
        if !self.reload.static_config().bootstrap.online_mode {
            return Self::disconnect_login(
                transport_io,
                &binding.adapter,
                "Encryption response is not valid in offline mode",
            )
            .await;
        }
        let epoch = self.authority.active();
        let Some(online_auth_keys) = epoch.online_auth_keys.clone() else {
            return Err(RuntimeError::Config(
                "online-mode=true requires generated auth keys".to_string(),
            ));
        };
        let Ok(shared_secret) =
            decrypt_login_blob(&online_auth_keys.private_key, &shared_secret_encrypted)
        else {
            return Self::disconnect_login(
                transport_io,
                &binding.adapter,
                "Invalid encryption response",
            )
            .await;
        };
        let Ok(verify_token) =
            decrypt_login_blob(&online_auth_keys.private_key, &verify_token_encrypted)
        else {
            return Self::disconnect_login(
                transport_io,
                &binding.adapter,
                "Invalid encryption response",
            )
            .await;
        };
        let Ok(shared_secret) = shared_secret.try_into() else {
            return Self::disconnect_login(
                transport_io,
                &binding.adapter,
                "Invalid shared secret length",
            )
            .await;
        };
        transport_io.enable_encryption(shared_secret)?;
        if verify_token.as_slice() != challenge.verify_token {
            return Self::disconnect_login(
                transport_io,
                &binding.adapter,
                "Encryption verification failed",
            )
            .await;
        }
        let server_hash = minecraft_server_hash(
            LOGIN_SERVER_ID,
            &shared_secret,
            &online_auth_keys.public_key_der,
        );
        let username = challenge.username.clone();
        let login_username = challenge.username;
        let auth_generation = Arc::clone(&challenge.auth_generation);
        let captured_generation_id = auth_generation.generation_id();
        let auth_profile = epoch.selection.auth_profile.clone();
        let authenticated = match tokio::task::spawn_blocking(move || {
            let current_generation_id = auth_profile
                .plugin_generation_id()
                .ok_or_else(|| RuntimeError::Config("missing auth generation".to_string()))?;
            if current_generation_id == captured_generation_id {
                auth_profile
                    .authenticate_online(&username, &server_hash)
                    .map_err(RuntimeError::from)
            } else {
                auth_generation
                    .authenticate_online(&username, &server_hash)
                    .map_err(RuntimeError::from)
            }
        })
        .await
        {
            Ok(Ok(player_id)) => player_id,
            Ok(Err(error)) => {
                return Self::disconnect_login(
                    transport_io,
                    &binding.adapter,
                    &format!("Authentication failed: {error}"),
                )
                .await;
            }
            Err(error) => return Err(RuntimeError::Join(error)),
        };
        *session = SessionPhase::Login(LoginSession::Negotiating(binding));
        let context = Self::login_context(session)?;
        self.apply_command(
            CoreCommand::LoginStart {
                connection_id,
                username: login_username,
                player_id: authenticated,
            },
            Some(context),
        )
        .await?;
        Ok(false)
    }

    pub(in crate::runtime::session) async fn handle_login_frame(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionPhase,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let adapter = Arc::clone(&Self::login_binding(session)?.adapter);
        match adapter.decode_login(frame)? {
            LoginRequest::BedrockNetworkSettingsRequest { protocol_number } => {
                self.handle_bedrock_network_settings_request(transport_io, session, protocol_number)
                    .await
            }
            LoginRequest::BedrockLogin {
                protocol_number,
                display_name,
                chain_jwts,
                client_data_jwt,
            } => {
                self.handle_bedrock_login(
                    connection_id,
                    session,
                    protocol_number,
                    display_name,
                    chain_jwts,
                    client_data_jwt,
                )
                .await
            }
            LoginRequest::LoginStart { username } => {
                self.handle_login_start(connection_id, transport_io, session, username)
                    .await
            }
            LoginRequest::EncryptionResponse {
                shared_secret_encrypted,
                verify_token_encrypted,
            } => {
                self.handle_encryption_response(
                    connection_id,
                    transport_io,
                    session,
                    shared_secret_encrypted,
                    verify_token_encrypted,
                )
                .await
            }
        }
    }

    async fn apply_bedrock_login(
        &self,
        connection_id: ConnectionId,
        session: &SessionPhase,
        authenticated: BedrockAuthResult,
        fallback_display_name: String,
    ) -> Result<(), RuntimeError> {
        let context = Self::login_context(session)?;
        self.apply_command(
            CoreCommand::LoginStart {
                connection_id,
                username: if authenticated.display_name.is_empty() {
                    fallback_display_name
                } else {
                    authenticated.display_name
                },
                player_id: authenticated.player_id,
            },
            Some(context),
        )
        .await
    }
}
