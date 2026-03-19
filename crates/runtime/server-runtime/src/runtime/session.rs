use super::{
    LOGIN_SERVER_ID, LOGIN_VERIFY_TOKEN_LEN, LoginChallengeState, OnlineAuthKeys, RuntimeServer,
    SessionHandle, SessionMessage, SessionState, now_ms,
};
use crate::RuntimeError;
use crate::plugin_host::{HotSwappableAuthProfile, HotSwappableGameplayProfile};
use crate::transport::{
    AcceptedTransportSession, TransportSessionIo, default_wire_codec, write_payload,
};
use bedrockrs_network::connection::Connection as BedrockConnection;
use bytes::BytesMut;
use mc_core::{ConnectionId, CoreCommand, CoreEvent, SessionCapabilitySet};
use mc_plugin_api::{AuthMode, BedrockAuthResult};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeNextState, LoginRequest, PlayEncodingContext,
    ServerListStatus, StatusRequest, TransportKind, WireCodec,
};
use num_bigint::BigInt;
use rand::RngCore;
use rsa::pkcs8::EncodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use sha1::{Digest, Sha1};
use std::sync::Arc;
use tokio::sync::mpsc;

impl OnlineAuthKeys {
    pub(super) fn generate() -> Result<Self, RuntimeError> {
        let mut rng = rand::rngs::OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 1024).map_err(|error| {
            RuntimeError::Auth(format!("failed to generate RSA keypair: {error}"))
        })?;
        let public_key_der = RsaPublicKey::from(&private_key)
            .to_public_key_der()
            .map_err(|error| {
                RuntimeError::Auth(format!("failed to encode RSA public key: {error}"))
            })?
            .as_bytes()
            .to_vec();
        Ok(Self {
            private_key,
            public_key_der,
        })
    }
}

fn random_verify_token() -> [u8; LOGIN_VERIFY_TOKEN_LEN] {
    let mut verify_token = [0_u8; LOGIN_VERIFY_TOKEN_LEN];
    rand::rngs::OsRng.fill_bytes(&mut verify_token);
    verify_token
}

fn decrypt_login_blob(private_key: &RsaPrivateKey, bytes: &[u8]) -> Result<Vec<u8>, RuntimeError> {
    private_key
        .decrypt(Pkcs1v15Encrypt, bytes)
        .map_err(|error| RuntimeError::Auth(format!("failed to decrypt login blob: {error}")))
}

fn minecraft_server_hash(
    server_id: &str,
    shared_secret: &[u8; 16],
    public_key_der: &[u8],
) -> String {
    let mut hasher = Sha1::new();
    hasher.update(server_id.as_bytes());
    hasher.update(shared_secret);
    hasher.update(public_key_der);
    let digest = hasher.finalize();
    BigInt::from_signed_bytes_be(&digest).to_str_radix(16)
}

impl RuntimeServer {
    pub(super) fn refresh_session_capabilities(session: &mut SessionState) {
        let Some(adapter) = session.adapter.as_ref() else {
            session.session_capabilities = None;
            return;
        };
        let Some(gameplay) = session.gameplay.as_ref() else {
            session.session_capabilities = None;
            return;
        };
        session.session_capabilities = Some(SessionCapabilitySet {
            protocol: adapter.capability_set(),
            gameplay: gameplay.capability_set(),
            gameplay_profile: gameplay.profile_id(),
            protocol_generation: adapter.plugin_generation_id(),
            gameplay_generation: gameplay.plugin_generation_id(),
        });
    }

    fn gameplay_profile_for_adapter(&self, adapter_id: &str) -> &str {
        self.config
            .gameplay_profile_map
            .get(adapter_id)
            .map(String::as_str)
            .unwrap_or(&self.config.default_gameplay_profile)
    }

    pub(super) fn resolve_gameplay_for_adapter(
        &self,
        adapter_id: &str,
    ) -> Result<Arc<HotSwappableGameplayProfile>, RuntimeError> {
        let profile_id = self.gameplay_profile_for_adapter(adapter_id);
        self.plugin_host
            .as_ref()
            .and_then(|plugin_host| plugin_host.resolve_gameplay_profile(profile_id))
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "gameplay profile `{profile_id}` for adapter `{adapter_id}` is not active"
                ))
            })
    }

    fn resolve_bedrock_auth_profile(&self) -> Result<Arc<HotSwappableAuthProfile>, RuntimeError> {
        self.bedrock_auth_profile
            .clone()
            .ok_or_else(|| RuntimeError::Config("bedrock auth profile is not active".to_string()))
    }

    async fn sync_session_handle(&self, connection_id: ConnectionId, session: &SessionState) {
        if let Some(handle) = self.sessions.lock().await.get_mut(&connection_id) {
            handle.phase = session.phase;
            handle.player_id = session.player_id;
            handle.entity_id = session.entity_id;
            handle.gameplay_profile = session
                .session_capabilities
                .as_ref()
                .map(|capabilities| capabilities.gameplay_profile.clone());
            handle.session_capabilities = session.session_capabilities.clone();
        }
    }

    pub(super) async fn spawn_session(
        self: &Arc<Self>,
        transport_session: AcceptedTransportSession,
    ) {
        let session = SessionState {
            transport: transport_session.transport,
            phase: ConnectionPhase::Handshaking,
            adapter: None,
            gameplay: None,
            login_challenge: None,
            player_id: None,
            entity_id: None,
            session_capabilities: None,
        };
        self.spawn_session_with_state(transport_session, session)
            .await;
    }

    pub(super) async fn spawn_bedrock_session(self: &Arc<Self>, connection: BedrockConnection) {
        let Some(adapter) = self.default_bedrock_adapter.clone() else {
            eprintln!("dropping bedrock session because no default bedrock adapter is active");
            return;
        };
        let gameplay = match self.resolve_gameplay_for_adapter(&adapter.descriptor().adapter_id) {
            Ok(gameplay) => gameplay,
            Err(error) => {
                eprintln!(
                    "dropping bedrock session because gameplay profile could not resolve: {error}"
                );
                return;
            }
        };
        let mut session = SessionState {
            transport: TransportKind::Udp,
            phase: ConnectionPhase::Login,
            adapter: Some(adapter),
            gameplay: Some(gameplay),
            login_challenge: None,
            player_id: None,
            entity_id: None,
            session_capabilities: None,
        };
        Self::refresh_session_capabilities(&mut session);
        self.spawn_session_with_state(
            AcceptedTransportSession {
                transport: TransportKind::Udp,
                io: TransportSessionIo::Bedrock {
                    connection,
                    compression: None,
                },
            },
            session,
        )
        .await;
    }

    async fn spawn_session_with_state(
        self: &Arc<Self>,
        transport_session: AcceptedTransportSession,
        session: SessionState,
    ) {
        let connection_id = {
            let mut next_connection_id = self.next_connection_id.lock().await;
            let connection_id = ConnectionId(*next_connection_id);
            *next_connection_id = next_connection_id.saturating_add(1);
            connection_id
        };

        let (tx, rx) = mpsc::unbounded_channel();
        self.sessions.lock().await.insert(
            connection_id,
            SessionHandle {
                tx,
                phase: session.phase,
                player_id: session.player_id,
                entity_id: session.entity_id,
                gameplay_profile: session
                    .session_capabilities
                    .as_ref()
                    .map(|capabilities| capabilities.gameplay_profile.clone()),
                session_capabilities: session.session_capabilities.clone(),
            },
        );

        let server = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = server
                .run_session(connection_id, transport_session.io, session, rx)
                .await
            {
                eprintln!("session {connection_id:?} ended with error: {error}");
            }
        });
    }

    async fn run_session(
        self: Arc<Self>,
        connection_id: ConnectionId,
        mut transport_io: TransportSessionIo,
        mut session: SessionState,
        mut rx: mpsc::UnboundedReceiver<SessionMessage>,
    ) -> Result<(), RuntimeError> {
        let mut read_buffer = BytesMut::with_capacity(8192);

        loop {
            tokio::select! {
            read = transport_io.read_into(&mut read_buffer) => {
                let bytes_read = read?;
                if bytes_read == 0 {
                    break;
                }
                loop {
                    let codec: &dyn WireCodec = session
                        .adapter
                        .as_ref()
                        .map_or(default_wire_codec(session.transport), |current| current.wire_codec());
                    let Some(frame) = codec.try_decode_frame(&mut read_buffer)? else {
                        break;
                    };
                    let should_close = self
                        .handle_incoming_frame(
                            connection_id,
                            &mut transport_io,
                            &mut session,
                            frame,
                        )
                        .await?;
                    if should_close {
                        self.unregister_session(connection_id, &session).await?;
                        return Ok(());
                    }
                }
            }
            maybe_message = rx.recv() => {
                    let Some(message) = maybe_message else {
                        break;
                    };
                    let should_close = self
                        .handle_outgoing_message(
                            connection_id,
                            &mut transport_io,
                            &mut session,
                            message,
                        )
                        .await?;
                    if should_close {
                        self.unregister_session(connection_id, &session).await?;
                        return Ok(());
                    }
                }
            }
        }

        self.unregister_session(connection_id, &session).await?;
        Ok(())
    }

    async fn handle_incoming_frame(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        frame: Vec<u8>,
    ) -> Result<bool, RuntimeError> {
        Self::refresh_session_capabilities(session);
        match session.phase {
            ConnectionPhase::Handshaking => {
                self.handle_handshake_frame(connection_id, transport_io, session, &frame)
                    .await
            }
            ConnectionPhase::Status => {
                self.handle_status_frame(transport_io, session, &frame)
                    .await
            }
            ConnectionPhase::Login => {
                self.handle_login_frame(connection_id, transport_io, session, &frame)
                    .await
            }
            ConnectionPhase::Play => self.handle_play_frame(session, &frame).await,
        }
    }

    async fn handle_handshake_frame(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let Some(intent) = self
            .protocol_registry
            .route_handshake(session.transport, frame)?
        else {
            return Ok(true);
        };
        let next_phase = match intent.next_state {
            HandshakeNextState::Status => ConnectionPhase::Status,
            HandshakeNextState::Login => ConnectionPhase::Login,
        };
        if let Some(next_adapter) = self.protocol_registry.resolve_route(
            session.transport,
            intent.edition,
            intent.protocol_number,
        ) {
            let gameplay =
                self.resolve_gameplay_for_adapter(&next_adapter.descriptor().adapter_id)?;
            session.adapter = Some(next_adapter);
            session.gameplay = Some(gameplay);
            session.phase = next_phase;
            Self::refresh_session_capabilities(session);
            self.sync_session_handle(connection_id, session).await;
            return Ok(false);
        }

        let fallback = Arc::clone(&self.default_adapter);
        let descriptor = fallback.descriptor();
        match next_phase {
            ConnectionPhase::Status => {
                let gameplay =
                    self.resolve_gameplay_for_adapter(&fallback.descriptor().adapter_id)?;
                session.adapter = Some(fallback);
                session.gameplay = Some(gameplay);
                session.phase = ConnectionPhase::Status;
                Self::refresh_session_capabilities(session);
                self.sync_session_handle(connection_id, session).await;
                Ok(false)
            }
            ConnectionPhase::Login => {
                let disconnect = fallback.encode_disconnect(
                    ConnectionPhase::Login,
                    &format!(
                        "Unsupported protocol {}. This server supports {} (protocol {}).",
                        intent.protocol_number, descriptor.version_name, descriptor.protocol_number
                    ),
                )?;
                write_payload(transport_io, fallback.wire_codec(), &disconnect).await?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    async fn handle_status_frame(
        &self,
        transport_io: &mut TransportSessionIo,
        session: &SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        match current.decode_status(frame)? {
            StatusRequest::Query => {
                let summary = self.player_summary().await;
                let response = current.encode_status_response(&ServerListStatus {
                    version: current.descriptor(),
                    players_online: summary.online_players,
                    max_players: usize::from(summary.max_players),
                    description: self.config.motd.clone(),
                })?;
                write_payload(transport_io, current.wire_codec(), &response).await?;
                Ok(false)
            }
            StatusRequest::Ping { payload } => {
                let response = current.encode_status_pong(payload)?;
                write_payload(transport_io, current.wire_codec(), &response).await?;
                Ok(true)
            }
        }
    }

    async fn handle_login_frame(
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
                let Some(next_adapter) = self.protocol_registry.resolve_route(
                    TransportKind::Udp,
                    Edition::Be,
                    protocol_number,
                ) else {
                    let disconnect = current.encode_disconnect(
                        ConnectionPhase::Login,
                        &format!("Unsupported Bedrock protocol {protocol_number}"),
                    )?;
                    write_payload(transport_io, current.wire_codec(), &disconnect).await?;
                    return Ok(true);
                };
                let gameplay =
                    self.resolve_gameplay_for_adapter(&next_adapter.descriptor().adapter_id)?;
                session.adapter = Some(next_adapter.clone());
                session.gameplay = Some(gameplay);
                Self::refresh_session_capabilities(session);
                self.sync_session_handle(connection_id, session).await;

                let response = next_adapter.encode_network_settings(1)?;
                write_payload(transport_io, next_adapter.wire_codec(), &response).await?;
                transport_io.enable_bedrock_compression(1);
                Ok(false)
            }
            LoginRequest::BedrockLogin {
                protocol_number,
                display_name,
                chain_jwts,
                client_data_jwt,
            } => {
                let next_adapter = if current.descriptor().edition == Edition::Be
                    && current.descriptor().protocol_number == protocol_number
                {
                    Arc::clone(&current)
                } else {
                    self.protocol_registry
                        .resolve_route(TransportKind::Udp, Edition::Be, protocol_number)
                        .ok_or_else(|| {
                            RuntimeError::Config(format!(
                                "no active bedrock adapter for protocol {protocol_number}"
                            ))
                        })?
                };
                let gameplay =
                    self.resolve_gameplay_for_adapter(&next_adapter.descriptor().adapter_id)?;
                session.adapter = Some(next_adapter);
                session.gameplay = Some(gameplay);
                Self::refresh_session_capabilities(session);
                self.sync_session_handle(connection_id, session).await;

                let auth_profile = self.resolve_bedrock_auth_profile()?;
                let authenticated = match auth_profile.mode()? {
                    AuthMode::BedrockOffline => {
                        auth_profile.authenticate_bedrock_offline(&display_name)?
                    }
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
            LoginRequest::LoginStart { username } => {
                if self.config.online_mode {
                    if session.login_challenge.is_some() {
                        let disconnect = current.encode_disconnect(
                            ConnectionPhase::Login,
                            "Login encryption is already in progress",
                        )?;
                        write_payload(transport_io, current.wire_codec(), &disconnect).await?;
                        return Ok(true);
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
                        challenge_started_at: now_ms(),
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
            LoginRequest::EncryptionResponse {
                shared_secret_encrypted,
                verify_token_encrypted,
            } => {
                if !self.config.online_mode {
                    let disconnect = current.encode_disconnect(
                        ConnectionPhase::Login,
                        "Encryption response is not valid in offline mode",
                    )?;
                    write_payload(transport_io, current.wire_codec(), &disconnect).await?;
                    return Ok(true);
                }

                let Some(challenge) = session.login_challenge.take() else {
                    let disconnect = current.encode_disconnect(
                        ConnectionPhase::Login,
                        "Unexpected encryption response",
                    )?;
                    write_payload(transport_io, current.wire_codec(), &disconnect).await?;
                    return Ok(true);
                };

                let Some(online_auth_keys) = self.online_auth_keys.as_ref() else {
                    return Err(RuntimeError::Config(
                        "online-mode=true requires generated auth keys".to_string(),
                    ));
                };
                let shared_secret = match decrypt_login_blob(
                    &online_auth_keys.private_key,
                    &shared_secret_encrypted,
                ) {
                    Ok(shared_secret) => shared_secret,
                    Err(_) => {
                        let disconnect = current.encode_disconnect(
                            ConnectionPhase::Login,
                            "Invalid encryption response",
                        )?;
                        write_payload(transport_io, current.wire_codec(), &disconnect).await?;
                        return Ok(true);
                    }
                };
                let verify_token = match decrypt_login_blob(
                    &online_auth_keys.private_key,
                    &verify_token_encrypted,
                ) {
                    Ok(verify_token) => verify_token,
                    Err(_) => {
                        let disconnect = current.encode_disconnect(
                            ConnectionPhase::Login,
                            "Invalid encryption response",
                        )?;
                        write_payload(transport_io, current.wire_codec(), &disconnect).await?;
                        return Ok(true);
                    }
                };
                let shared_secret: [u8; 16] = match shared_secret.try_into() {
                    Ok(shared_secret) => shared_secret,
                    Err(_) => {
                        let disconnect = current.encode_disconnect(
                            ConnectionPhase::Login,
                            "Invalid shared secret length",
                        )?;
                        write_payload(transport_io, current.wire_codec(), &disconnect).await?;
                        return Ok(true);
                    }
                };
                transport_io.enable_encryption(shared_secret);
                if verify_token.as_slice() != challenge.verify_token {
                    let disconnect = current.encode_disconnect(
                        ConnectionPhase::Login,
                        "Encryption verification failed",
                    )?;
                    write_payload(transport_io, current.wire_codec(), &disconnect).await?;
                    return Ok(true);
                }
                let server_hash = minecraft_server_hash(
                    LOGIN_SERVER_ID,
                    &shared_secret,
                    &online_auth_keys.public_key_der,
                );
                let username = challenge.username.clone();
                let login_username = challenge.username;
                let auth_generation = Arc::clone(&challenge.auth_generation);
                let captured_generation_id = auth_generation.generation_id;
                let auth_profile = Arc::clone(&self.auth_profile);
                let authenticated = match tokio::task::spawn_blocking(move || {
                    let current_generation_id =
                        auth_profile.plugin_generation_id().ok_or_else(|| {
                            RuntimeError::Config("missing auth generation".to_string())
                        })?;
                    if current_generation_id != captured_generation_id {
                        auth_generation.authenticate_online(&username, &server_hash)
                    } else {
                        auth_profile.authenticate_online(&username, &server_hash)
                    }
                })
                .await
                {
                    Ok(Ok(player_id)) => player_id,
                    Ok(Err(error)) => {
                        let disconnect = current.encode_disconnect(
                            ConnectionPhase::Login,
                            &format!("Authentication failed: {error}"),
                        )?;
                        write_payload(transport_io, current.wire_codec(), &disconnect).await?;
                        return Ok(true);
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
        }
    }

    async fn handle_play_frame(
        &self,
        session: &SessionState,
        frame: &[u8],
    ) -> Result<bool, RuntimeError> {
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        let Some(current_player_id) = session.player_id else {
            return Ok(true);
        };
        if let Some(command) = current.decode_play(current_player_id, frame)? {
            self.apply_command(command, Some(session)).await?;
        }
        Ok(false)
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

    async fn handle_outgoing_message(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        message: SessionMessage,
    ) -> Result<bool, RuntimeError> {
        let SessionMessage::Event(event) = message;
        Self::refresh_session_capabilities(session);
        let current = session
            .adapter
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
        let packets = match &event {
            CoreEvent::LoginAccepted { player, .. } => vec![current.encode_login_success(player)?],
            CoreEvent::Disconnect { reason } => {
                vec![current.encode_disconnect(session.phase, reason)?]
            }
            _ => {
                let player_id = session.player_id.ok_or_else(|| {
                    RuntimeError::Config("missing player id for play event encoding".to_string())
                })?;
                let entity_id = session.entity_id.ok_or_else(|| {
                    RuntimeError::Config("missing entity id for play event encoding".to_string())
                })?;
                current.encode_play_event(
                    &event,
                    &PlayEncodingContext {
                        player_id,
                        entity_id,
                    },
                )?
            }
        };
        for packet in packets {
            write_payload(transport_io, current.wire_codec(), &packet).await?;
        }

        match event {
            CoreEvent::LoginAccepted {
                player_id: accepted_player_id,
                entity_id: accepted_entity_id,
                ..
            } => {
                session.player_id = Some(accepted_player_id);
                session.entity_id = Some(accepted_entity_id);
                session.phase = ConnectionPhase::Play;
                Self::refresh_session_capabilities(session);
                self.sync_session_handle(connection_id, session).await;
            }
            CoreEvent::Disconnect { .. } => return Ok(true),
            _ => {}
        }
        Ok(false)
    }
}
