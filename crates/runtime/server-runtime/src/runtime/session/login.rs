use super::crypto::{decrypt_login_blob, minecraft_server_hash, random_verify_token};
use crate::RuntimeError;
use crate::runtime::{LOGIN_SERVER_ID, LoginChallengeState, RuntimeServer, SessionState};
use crate::transport::{TransportSessionIo, write_payload};
use mc_core::{ConnectionId, CoreCommand};
use mc_plugin_api::codec::auth::{AuthMode, BedrockAuthResult};
use mc_proto_common::{Edition, LoginRequest, TransportKind};
use std::sync::Arc;

impl RuntimeServer {
    async fn disconnect_login(
        transport_io: &mut TransportSessionIo,
        current: &Arc<dyn mc_proto_common::ProtocolAdapter>,
        reason: &str,
    ) -> Result<bool, RuntimeError> {
        let disconnect =
            current.encode_disconnect(mc_proto_common::ConnectionPhase::Login, reason)?;
        write_payload(transport_io, current.wire_codec(), &disconnect).await?;
        Ok(true)
    }

    async fn handle_bedrock_network_settings_request(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        current: &Arc<dyn mc_proto_common::ProtocolAdapter>,
        protocol_number: i32,
    ) -> Result<bool, RuntimeError> {
        let topology = self
            .topology_generation(session.topology_generation_id)
            .ok_or_else(|| RuntimeError::Config("missing topology generation".to_string()))?;
        let Some(next_adapter) = topology.protocol_registry.resolve_route(
            TransportKind::Udp,
            Edition::Be,
            protocol_number,
        ) else {
            return Self::disconnect_login(
                transport_io,
                current,
                &format!("Unsupported Bedrock protocol {protocol_number}"),
            )
            .await;
        };
        let gameplay = self.resolve_gameplay_for_adapter(&next_adapter.descriptor().adapter_id)?;
        session.adapter = Some(next_adapter.clone());
        session.gameplay = Some(gameplay);
        Self::refresh_session_capabilities(session);
        self.sync_session_handle(connection_id, session).await;

        let response = next_adapter.encode_network_settings(1)?;
        write_payload(transport_io, next_adapter.wire_codec(), &response).await?;
        transport_io.enable_bedrock_compression(1);
        Ok(false)
    }

    async fn handle_bedrock_login(
        &self,
        connection_id: ConnectionId,
        session: &mut SessionState,
        current: &Arc<dyn mc_proto_common::ProtocolAdapter>,
        login: LoginRequest,
    ) -> Result<bool, RuntimeError> {
        let LoginRequest::BedrockLogin {
            protocol_number,
            display_name,
            chain_jwts,
            client_data_jwt,
        } = login
        else {
            unreachable!("bedrock login helper only accepts BedrockLogin requests");
        };
        let next_adapter = if current.descriptor().edition == Edition::Be
            && current.descriptor().protocol_number == protocol_number
        {
            Arc::clone(current)
        } else {
            let topology = self
                .topology_generation(session.topology_generation_id)
                .ok_or_else(|| RuntimeError::Config("missing topology generation".to_string()))?;
            topology
                .protocol_registry
                .resolve_route(TransportKind::Udp, Edition::Be, protocol_number)
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "no active bedrock adapter for protocol {protocol_number}"
                    ))
                })?
        };
        let gameplay = self.resolve_gameplay_for_adapter(&next_adapter.descriptor().adapter_id)?;
        session.adapter = Some(next_adapter);
        session.gameplay = Some(gameplay);
        Self::refresh_session_capabilities(session);
        self.sync_session_handle(connection_id, session).await;

        let auth_profile = self.resolve_bedrock_auth_profile()?;
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
        session: &mut SessionState,
        current: &Arc<dyn mc_proto_common::ProtocolAdapter>,
        username: String,
    ) -> Result<bool, RuntimeError> {
        if self.config.online_mode {
            if session.login_challenge.is_some() {
                return Self::disconnect_login(
                    transport_io,
                    current,
                    "Login encryption is already in progress",
                )
                .await;
            }
            let Some(online_auth_keys) = self.online_auth_keys.as_ref() else {
                return Err(RuntimeError::Config(
                    "online-mode=true requires generated auth keys".to_string(),
                ));
            };
            let verify_token = random_verify_token();
            let auth_generation = self.auth_profile.capture_generation()?;
            let encryption_request = current.encode_encryption_request(
                LOGIN_SERVER_ID,
                &online_auth_keys.public_key_der,
                &verify_token,
            )?;
            session.login_challenge = Some(LoginChallengeState {
                username,
                verify_token,
                auth_generation,
            });
            write_payload(transport_io, current.wire_codec(), &encryption_request).await?;
            return Ok(false);
        }

        let authenticated = self.auth_profile.authenticate_offline(&username)?;
        self.apply_command(
            CoreCommand::LoginStart {
                connection_id,
                username,
                player_id: authenticated,
            },
            Some(session),
        )
        .await?;
        Ok(false)
    }

    async fn handle_encryption_response(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        current: &Arc<dyn mc_proto_common::ProtocolAdapter>,
        shared_secret_encrypted: Vec<u8>,
        verify_token_encrypted: Vec<u8>,
    ) -> Result<bool, RuntimeError> {
        if !self.config.online_mode {
            return Self::disconnect_login(
                transport_io,
                current,
                "Encryption response is not valid in offline mode",
            )
            .await;
        }

        let Some(challenge) = session.login_challenge.take() else {
            return Self::disconnect_login(transport_io, current, "Unexpected encryption response")
                .await;
        };
        let Some(online_auth_keys) = self.online_auth_keys.as_ref() else {
            return Err(RuntimeError::Config(
                "online-mode=true requires generated auth keys".to_string(),
            ));
        };
        let Ok(shared_secret) =
            decrypt_login_blob(&online_auth_keys.private_key, &shared_secret_encrypted)
        else {
            return Self::disconnect_login(transport_io, current, "Invalid encryption response")
                .await;
        };
        let Ok(verify_token) =
            decrypt_login_blob(&online_auth_keys.private_key, &verify_token_encrypted)
        else {
            return Self::disconnect_login(transport_io, current, "Invalid encryption response")
                .await;
        };
        let Ok(shared_secret) = shared_secret.try_into() else {
            return Self::disconnect_login(transport_io, current, "Invalid shared secret length")
                .await;
        };
        transport_io.enable_encryption(shared_secret);
        if verify_token.as_slice() != challenge.verify_token {
            return Self::disconnect_login(transport_io, current, "Encryption verification failed")
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
        let auth_profile = Arc::clone(&self.auth_profile);
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
                    current,
                    &format!("Authentication failed: {error}"),
                )
                .await;
            }
            Err(error) => return Err(RuntimeError::Join(error)),
        };
        self.apply_command(
            CoreCommand::LoginStart {
                connection_id,
                username: login_username,
                player_id: authenticated,
            },
            Some(session),
        )
        .await?;
        Ok(false)
    }

    pub(in crate::runtime::session) async fn handle_login_frame(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let current = Arc::clone(
            session
                .adapter
                .as_ref()
                .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?,
        );
        match current.decode_login(frame)? {
            LoginRequest::BedrockNetworkSettingsRequest { protocol_number } => {
                self.handle_bedrock_network_settings_request(
                    connection_id,
                    transport_io,
                    session,
                    &current,
                    protocol_number,
                )
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
                    &current,
                    LoginRequest::BedrockLogin {
                        protocol_number,
                        display_name,
                        chain_jwts,
                        client_data_jwt,
                    },
                )
                .await
            }
            LoginRequest::LoginStart { username } => {
                self.handle_login_start(connection_id, transport_io, session, &current, username)
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
                    &current,
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
        session: &SessionState,
        authenticated: BedrockAuthResult,
        fallback_display_name: String,
    ) -> Result<(), RuntimeError> {
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
            Some(session),
        )
        .await
    }
}
