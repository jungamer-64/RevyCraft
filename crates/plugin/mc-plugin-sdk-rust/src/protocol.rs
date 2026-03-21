use super::*;

pub use crate::{delegate_protocol_adapter, export_protocol_plugin};

pub trait RustProtocolPlugin: HandshakeProbe + ProtocolAdapter + Send + Sync + 'static {
    /// Exports protocol plugin session state into an opaque transfer blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin cannot serialize its protocol session state.
    fn export_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        Ok(Vec::new())
    }

    /// Imports protocol plugin session state from a previously exported blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the transfer blob is invalid for the current plugin.
    fn import_session_state(
        &self,
        _session: &ProtocolSessionSnapshot,
        _blob: &[u8],
    ) -> Result<(), ProtocolError> {
        Ok(())
    }
}

pub(crate) fn handle_protocol_request<P: RustProtocolPlugin>(
    plugin: &P,
    request: ProtocolRequest,
) -> Result<ProtocolResponse, String> {
    match request {
        ProtocolRequest::Describe => Ok(ProtocolResponse::Descriptor(plugin.descriptor())),
        ProtocolRequest::DescribeBedrockListener => Ok(
            ProtocolResponse::BedrockListenerDescriptor(plugin.bedrock_listener_descriptor()),
        ),
        ProtocolRequest::CapabilitySet => {
            Ok(ProtocolResponse::CapabilitySet(plugin.capability_set()))
        }
        ProtocolRequest::TryRoute { frame } => plugin
            .try_route(&frame)
            .map(ProtocolResponse::HandshakeIntent)
            .map_err(|error| error.to_string()),
        ProtocolRequest::DecodeStatus { frame } => plugin
            .decode_status(&frame)
            .map(ProtocolResponse::StatusRequest)
            .map_err(|error| error.to_string()),
        ProtocolRequest::DecodeLogin { frame } => plugin
            .decode_login(&frame)
            .map(ProtocolResponse::LoginRequest)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeStatusResponse { status } => plugin
            .encode_status_response(&status)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeStatusPong { payload } => plugin
            .encode_status_pong(payload)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeDisconnect { phase, reason } => plugin
            .encode_disconnect(phase, &reason)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeEncryptionRequest {
            server_id,
            public_key_der,
            verify_token,
        } => plugin
            .encode_encryption_request(&server_id, &public_key_der, &verify_token)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeNetworkSettings {
            compression_threshold,
        } => plugin
            .encode_network_settings(compression_threshold)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeLoginSuccess { player } => plugin
            .encode_login_success(&player)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::DecodePlay { player_id, frame } => plugin
            .decode_play(player_id, &frame)
            .map(ProtocolResponse::CoreCommand)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodePlayEvent { event, context } => plugin
            .encode_play_event(&event, &context)
            .map(ProtocolResponse::Frames)
            .map_err(|error| error.to_string()),
        ProtocolRequest::ExportSessionState { session } => plugin
            .export_session_state(&session)
            .map(ProtocolResponse::SessionTransferBlob)
            .map_err(|error| error.to_string()),
        ProtocolRequest::ImportSessionState { session, blob } => plugin
            .import_session_state(&session, &blob)
            .map(|()| ProtocolResponse::Empty)
            .map_err(|error| error.to_string()),
        ProtocolRequest::EncodeWireFrame { payload } => plugin
            .wire_codec()
            .encode_frame(&payload)
            .map(ProtocolResponse::Frame)
            .map_err(|error| error.to_string()),
        ProtocolRequest::TryDecodeWireFrame { buffer } => {
            let mut buffer = BytesMut::from(buffer.as_slice());
            let original_len = buffer.len();
            plugin
                .wire_codec()
                .try_decode_frame(&mut buffer)
                .map(|frame| {
                    ProtocolResponse::WireFrameDecodeResult(frame.map(|frame| {
                        WireFrameDecodeResult {
                            frame,
                            bytes_consumed: original_len - buffer.len(),
                        }
                    }))
                })
                .map_err(|error| error.to_string())
        }
    }
}
