use super::{
    Arc, AuthGeneration, AuthGenerationHandle, AuthMode, BedrockAuthResult,
    BedrockListenerDescriptor, BytesMut, CapabilitySet, ConnectionPhase, GameplayEffect,
    GameplayGeneration, GameplayJoinEffect, GameplayPolicyResolver, GameplayProfileHandle,
    GameplayProfileId, GameplayQuery, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
    HandshakeIntent, HandshakeProbe, LoginRequest, Path, PlayerId, PlayerSnapshot,
    PluginFailureAction, PluginFailureDispatch, PluginGenerationId, PluginKind, ProtocolAdapter,
    ProtocolDescriptor, ProtocolError, ProtocolGeneration, ProtocolRequest, ProtocolResponse,
    RuntimeError, RwLock, ServerListStatus, SessionCapabilitySet, StatusRequest, StorageAdapter,
    StorageError, StorageGeneration, StorageProfileHandle, StorageRequest, StorageResponse,
    SystemTime, TransportKind, WireCodec, WireFormatKind, WireFrameDecodeResult, WorldSnapshot,
    with_gameplay_query,
};
use mc_proto_common::Edition;

pub(crate) struct HotSwappableProtocolAdapter {
    plugin_id: String,
    pub(crate) generation: RwLock<Arc<ProtocolGeneration>>,
    failures: Arc<PluginFailureDispatch>,
    pub(crate) reload_gate: RwLock<()>,
}

impl HotSwappableProtocolAdapter {
    pub(crate) const fn new(
        plugin_id: String,
        generation: Arc<ProtocolGeneration>,
        failures: Arc<PluginFailureDispatch>,
    ) -> Self {
        Self {
            plugin_id,
            generation: RwLock::new(generation),
            failures,
            reload_gate: RwLock::new(()),
        }
    }

    pub(crate) fn current_generation(&self) -> Result<Arc<ProtocolGeneration>, ProtocolError> {
        if self.failures.is_active_quarantined(&self.plugin_id) {
            return Err(ProtocolError::Plugin(
                self.failures
                    .active_reason(&self.plugin_id)
                    .unwrap_or_else(|| "plugin quarantined".to_string()),
            ));
        }
        Ok(self
            .generation
            .read()
            .expect("protocol generation lock should not be poisoned")
            .clone())
    }

    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn swap_generation(&self, generation: Arc<ProtocolGeneration>) {
        let _guard = self
            .reload_gate
            .write()
            .expect("protocol reload gate should not be poisoned");
        self.swap_generation_while_reloading(generation);
    }

    pub(crate) fn swap_generation_while_reloading(&self, generation: Arc<ProtocolGeneration>) {
        *self
            .generation
            .write()
            .expect("protocol generation lock should not be poisoned") = generation;
    }

    fn quarantine_on_error<T>(&self, result: Result<T, ProtocolError>) -> Result<T, ProtocolError> {
        if let Err(ProtocolError::Plugin(message)) = &result {
            let _ = self.failures.handle_runtime_failure(
                PluginKind::Protocol,
                &self.plugin_id,
                message,
            );
        }
        result
    }

    fn with_generation<T>(
        &self,
        f: impl FnOnce(&ProtocolGeneration) -> Result<T, ProtocolError>,
    ) -> Result<T, ProtocolError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("protocol reload gate should not be poisoned");
        let generation = self.current_generation()?;
        self.quarantine_on_error(f(&generation))
    }
}

impl HandshakeProbe for HotSwappableProtocolAdapter {
    fn transport_kind(&self) -> TransportKind {
        self.with_generation(|generation| Ok(generation.descriptor.transport))
            .unwrap_or(TransportKind::Tcp)
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::TryRoute {
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::HandshakeIntent(intent) => Ok(intent),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected try_route response: {other:?}"
                ))),
            }
        })
    }
}

impl WireCodec for HotSwappableProtocolAdapter {
    fn encode_frame(&self, payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeWireFrame {
                payload: payload.to_vec(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_wire_frame response: {other:?}"
                ))),
            }
        })
    }

    fn try_decode_frame(&self, buffer: &mut BytesMut) -> Result<Option<Vec<u8>>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::TryDecodeWireFrame {
                buffer: buffer.to_vec(),
            })? {
                ProtocolResponse::WireFrameDecodeResult(result) => {
                    let Some(WireFrameDecodeResult {
                        frame,
                        bytes_consumed,
                    }) = result
                    else {
                        return Ok(None);
                    };
                    if bytes_consumed > buffer.len() {
                        return Err(ProtocolError::Plugin(format!(
                            "wire codec consumed {bytes_consumed} buffered bytes but only {} were available",
                            buffer.len()
                        )));
                    }
                    let _ = buffer.split_to(bytes_consumed);
                    Ok(Some(frame))
                }
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected try_decode_wire_frame response: {other:?}"
                ))),
            }
        })
    }
}

impl mc_proto_common::SessionAdapter for HotSwappableProtocolAdapter {
    fn wire_codec(&self) -> &dyn WireCodec {
        self
    }

    fn decode_status(&self, frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::DecodeStatus {
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::StatusRequest(request) => Ok(request),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected decode_status response: {other:?}"
                ))),
            }
        })
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::DecodeLogin {
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::LoginRequest(request) => Ok(request),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected decode_login response: {other:?}"
                ))),
            }
        })
    }

    fn encode_status_response(&self, status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeStatusResponse {
                status: status.clone(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_status_response payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_status_pong(&self, payload: i64) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeStatusPong { payload })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_status_pong payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeDisconnect {
                phase,
                reason: reason.to_string(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_disconnect payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_encryption_request(
        &self,
        server_id: &str,
        public_key_der: &[u8],
        verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeEncryptionRequest {
                server_id: server_id.to_string(),
                public_key_der: public_key_der.to_vec(),
                verify_token: verify_token.to_vec(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_encryption_request payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_network_settings(
        &self,
        compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeNetworkSettings {
                compression_threshold,
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_network_settings payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_login_success(
        &self,
        player: &mc_core::PlayerSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeLoginSuccess {
                player: player.clone(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_login_success payload: {other:?}"
                ))),
            }
        })
    }
}

impl mc_proto_common::PlaySyncAdapter for HotSwappableProtocolAdapter {
    fn decode_play(
        &self,
        player_id: mc_core::PlayerId,
        frame: &[u8],
    ) -> Result<Option<mc_core::CoreCommand>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::DecodePlay {
                player_id,
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::CoreCommand(command) => Ok(command),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected decode_play payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_play_event(
        &self,
        event: &mc_core::CoreEvent,
        context: &super::PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodePlayEvent {
                event: event.clone(),
                context: *context,
            })? {
                ProtocolResponse::Frames(frames) => Ok(frames),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_play_event payload: {other:?}"
                ))),
            }
        })
    }
}

impl ProtocolAdapter for HotSwappableProtocolAdapter {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.with_generation(|generation| Ok(generation.descriptor.clone()))
            .map_or_else(
                |_| ProtocolDescriptor {
                    adapter_id: self.plugin_id.clone(),
                    transport: TransportKind::Tcp,
                    wire_format: WireFormatKind::MinecraftFramed,
                    edition: Edition::Je,
                    version_name: "quarantined".to_string(),
                    protocol_number: -1,
                },
                |descriptor| descriptor,
            )
    }

    fn bedrock_listener_descriptor(&self) -> Option<BedrockListenerDescriptor> {
        self.with_generation(|generation| Ok(generation.bedrock_listener_descriptor.clone()))
            .ok()
            .flatten()
    }

    fn capability_set(&self) -> CapabilitySet {
        self.with_generation(|generation| Ok(generation.capabilities.clone()))
            .unwrap_or_default()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.with_generation(|generation| Ok(generation.generation_id))
            .ok()
    }
}

pub(crate) struct HotSwappableGameplayProfile {
    plugin_id: String,
    profile_id: GameplayProfileId,
    pub(crate) generation: RwLock<Arc<GameplayGeneration>>,
    failures: Arc<PluginFailureDispatch>,
    pub(crate) reload_gate: RwLock<()>,
}

impl HotSwappableGameplayProfile {
    pub(crate) const fn new(
        plugin_id: String,
        profile_id: GameplayProfileId,
        generation: Arc<GameplayGeneration>,
        failures: Arc<PluginFailureDispatch>,
    ) -> Self {
        Self {
            plugin_id,
            profile_id,
            generation: RwLock::new(generation),
            failures,
            reload_gate: RwLock::new(()),
        }
    }

    pub(crate) fn current_generation(&self) -> Arc<GameplayGeneration> {
        self.generation
            .read()
            .expect("gameplay generation lock should not be poisoned")
            .clone()
    }

    pub(crate) fn swap_generation(&self, generation: Arc<GameplayGeneration>) {
        *self
            .generation
            .write()
            .expect("gameplay generation lock should not be poisoned") = generation;
    }

    fn profile_id(&self) -> GameplayProfileId {
        self.profile_id.clone()
    }

    fn capability_set(&self) -> CapabilitySet {
        self.current_generation().capabilities.clone()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Some(self.current_generation().generation_id)
    }

    fn session_closed(&self, session: &GameplaySessionSnapshot) -> Result<(), RuntimeError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        let generation = self.current_generation();
        match generation
            .invoke(&GameplayRequest::SessionClosed {
                session: session.clone(),
            })
            .map_err(RuntimeError::Config)?
        {
            GameplayResponse::Empty => Ok(()),
            other => Err(RuntimeError::Config(format!(
                "unexpected gameplay session_closed payload: {other:?}"
            ))),
        }
    }
}

impl GameplayPolicyResolver for HotSwappableGameplayProfile {
    fn handle_player_join(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        player: &PlayerSnapshot,
    ) -> Result<GameplayJoinEffect, String> {
        let session = GameplaySessionSnapshot {
            phase: ConnectionPhase::Login,
            player_id: Some(player.id),
            entity_id: session.entity_id,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        if self.failures.is_active_quarantined(&self.plugin_id) {
            return Ok(GameplayJoinEffect::default());
        }
        let generation = self.current_generation();
        with_gameplay_query(query, || {
            match generation.invoke(&GameplayRequest::HandlePlayerJoin {
                session,
                player: player.clone(),
            }) {
                Ok(GameplayResponse::JoinEffect(effect)) => Ok(effect),
                Ok(other) => {
                    let message = format!("unexpected gameplay join payload: {other:?}");
                    match self.failures.handle_runtime_failure(
                        PluginKind::Gameplay,
                        &self.plugin_id,
                        &message,
                    ) {
                        PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                            Ok(GameplayJoinEffect::default())
                        }
                        PluginFailureAction::FailFast => Err(message),
                    }
                }
                Err(error) => match self.failures.handle_runtime_failure(
                    PluginKind::Gameplay,
                    &self.plugin_id,
                    &error,
                ) {
                    PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                        Ok(GameplayJoinEffect::default())
                    }
                    PluginFailureAction::FailFast => Err(error),
                },
            }
        })
    }

    fn handle_command(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        command: &mc_core::CoreCommand,
    ) -> Result<GameplayEffect, String> {
        let session = GameplaySessionSnapshot {
            phase: ConnectionPhase::Play,
            player_id: command.player_id(),
            entity_id: session.entity_id,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        if self.failures.is_active_quarantined(&self.plugin_id) {
            return Ok(GameplayEffect::default());
        }
        let generation = self.current_generation();
        with_gameplay_query(query, || {
            match generation.invoke(&GameplayRequest::HandleCommand {
                session,
                command: command.clone(),
            }) {
                Ok(GameplayResponse::Effect(effect)) => Ok(effect),
                Ok(other) => {
                    let message = format!("unexpected gameplay command payload: {other:?}");
                    match self.failures.handle_runtime_failure(
                        PluginKind::Gameplay,
                        &self.plugin_id,
                        &message,
                    ) {
                        PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                            Ok(GameplayEffect::default())
                        }
                        PluginFailureAction::FailFast => Err(message),
                    }
                }
                Err(error) => match self.failures.handle_runtime_failure(
                    PluginKind::Gameplay,
                    &self.plugin_id,
                    &error,
                ) {
                    PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                        Ok(GameplayEffect::default())
                    }
                    PluginFailureAction::FailFast => Err(error),
                },
            }
        })
    }

    fn handle_tick(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        player_id: mc_core::PlayerId,
        now_ms: u64,
    ) -> Result<GameplayEffect, String> {
        let session = GameplaySessionSnapshot {
            phase: ConnectionPhase::Play,
            player_id: Some(player_id),
            entity_id: session.entity_id,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        if self.failures.is_active_quarantined(&self.plugin_id) {
            return Ok(GameplayEffect::default());
        }
        let generation = self.current_generation();
        with_gameplay_query(query, || {
            match generation.invoke(&GameplayRequest::HandleTick { session, now_ms }) {
                Ok(GameplayResponse::Effect(effect)) => Ok(effect),
                Ok(other) => {
                    let message = format!("unexpected gameplay tick payload: {other:?}");
                    match self.failures.handle_runtime_failure(
                        PluginKind::Gameplay,
                        &self.plugin_id,
                        &message,
                    ) {
                        PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                            Ok(GameplayEffect::default())
                        }
                        PluginFailureAction::FailFast => Err(message),
                    }
                }
                Err(error) => match self.failures.handle_runtime_failure(
                    PluginKind::Gameplay,
                    &self.plugin_id,
                    &error,
                ) {
                    PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                        Ok(GameplayEffect::default())
                    }
                    PluginFailureAction::FailFast => Err(error),
                },
            }
        })
    }
}

impl GameplayProfileHandle for HotSwappableGameplayProfile {
    fn profile_id(&self) -> GameplayProfileId {
        Self::profile_id(self)
    }

    fn capability_set(&self) -> CapabilitySet {
        Self::capability_set(self)
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Self::plugin_generation_id(self)
    }

    fn session_closed(&self, session: &GameplaySessionSnapshot) -> Result<(), RuntimeError> {
        Self::session_closed(self, session)
    }
}

pub(crate) struct HotSwappableStorageProfile {
    plugin_id: String,
    pub(crate) generation: RwLock<Arc<StorageGeneration>>,
    pub(crate) reload_gate: RwLock<()>,
}

impl HotSwappableStorageProfile {
    pub(crate) const fn new(plugin_id: String, generation: Arc<StorageGeneration>) -> Self {
        Self {
            plugin_id,
            generation: RwLock::new(generation),
            reload_gate: RwLock::new(()),
        }
    }

    pub(crate) fn current_generation(&self) -> Result<Arc<StorageGeneration>, StorageError> {
        Ok(self
            .generation
            .read()
            .expect("storage generation lock should not be poisoned")
            .clone())
    }

    pub(crate) fn swap_generation(&self, generation: Arc<StorageGeneration>) {
        *self
            .generation
            .write()
            .expect("storage generation lock should not be poisoned") = generation;
    }

    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    fn capability_set(&self) -> CapabilitySet {
        self.current_generation()
            .map(|generation| generation.capabilities.clone())
            .unwrap_or_default()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.current_generation()
            .ok()
            .map(|generation| generation.generation_id)
    }

    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("storage reload gate should not be poisoned");
        let generation = self.current_generation()?;
        match generation.invoke(&StorageRequest::LoadSnapshot {
            world_dir: world_dir.display().to_string(),
        })? {
            StorageResponse::Snapshot(snapshot) => Ok(snapshot),
            other => Err(StorageError::Plugin(format!(
                "unexpected storage load_snapshot payload: {other:?}"
            ))),
        }
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("storage reload gate should not be poisoned");
        let generation = self.current_generation()?;
        match generation.invoke(&StorageRequest::SaveSnapshot {
            world_dir: world_dir.display().to_string(),
            snapshot: snapshot.clone(),
        })? {
            StorageResponse::Empty => Ok(()),
            other => Err(StorageError::Plugin(format!(
                "unexpected storage save_snapshot payload: {other:?}"
            ))),
        }
    }
}

impl StorageAdapter for HotSwappableStorageProfile {
    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        Self::load_snapshot(self, world_dir)
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        Self::save_snapshot(self, world_dir, snapshot)
    }
}

impl StorageProfileHandle for HotSwappableStorageProfile {
    fn plugin_id(&self) -> &str {
        Self::plugin_id(self)
    }

    fn capability_set(&self) -> CapabilitySet {
        Self::capability_set(self)
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Self::plugin_generation_id(self)
    }

    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        Self::load_snapshot(self, world_dir)
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        Self::save_snapshot(self, world_dir, snapshot)
    }
}

pub(crate) struct HotSwappableAuthProfile {
    plugin_id: String,
    pub(crate) generation: RwLock<Arc<AuthGeneration>>,
    failures: Arc<PluginFailureDispatch>,
}

impl HotSwappableAuthProfile {
    pub(crate) const fn new(
        plugin_id: String,
        generation: Arc<AuthGeneration>,
        failures: Arc<PluginFailureDispatch>,
    ) -> Self {
        Self {
            plugin_id,
            generation: RwLock::new(generation),
            failures,
        }
    }

    fn current_generation(&self) -> Result<Arc<AuthGeneration>, String> {
        Ok(self
            .generation
            .read()
            .expect("auth generation lock should not be poisoned")
            .clone())
    }

    pub(crate) fn swap_generation(&self, generation: Arc<AuthGeneration>) {
        *self
            .generation
            .write()
            .expect("auth generation lock should not be poisoned") = generation;
    }

    fn capability_set(&self) -> CapabilitySet {
        self.current_generation()
            .map(|generation| generation.capabilities.clone())
            .unwrap_or_default()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.current_generation()
            .ok()
            .map(|generation| generation.generation_id)
    }

    fn mode(&self) -> Result<AuthMode, RuntimeError> {
        self.current_generation()
            .map(|generation| generation.mode())
            .map_err(RuntimeError::Config)
    }

    fn capture_generation(&self) -> Result<Arc<AuthGeneration>, RuntimeError> {
        self.current_generation().map_err(RuntimeError::Config)
    }

    fn authenticate_offline(&self, username: &str) -> Result<PlayerId, RuntimeError> {
        match self.capture_generation()?.authenticate_offline(username) {
            Ok(player_id) => Ok(player_id),
            Err(RuntimeError::Config(message)) => {
                let _ = self.failures.handle_runtime_failure(
                    PluginKind::Auth,
                    &self.plugin_id,
                    &message,
                );
                Err(RuntimeError::Config(message))
            }
            Err(error) => Err(error),
        }
    }

    fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, RuntimeError> {
        match self
            .capture_generation()?
            .authenticate_online(username, server_hash)
        {
            Ok(player_id) => Ok(player_id),
            Err(RuntimeError::Config(message)) => {
                let _ = self.failures.handle_runtime_failure(
                    PluginKind::Auth,
                    &self.plugin_id,
                    &message,
                );
                Err(RuntimeError::Config(message))
            }
            Err(error) => Err(error),
        }
    }

    fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .capture_generation()?
            .authenticate_bedrock_offline(display_name)
        {
            Ok(result) => Ok(result),
            Err(RuntimeError::Config(message)) => {
                let _ = self.failures.handle_runtime_failure(
                    PluginKind::Auth,
                    &self.plugin_id,
                    &message,
                );
                Err(RuntimeError::Config(message))
            }
            Err(error) => Err(error),
        }
    }

    fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .capture_generation()?
            .authenticate_bedrock_xbl(chain_jwts, client_data_jwt)
        {
            Ok(result) => Ok(result),
            Err(RuntimeError::Config(message)) => {
                let _ = self.failures.handle_runtime_failure(
                    PluginKind::Auth,
                    &self.plugin_id,
                    &message,
                );
                Err(RuntimeError::Config(message))
            }
            Err(error) => Err(error),
        }
    }
}

impl super::AuthProfileHandle for HotSwappableAuthProfile {
    fn capability_set(&self) -> CapabilitySet {
        Self::capability_set(self)
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Self::plugin_generation_id(self)
    }

    fn mode(&self) -> Result<AuthMode, RuntimeError> {
        Self::mode(self)
    }

    fn capture_generation(&self) -> Result<Arc<dyn AuthGenerationHandle>, RuntimeError> {
        Self::capture_generation(self).map(|generation| generation as Arc<dyn AuthGenerationHandle>)
    }

    fn authenticate_offline(&self, username: &str) -> Result<PlayerId, RuntimeError> {
        Self::authenticate_offline(self, username)
    }

    fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, RuntimeError> {
        Self::authenticate_online(self, username, server_hash)
    }

    fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        Self::authenticate_bedrock_offline(self, display_name)
    }

    fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        Self::authenticate_bedrock_xbl(self, chain_jwts, client_data_jwt)
    }
}

pub(crate) struct ManagedProtocolPlugin {
    pub(crate) package: super::PluginPackage,
    pub(crate) adapter: Arc<HotSwappableProtocolAdapter>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}

pub(crate) struct ManagedGameplayPlugin {
    pub(crate) package: super::PluginPackage,
    pub(crate) profile_id: GameplayProfileId,
    pub(crate) profile: Arc<HotSwappableGameplayProfile>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}

pub(crate) struct ManagedStoragePlugin {
    pub(crate) package: super::PluginPackage,
    pub(crate) profile_id: String,
    pub(crate) profile: Arc<HotSwappableStorageProfile>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}

pub(crate) struct ManagedAuthPlugin {
    pub(crate) package: super::PluginPackage,
    pub(crate) profile_id: String,
    pub(crate) profile: Arc<HotSwappableAuthProfile>,
    pub(crate) loaded_at: SystemTime,
    pub(crate) active_loaded_at: SystemTime,
}
