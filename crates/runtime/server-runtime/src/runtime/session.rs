use super::{
    LOGIN_SERVER_ID, LOGIN_VERIFY_TOKEN_LEN, LoginChallengeState, OnlineAuthKeys, RuntimeServer,
    SessionHandle, SessionMessage, SessionState, now_ms,
};
use crate::RuntimeError;
use crate::transport::{
    AcceptedTransportSession, TransportSessionIo, default_wire_codec, write_payload,
};
use bytes::BytesMut;
use mc_core::{ConnectionId, CoreCommand, CoreEvent, SessionCapabilitySet};
use mc_plugin_api::codec::auth::{AuthMode, BedrockAuthResult};
use mc_plugin_host::{HotSwappableAuthProfile, HotSwappableGameplayProfile};
use mc_proto_common::{
    ConnectionPhase, Edition, HandshakeNextState, LoginRequest, PlayEncodingContext,
    ProtocolAdapter, ServerListStatus, StatusRequest, TransportKind, WireCodec,
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
    async fn disconnect_login(
        transport_io: &mut TransportSessionIo,
        current: &Arc<dyn ProtocolAdapter>,
        reason: &str,
    ) -> Result<bool, RuntimeError> {
        let disconnect = current.encode_disconnect(ConnectionPhase::Login, reason)?;
        write_payload(transport_io, current.wire_codec(), &disconnect).await?;
        Ok(true)
    }

    async fn handle_bedrock_network_settings_request(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        current: &Arc<dyn ProtocolAdapter>,
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
        current: &Arc<dyn ProtocolAdapter>,
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
        current: &Arc<dyn ProtocolAdapter>,
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

    async fn handle_encryption_response(
        &self,
        connection_id: ConnectionId,
        transport_io: &mut TransportSessionIo,
        session: &mut SessionState,
        current: &Arc<dyn ProtocolAdapter>,
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
        let captured_generation_id = auth_generation.generation_id;
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
            entity_id: session.entity_id,
            protocol_generation: adapter.plugin_generation_id(),
            gameplay_generation: gameplay.plugin_generation_id(),
        });
    }

    fn gameplay_profile_for_adapter(&self, adapter_id: &str) -> &str {
        self.config
            .gameplay_profile_map
            .get(adapter_id)
            .map_or(&self.config.default_gameplay_profile, String::as_str)
    }

    pub(super) fn resolve_gameplay_for_adapter(
        &self,
        adapter_id: &str,
    ) -> Result<Arc<HotSwappableGameplayProfile>, RuntimeError> {
        let profile_id = self.gameplay_profile_for_adapter(adapter_id);
        self.plugin_host
            .as_ref()
            .and_then(|plugin_host| plugin_host.resolve_gameplay_profile(profile_id))
            .or_else(|| self.loaded_plugins.resolve_gameplay_profile(profile_id))
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
            handle.topology_generation_id = session.topology_generation_id;
            handle.transport = session.transport;
            handle.phase = session.phase;
            handle.adapter_id = session
                .adapter
                .as_ref()
                .map(|adapter| adapter.descriptor().adapter_id);
            handle.player_id = session.player_id;
            handle.entity_id = session.entity_id;
            handle.gameplay_profile = session
                .session_capabilities
                .as_ref()
                .map(|capabilities| capabilities.gameplay_profile.clone());
            handle
                .session_capabilities
                .clone_from(&session.session_capabilities);
        }
    }

    pub(super) async fn spawn_transport_session(
        self: &Arc<Self>,
        topology_generation_id: super::TopologyGenerationId,
        transport_session: AcceptedTransportSession,
    ) {
        let Some(topology) = self.topology_generation(topology_generation_id) else {
            eprintln!(
                "dropping transport session because topology generation {:?} is no longer active",
                topology_generation_id
            );
            return;
        };
        let session = match transport_session.transport {
            TransportKind::Tcp => SessionState {
                topology_generation_id,
                transport: TransportKind::Tcp,
                phase: ConnectionPhase::Handshaking,
                adapter: None,
                gameplay: None,
                login_challenge: None,
                player_id: None,
                entity_id: None,
                session_capabilities: None,
            },
            TransportKind::Udp => {
                let Some(adapter) = topology.default_bedrock_adapter.clone() else {
                    eprintln!(
                        "dropping bedrock session because no default bedrock adapter is active"
                    );
                    return;
                };
                let gameplay = match self
                    .resolve_gameplay_for_adapter(&adapter.descriptor().adapter_id)
                {
                    Ok(gameplay) => gameplay,
                    Err(error) => {
                        eprintln!(
                            "dropping bedrock session because gameplay profile could not resolve: {error}"
                        );
                        return;
                    }
                };
                let mut session = SessionState {
                    topology_generation_id,
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
                session
            }
        };
        self.spawn_session_with_state(transport_session, session)
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
                topology_generation_id: session.topology_generation_id,
                transport: session.transport,
                phase: session.phase,
                adapter_id: session
                    .adapter
                    .as_ref()
                    .map(|adapter| adapter.descriptor().adapter_id),
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
                    let codec: &dyn WireCodec = match session.adapter.as_ref() {
                        Some(current) => current.wire_codec(),
                        None => default_wire_codec(session.transport)?,
                    };
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
        let topology = self
            .topology_generation(session.topology_generation_id)
            .ok_or_else(|| RuntimeError::Config("missing topology generation".to_string()))?;
        let Some(intent) = topology
            .protocol_registry
            .route_handshake(session.transport, frame)?
        else {
            return Ok(true);
        };
        let next_phase = match intent.next_state {
            HandshakeNextState::Status => ConnectionPhase::Status,
            HandshakeNextState::Login => ConnectionPhase::Login,
        };
        if let Some(next_adapter) = topology.protocol_registry.resolve_route(
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

        let fallback = Arc::clone(&topology.default_adapter);
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
        let topology = self
            .topology_generation(session.topology_generation_id)
            .ok_or_else(|| RuntimeError::Config("missing topology generation".to_string()))?;
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
                    max_players: usize::from(topology.config.max_players),
                    description: topology.config.motd.clone(),
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
        match message {
            SessionMessage::Event(event) => {
                let event = event.as_ref();
                Self::refresh_session_capabilities(session);
                let current = session
                    .adapter
                    .as_ref()
                    .ok_or_else(|| RuntimeError::Config("missing protocol adapter".to_string()))?;
                let packets = match &event {
                    CoreEvent::LoginAccepted { player, .. } => {
                        vec![current.encode_login_success(player)?]
                    }
                    CoreEvent::Disconnect { reason } => {
                        vec![current.encode_disconnect(session.phase, reason)?]
                    }
                    _ => {
                        let player_id = session.player_id.ok_or_else(|| {
                            RuntimeError::Config(
                                "missing player id for play event encoding".to_string(),
                            )
                        })?;
                        let entity_id = session.entity_id.ok_or_else(|| {
                            RuntimeError::Config(
                                "missing entity id for play event encoding".to_string(),
                            )
                        })?;
                        current.encode_play_event(
                            event,
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
                        session.player_id = Some(*accepted_player_id);
                        session.entity_id = Some(*accepted_entity_id);
                        session.phase = ConnectionPhase::Play;
                        Self::refresh_session_capabilities(session);
                        self.sync_session_handle(connection_id, session).await;
                    }
                    CoreEvent::Disconnect { .. } => return Ok(true),
                    _ => {}
                }
            }
            SessionMessage::Terminate { reason } => {
                if session.phase == ConnectionPhase::Play
                    && let Some(current) = session.adapter.as_ref()
                    && let Ok(packet) = current.encode_disconnect(ConnectionPhase::Play, &reason)
                {
                    let _ = write_payload(transport_io, current.wire_codec(), &packet).await;
                }
                return Ok(true);
            }
        }
        Ok(false)
    }
}
