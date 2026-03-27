use super::{
    GenerationId, LoginChallengeState, OnlineAuthKeys, RunningServer, RuntimeServer,
    ServerSupervisor, SessionControl, SessionMessage,
};
use crate::RuntimeError;
use crate::config::{ServerConfig, ServerConfigSource};
use crate::runtime::bootstrap::boot_server_from_upgrade;
use crate::transport::{AcceptedTransportSession, TransportEncryptionSnapshot, TransportSessionIo};
use bytes::BytesMut;
use mc_core::{ConnectionId, CoreEvent, CoreRuntimeStateBlob, EntityId, PlayerId};
use mc_plugin_host::host::plugin_host_from_config;
use mc_proto_common::{ConnectionPhase, TransportKind};
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedRwLockWriteGuard, oneshot};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnlineAuthKeysSnapshot {
    pub private_key_pkcs8_der: Vec<u8>,
    pub public_key_der: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeUpgradeLoginChallenge {
    pub username: String,
    pub verify_token: [u8; super::LOGIN_VERIFY_TOKEN_LEN],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RuntimeUpgradeQueuedMessage {
    Event(CoreEvent),
    Terminate { reason: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeUpgradeSessionState {
    pub connection_id: ConnectionId,
    pub generation_id: GenerationId,
    pub transport: TransportKind,
    pub phase: ConnectionPhase,
    pub adapter_id: Option<String>,
    pub player_id: Option<PlayerId>,
    pub entity_id: Option<EntityId>,
    pub gameplay_profile: Option<mc_core::GameplayProfileId>,
    pub protocol_generation: Option<mc_core::PluginGenerationId>,
    pub gameplay_generation: Option<mc_core::PluginGenerationId>,
    pub login_challenge: Option<RuntimeUpgradeLoginChallenge>,
    pub read_buffer: Vec<u8>,
    pub queued_messages: Vec<RuntimeUpgradeQueuedMessage>,
    pub encryption: Option<TransportEncryptionSnapshot>,
    pub protocol_session_blob: Option<Vec<u8>>,
    pub gameplay_session_blob: Option<Vec<u8>>,
}

pub struct RuntimeUpgradeSessionHandle {
    pub state: RuntimeUpgradeSessionState,
    pub stream: std::net::TcpStream,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeUpgradePayload {
    pub config: ServerConfig,
    pub active_generation_id: GenerationId,
    pub core: CoreRuntimeStateBlob,
    pub dirty: bool,
    pub online_auth_keys: Option<OnlineAuthKeysSnapshot>,
    pub sessions: Vec<RuntimeUpgradeSessionState>,
}

pub struct RuntimeUpgradeImport {
    pub payload: RuntimeUpgradePayload,
    pub game_listener: std::net::TcpListener,
    pub sessions: Vec<RuntimeUpgradeSessionHandle>,
}

pub struct RuntimeUpgradeGuard {
    runtime: Arc<RuntimeServer>,
    consistency_guard: Option<OwnedRwLockWriteGuard<()>>,
    game_listener: Option<std::net::TcpListener>,
    sessions: Vec<RuntimeUpgradeSessionHandle>,
    payload: RuntimeUpgradePayload,
}

pub struct RuntimeUpgradeCommitHold {
    _consistency_guard: OwnedRwLockWriteGuard<()>,
}

impl OnlineAuthKeys {
    pub(super) fn snapshot(&self) -> Result<OnlineAuthKeysSnapshot, RuntimeError> {
        Ok(OnlineAuthKeysSnapshot {
            private_key_pkcs8_der: self
                .private_key
                .to_pkcs8_der()
                .map_err(|error| {
                    RuntimeError::Auth(format!("failed to encode auth private key: {error}"))
                })?
                .as_bytes()
                .to_vec(),
            public_key_der: self.public_key_der.clone(),
        })
    }

    pub(super) fn from_snapshot(snapshot: OnlineAuthKeysSnapshot) -> Result<Self, RuntimeError> {
        let private_key = rsa::RsaPrivateKey::from_pkcs8_der(&snapshot.private_key_pkcs8_der)
            .map_err(|error| {
                RuntimeError::Auth(format!("failed to decode auth private key: {error}"))
            })?;
        Ok(Self {
            private_key,
            public_key_der: snapshot.public_key_der,
        })
    }
}

impl RuntimeUpgradeGuard {
    #[must_use]
    pub const fn payload(&self) -> &RuntimeUpgradePayload {
        &self.payload
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the exported listener cannot be cloned for the child process.
    pub fn clone_game_listener(&self) -> Result<std::net::TcpListener, RuntimeError> {
        self.game_listener
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("upgrade game listener is no longer available".to_string()))?
            .try_clone()
            .map_err(Into::into)
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when any exported tcp session socket cannot be cloned for the child process.
    pub fn clone_sessions_for_child(&self) -> Result<Vec<RuntimeUpgradeSessionHandle>, RuntimeError> {
        self.sessions
            .iter()
            .map(|session| {
                Ok(RuntimeUpgradeSessionHandle {
                    state: session.state.clone(),
                    stream: session.stream.try_clone()?,
                })
            })
            .collect()
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when listener/session rollback import fails.
    pub async fn rollback(mut self) -> Result<(), RuntimeError> {
        if let Some(listener) = self.game_listener.take() {
            self.runtime
                .topology
                .import_tcp_listener_after_upgrade_rollback(listener, &self.runtime.sessions)
                .await?;
        }
        self.runtime
            .import_live_sessions_after_upgrade(std::mem::take(&mut self.sessions))
            .await?;
        let _ = self.consistency_guard.take();
        Ok(())
    }

    #[must_use]
    pub fn commit(mut self) -> RuntimeUpgradeCommitHold {
        RuntimeUpgradeCommitHold {
            _consistency_guard: self
                .consistency_guard
                .take()
                .expect("upgrade guard commit should retain consistency guard"),
        }
    }
}

impl ServerSupervisor {
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime cannot freeze and export a consistent upgrade snapshot.
    pub async fn begin_runtime_upgrade(&self) -> Result<RuntimeUpgradeGuard, RuntimeError> {
        self.running.begin_runtime_upgrade().await
    }

    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the replacement runtime cannot boot from the transferred upgrade state.
    pub async fn boot_from_runtime_upgrade(
        config_source: ServerConfigSource,
        import: RuntimeUpgradeImport,
    ) -> Result<Self, RuntimeError> {
        let plugin_host =
            plugin_host_from_config(&import.payload.config.plugin_host_bootstrap_config())?
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "no packaged plugins discovered under `{}`",
                        import.payload.config.bootstrap.plugins_dir.display()
                    ))
                })?;
        let loaded_plugins =
            plugin_host.load_plugin_set(&import.payload.config.plugin_host_runtime_selection_config())?;
        let running = boot_server_from_upgrade(
            config_source,
            import,
            loaded_plugins,
            Some(plugin_host),
        )
        .await?;
        Ok(Self { running })
    }
}

impl RunningServer {
    async fn begin_runtime_upgrade(&self) -> Result<RuntimeUpgradeGuard, RuntimeError> {
        let active_config = self.runtime.selection_state().await.config;
        if active_config.topology.be_enabled {
            return Err(RuntimeError::Unsupported(
                "runtime upgrade does not support bedrock listener/session transfer".to_string(),
            ));
        }

        let game_listener = self.runtime.topology.export_tcp_listener_for_upgrade().await?;
        while self.runtime.sessions.queued_accepts().total_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let consistency_guard = self.runtime.reload.write_consistency_owned().await;
        let sessions = match self.runtime.export_live_sessions_for_upgrade().await {
            Ok(sessions) => sessions,
            Err(error) => {
                self.runtime
                    .topology
                    .import_tcp_listener_after_upgrade_rollback(game_listener, &self.runtime.sessions)
                    .await?;
                return Err(error);
            }
        };
        let core = self.runtime.kernel.export_core_runtime_state().await;
        let payload = RuntimeUpgradePayload {
            config: active_config,
            active_generation_id: self.runtime.topology.active_generation_id(),
            core: core.blob,
            dirty: core.dirty,
            online_auth_keys: self
                .runtime
                .selection
                .online_auth_keys()
                .map(|keys| keys.snapshot())
                .transpose()?,
            sessions: sessions.iter().map(|session| session.state.clone()).collect(),
        };
        Ok(RuntimeUpgradeGuard {
            runtime: Arc::clone(&self.runtime),
            consistency_guard: Some(consistency_guard),
            game_listener: Some(game_listener),
            sessions,
            payload,
        })
    }
}

impl RuntimeServer {
    pub(super) async fn export_live_sessions_for_upgrade(
        self: &Arc<Self>,
    ) -> Result<Vec<RuntimeUpgradeSessionHandle>, RuntimeError> {
        let handles = self.sessions.all_handles().await;
        let mut exported = Vec::with_capacity(handles.len());
        for handle in handles {
            if handle.transport != TransportKind::Tcp {
                self.import_live_sessions_after_upgrade(exported).await?;
                return Err(RuntimeError::Unsupported(
                    "runtime upgrade only supports tcp sessions".to_string(),
                ));
            }
            let (ack_tx, ack_rx) = oneshot::channel();
            handle
                .control_tx
                .send(SessionControl::Export { ack_tx })
                .await
                .map_err(|_| {
                    RuntimeError::Config("failed to export live session for upgrade".to_string())
                })?;
            match ack_rx.await.map_err(|_| {
                RuntimeError::Config("session export channel closed unexpectedly".to_string())
            })? {
                Ok(session) => exported.push(session),
                Err(error) => {
                    self.import_live_sessions_after_upgrade(exported).await?;
                    return Err(error);
                }
            }
        }
        Ok(exported)
    }

    pub(super) async fn import_live_sessions_after_upgrade(
        self: &Arc<Self>,
        sessions: Vec<RuntimeUpgradeSessionHandle>,
    ) -> Result<(), RuntimeError> {
        let selection = self.selection_state().await;
        let generation = self.topology.active_generation();
        for imported in sessions {
            if imported.state.transport != TransportKind::Tcp {
                return Err(RuntimeError::Unsupported(
                    "runtime upgrade only supports tcp session import".to_string(),
                ));
            }
            let adapter = match imported.state.adapter_id.as_deref() {
                Some(adapter_id) => Some(
                    generation
                        .protocol_registry
                        .resolve_adapter(adapter_id)
                        .ok_or_else(|| {
                            RuntimeError::Config(format!(
                                "imported session references inactive adapter `{adapter_id}`"
                            ))
                        })?,
                ),
                None => None,
            };
            let gameplay = match imported.state.gameplay_profile.as_ref() {
                Some(profile_id) => Some(
                    selection
                        .loaded_plugins
                        .resolve_gameplay_profile(profile_id.as_str())
                        .ok_or_else(|| {
                            RuntimeError::Config(format!(
                                "imported session references inactive gameplay profile `{}`",
                                profile_id.as_str()
                            ))
                        })?,
                ),
                None => None,
            };
            let login_challenge = match imported.state.login_challenge.as_ref() {
                Some(challenge) => Some(LoginChallengeState {
                    username: challenge.username.clone(),
                    verify_token: challenge.verify_token,
                    auth_generation: selection.auth_profile.capture_generation()?,
                }),
                None => None,
            };
            let mut session = super::SessionState {
                generation: Arc::clone(&generation),
                transport: imported.state.transport,
                phase: imported.state.phase,
                adapter: adapter.clone(),
                gameplay: gameplay.clone(),
                login_challenge,
                player_id: imported.state.player_id,
                entity_id: imported.state.entity_id,
                session_capabilities: None,
            };
            Self::refresh_session_capabilities(&mut session);
            if let (Some(adapter), Some(blob)) = (adapter.as_ref(), imported.state.protocol_session_blob.as_ref()) {
                adapter
                    .import_session_state(
                        &Self::protocol_session_snapshot(imported.state.connection_id, &session),
                        blob,
                    )
                    .map_err(|error| RuntimeError::Config(error.to_string()))?;
            }
            if let (
                Some(gameplay),
                Some(session_capabilities),
                Some(player_id),
                Some(blob),
            ) = (
                gameplay.as_ref(),
                session.session_capabilities.as_ref(),
                session.player_id,
                imported.state.gameplay_session_blob.as_ref(),
            ) {
                gameplay
                    .import_session_state(
                        &mc_plugin_api::codec::gameplay::GameplaySessionSnapshot {
                            phase: session.phase,
                            player_id: Some(player_id),
                            entity_id: session.entity_id,
                            protocol: session_capabilities.protocol.clone(),
                            gameplay_profile: session_capabilities.gameplay_profile.clone(),
                            protocol_generation: session_capabilities.protocol_generation,
                            gameplay_generation: session_capabilities.gameplay_generation,
                        },
                        blob,
                    )
                    .map_err(|error| RuntimeError::Config(error.to_string()))?;
            }
            let queued_messages = imported
                .state
                .queued_messages
                .into_iter()
                .map(|message| match message {
                    RuntimeUpgradeQueuedMessage::Event(event) => {
                        SessionMessage::Event(Arc::new(event))
                    }
                    RuntimeUpgradeQueuedMessage::Terminate { reason } => {
                        SessionMessage::Terminate { reason }
                    }
                })
                .collect::<Vec<_>>();
            self.spawn_session_with_fixed_connection_id(
                imported.state.connection_id,
                AcceptedTransportSession {
                    transport: TransportKind::Tcp,
                    io: TransportSessionIo::import_tcp_for_upgrade(
                        imported.stream,
                        imported.state.encryption,
                    )?,
                },
                session,
                BytesMut::from(imported.state.read_buffer.as_slice()),
                queued_messages,
            )
            .await;
        }
        Ok(())
    }
}
