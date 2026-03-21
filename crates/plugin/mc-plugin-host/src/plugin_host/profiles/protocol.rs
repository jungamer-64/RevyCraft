use super::{
    Arc, BedrockListenerDescriptor, BytesMut, CapabilitySet, ConnectionPhase, Edition,
    HandshakeIntent, HandshakeProbe, LoginRequest, PlayEncodingContext, PlayerId,
    PluginFailureDispatch, PluginGenerationId, PluginKind, ProtocolAdapter, ProtocolDescriptor,
    ProtocolError, ProtocolGeneration, ProtocolRequest, ProtocolResponse, RwLock, ServerListStatus,
    StatusRequest, TransportKind, WireCodec, WireFormatKind, WireFrameDecodeResult,
};

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
        player_id: PlayerId,
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
        context: &PlayEncodingContext,
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
