use super::{
    AdminRequest, AdminResponse, AdminUiInput, AdminUiOutput, AdminUiPluginInvokeV1Fn, Arc,
    AuthGenerationHandle, AuthMode, AuthRequest, AuthResponse, BedrockAuthResult,
    BedrockListenerDescriptor, ByteSlice, CapabilitySet, GameplayPluginInvokeV2Fn,
    GameplayProfileId, GameplayRequest, GameplayResponse, Library, Mutex, OwnedBuffer, PlayerId,
    PluginErrorCode, PluginFreeBufferFn, PluginGenerationId, PluginInvokeFn, ProtocolDescriptor,
    ProtocolError, ProtocolRequest, ProtocolResponse, RuntimeError, StorageError, StorageRequest,
    StorageResponse, admin_ui_host_api, decode_admin_ui_output, decode_auth_response,
    decode_gameplay_response, decode_protocol_response, decode_storage_response,
    encode_admin_ui_input, encode_auth_request, encode_gameplay_request, encode_protocol_request,
    encode_storage_request,
};

#[derive(Default)]
pub(crate) struct GenerationManager {
    next_generation_id: Mutex<u64>,
}

impl GenerationManager {
    pub(crate) fn next_generation_id(&self) -> PluginGenerationId {
        let mut next = self
            .next_generation_id
            .lock()
            .expect("plugin generation mutex should not be poisoned");
        let generation = PluginGenerationId(*next);
        *next = next.saturating_add(1);
        generation
    }
}

#[derive(Clone)]
pub(crate) struct ProtocolGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) descriptor: ProtocolDescriptor,
    pub(crate) bedrock_listener_descriptor: Option<BedrockListenerDescriptor>,
    pub(crate) capabilities: CapabilitySet,
    pub(crate) invoke: PluginInvokeFn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) _library_guard: Option<Arc<Mutex<Library>>>,
}

pub(crate) fn decode_plugin_error(
    plugin_id: &str,
    status: PluginErrorCode,
    free_buffer: PluginFreeBufferFn,
    error: OwnedBuffer,
) -> String {
    if error.ptr.is_null() {
        format!("plugin `{plugin_id}` returned {status:?}")
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
        unsafe {
            (free_buffer)(error);
        }
        String::from_utf8(bytes)
            .unwrap_or_else(|_| format!("plugin `{plugin_id}` returned invalid utf-8"))
    }
}

pub(crate) fn write_owned_buffer(output: *mut OwnedBuffer, mut bytes: Vec<u8>) {
    if output.is_null() {
        return;
    }
    unsafe {
        *output = OwnedBuffer {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            cap: bytes.capacity(),
        };
        std::mem::forget(bytes);
    }
}

impl ProtocolGeneration {
    pub(crate) fn invoke(
        &self,
        request: &ProtocolRequest,
    ) -> Result<ProtocolResponse, ProtocolError> {
        let request_bytes = encode_protocol_request(request)
            .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            (self.invoke)(
                ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            return Err(ProtocolError::Plugin(decode_plugin_error(
                &self.plugin_id,
                status,
                self.free_buffer,
                error,
            )));
        }

        let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
        unsafe {
            (self.free_buffer)(output);
        }
        decode_protocol_response(request, &response_bytes)
            .map_err(|error| ProtocolError::Plugin(error.to_string()))
    }
}

#[derive(Clone)]
pub(crate) struct GameplayGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) profile_id: GameplayProfileId,
    pub(crate) capabilities: CapabilitySet,
    pub(crate) invoke: GameplayPluginInvokeV2Fn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) _library_guard: Option<Arc<Mutex<Library>>>,
}

impl GameplayGeneration {
    pub(crate) fn invoke(&self, request: &GameplayRequest) -> Result<GameplayResponse, String> {
        let request_bytes = encode_gameplay_request(request).map_err(|error| error.to_string())?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let host_api = super::gameplay_host_api();
        let status = unsafe {
            (self.invoke)(
                ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw const host_api,
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            return Err(decode_plugin_error(
                &self.plugin_id,
                status,
                self.free_buffer,
                error,
            ));
        }
        let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
        unsafe {
            (self.free_buffer)(output);
        }
        decode_gameplay_response(request, &response_bytes).map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
pub(crate) struct StorageGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) profile_id: String,
    pub(crate) capabilities: CapabilitySet,
    pub(crate) invoke: PluginInvokeFn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) _library_guard: Option<Arc<Mutex<Library>>>,
}

impl StorageGeneration {
    pub(crate) fn invoke(&self, request: &StorageRequest) -> Result<StorageResponse, StorageError> {
        let request_bytes = encode_storage_request(request)
            .map_err(|error| StorageError::Plugin(error.to_string()))?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            (self.invoke)(
                ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            return Err(StorageError::Plugin(decode_plugin_error(
                &self.plugin_id,
                status,
                self.free_buffer,
                error,
            )));
        }
        let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
        unsafe {
            (self.free_buffer)(output);
        }
        decode_storage_response(request, &response_bytes)
            .map_err(|error| StorageError::Plugin(error.to_string()))
    }
}

#[derive(Clone)]
pub(crate) struct AuthGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) profile_id: String,
    pub(crate) mode: AuthMode,
    pub(crate) capabilities: CapabilitySet,
    pub(crate) invoke: PluginInvokeFn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) _library_guard: Option<Arc<Mutex<Library>>>,
}

#[derive(Clone)]
pub(crate) struct AdminUiGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) profile_id: String,
    pub(crate) capabilities: CapabilitySet,
    pub(crate) invoke: AdminUiPluginInvokeV1Fn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) _library_guard: Option<Arc<Mutex<Library>>>,
}

impl AuthGeneration {
    fn invoke(&self, request: &AuthRequest) -> Result<AuthResponse, String> {
        let request_bytes = encode_auth_request(request).map_err(|error| error.to_string())?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            (self.invoke)(
                ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            return Err(decode_plugin_error(
                &self.plugin_id,
                status,
                self.free_buffer,
                error,
            ));
        }
        let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
        unsafe {
            (self.free_buffer)(output);
        }
        decode_auth_response(request, &response_bytes).map_err(|error| error.to_string())
    }

    pub(crate) const fn mode(&self) -> AuthMode {
        self.mode
    }

    pub(crate) fn authenticate_offline(&self, username: &str) -> Result<PlayerId, RuntimeError> {
        match self
            .invoke(&AuthRequest::AuthenticateOffline {
                username: username.to_string(),
            })
            .map_err(RuntimeError::Config)?
        {
            AuthResponse::AuthenticatedPlayer(player_id) => Ok(player_id),
            other => Err(RuntimeError::Config(format!(
                "unexpected auth authenticate_offline payload: {other:?}"
            ))),
        }
    }

    pub(crate) fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, RuntimeError> {
        match self
            .invoke(&AuthRequest::AuthenticateOnline {
                username: username.to_string(),
                server_hash: server_hash.to_string(),
            })
            .map_err(RuntimeError::Config)?
        {
            AuthResponse::AuthenticatedPlayer(player_id) => Ok(player_id),
            other => Err(RuntimeError::Config(format!(
                "unexpected auth authenticate_online payload: {other:?}"
            ))),
        }
    }

    pub(crate) fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .invoke(&AuthRequest::AuthenticateBedrockOffline {
                display_name: display_name.to_string(),
            })
            .map_err(RuntimeError::Config)?
        {
            AuthResponse::AuthenticatedBedrockPlayer(result) => Ok(result),
            other => Err(RuntimeError::Config(format!(
                "unexpected auth authenticate_bedrock_offline payload: {other:?}"
            ))),
        }
    }

    pub(crate) fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .invoke(&AuthRequest::AuthenticateBedrockXbl {
                chain_jwts: chain_jwts.to_vec(),
                client_data_jwt: client_data_jwt.to_string(),
            })
            .map_err(RuntimeError::Config)?
        {
            AuthResponse::AuthenticatedBedrockPlayer(result) => Ok(result),
            other => Err(RuntimeError::Config(format!(
                "unexpected auth authenticate_bedrock_xbl payload: {other:?}"
            ))),
        }
    }
}

impl AdminUiGeneration {
    fn invoke(&self, request: &AdminUiInput) -> Result<AdminUiOutput, String> {
        let request_bytes = encode_admin_ui_input(request).map_err(|error| error.to_string())?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let host_api = admin_ui_host_api();
        let status = unsafe {
            (self.invoke)(
                ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw const host_api,
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            return Err(decode_plugin_error(
                &self.plugin_id,
                status,
                self.free_buffer,
                error,
            ));
        }

        let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
        unsafe {
            (self.free_buffer)(output);
        }
        decode_admin_ui_output(request, &response_bytes).map_err(|error| error.to_string())
    }

    pub(crate) fn parse_line(&self, line: &str) -> Result<AdminRequest, String> {
        match self.invoke(&AdminUiInput::ParseLine {
            line: line.to_string(),
        })? {
            AdminUiOutput::ParsedRequest(request) => Ok(request),
            other => Err(format!("unexpected admin-ui parse payload: {other:?}")),
        }
    }

    pub(crate) fn render_response(&self, response: &AdminResponse) -> Result<String, String> {
        match self.invoke(&AdminUiInput::RenderResponse {
            response: response.clone(),
        })? {
            AdminUiOutput::RenderedText(text) => Ok(text),
            other => Err(format!("unexpected admin-ui render payload: {other:?}")),
        }
    }
}

impl AuthGenerationHandle for AuthGeneration {
    fn generation_id(&self) -> PluginGenerationId {
        self.generation_id
    }

    fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, RuntimeError> {
        Self::authenticate_online(self, username, server_hash)
    }
}
