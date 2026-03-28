use super::{
    AdminSurfaceCapabilitySet, AdminSurfaceHostApiV1, AdminSurfaceInstanceDeclaration,
    AdminSurfacePauseView, AdminSurfacePluginInvokeV1Fn, AdminSurfaceProfileId,
    AdminSurfaceRequest, AdminSurfaceResponse, AdminSurfaceStatusView, Arc, AuthCapabilitySet,
    AuthGenerationHandle, AuthMode, AuthProfileId, AuthRequest, AuthResponse, BedrockAuthResult,
    BedrockListenerDescriptor, ByteSlice, GameplayCapabilitySet, GameplayPluginInvokeV3Fn,
    GameplayProfileId, GameplayRequest, GameplayResponse, Library, Mutex, OwnedBuffer, PlayerId,
    PluginBuildTag, PluginErrorCode, PluginFreeBufferFn, PluginGenerationId, PluginInvokeFn,
    ProtocolCapabilitySet, ProtocolDescriptor, ProtocolError, ProtocolRequest, ProtocolResponse,
    RuntimeError, StorageCapabilitySet, StorageError, StorageProfileId, StorageRequest,
    StorageResponse, admin_surface_host_api, decode_admin_surface_response, decode_auth_response,
    decode_gameplay_response, decode_protocol_response, decode_storage_response,
    encode_admin_surface_request, encode_auth_request, encode_gameplay_request,
    encode_protocol_request, encode_storage_request, take_owned_buffer,
};
use crate::config::PluginBufferLimits;

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
    pub(crate) capabilities: ProtocolCapabilitySet,
    pub(crate) buffer_limits: PluginBufferLimits,
    pub(crate) build_tag: Option<PluginBuildTag>,
    pub(crate) invoke: PluginInvokeFn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) _library_guard: Option<Arc<Mutex<Library>>>,
}

pub(crate) fn decode_plugin_error(
    plugin_id: &str,
    status: PluginErrorCode,
    free_buffer: PluginFreeBufferFn,
    error: OwnedBuffer,
    max_bytes: usize,
) -> String {
    if error.ptr.is_null() {
        format!("plugin `{plugin_id}` returned {status:?}")
    } else {
        let bytes = match take_owned_buffer(free_buffer, error, max_bytes, "plugin error buffer") {
            Ok(bytes) => bytes,
            Err(error) => {
                return format!("plugin `{plugin_id}` returned invalid error buffer: {error}");
            }
        };
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
                self.buffer_limits.metadata_bytes,
            )));
        }

        let response_bytes = take_owned_buffer(
            self.free_buffer,
            output,
            self.buffer_limits.protocol_response_bytes,
            "protocol response buffer",
        )
        .map_err(ProtocolError::Plugin)?;
        decode_protocol_response(request, &response_bytes)
            .map_err(|error| ProtocolError::Plugin(error.to_string()))
    }
}

#[derive(Clone)]
pub(crate) struct GameplayGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) profile_id: GameplayProfileId,
    pub(crate) capabilities: GameplayCapabilitySet,
    pub(crate) buffer_limits: PluginBufferLimits,
    pub(crate) build_tag: Option<PluginBuildTag>,
    pub(crate) invoke: GameplayPluginInvokeV3Fn,
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
                self.buffer_limits.metadata_bytes,
            ));
        }
        let response_bytes = take_owned_buffer(
            self.free_buffer,
            output,
            self.buffer_limits.gameplay_response_bytes,
            "gameplay response buffer",
        )?;
        decode_gameplay_response(request, &response_bytes).map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
pub(crate) struct StorageGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) profile_id: StorageProfileId,
    pub(crate) capabilities: StorageCapabilitySet,
    pub(crate) buffer_limits: PluginBufferLimits,
    pub(crate) build_tag: Option<PluginBuildTag>,
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
                self.buffer_limits.metadata_bytes,
            )));
        }
        let response_bytes = take_owned_buffer(
            self.free_buffer,
            output,
            self.buffer_limits.storage_response_bytes,
            "storage response buffer",
        )
        .map_err(StorageError::Plugin)?;
        decode_storage_response(request, &response_bytes)
            .map_err(|error| StorageError::Plugin(error.to_string()))
    }
}

#[derive(Clone)]
pub(crate) struct AuthGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) profile_id: AuthProfileId,
    pub(crate) mode: AuthMode,
    pub(crate) capabilities: AuthCapabilitySet,
    pub(crate) buffer_limits: PluginBufferLimits,
    pub(crate) build_tag: Option<PluginBuildTag>,
    pub(crate) invoke: PluginInvokeFn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) _library_guard: Option<Arc<Mutex<Library>>>,
}

#[derive(Clone)]
pub(crate) struct AdminSurfaceGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) profile_id: AdminSurfaceProfileId,
    pub(crate) capabilities: AdminSurfaceCapabilitySet,
    pub(crate) buffer_limits: PluginBufferLimits,
    pub(crate) build_tag: Option<PluginBuildTag>,
    pub(crate) invoke: AdminSurfacePluginInvokeV1Fn,
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
                self.buffer_limits.metadata_bytes,
            ));
        }
        let response_bytes = take_owned_buffer(
            self.free_buffer,
            output,
            self.buffer_limits.auth_response_bytes,
            "auth response buffer",
        )?;
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

impl AdminSurfaceGeneration {
    pub(crate) fn invoke(
        &self,
        request: &AdminSurfaceRequest,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceResponse, String> {
        let request_bytes =
            encode_admin_surface_request(request).map_err(|error| error.to_string())?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
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
                self.buffer_limits.metadata_bytes,
            ));
        }

        let response_bytes = take_owned_buffer(
            self.free_buffer,
            output,
            self.buffer_limits.admin_surface_response_bytes,
            "admin-surface response buffer",
        )?;
        decode_admin_surface_response(request, &response_bytes).map_err(|error| error.to_string())
    }

    pub(crate) fn declare_instance(
        &self,
        instance_id: &str,
        surface_config_path: Option<&std::path::Path>,
    ) -> Result<AdminSurfaceInstanceDeclaration, String> {
        match self.invoke(
            &AdminSurfaceRequest::DeclareInstance {
                instance_id: instance_id.to_string(),
                surface_config_path: surface_config_path.map(|path| path.display().to_string()),
            },
            admin_surface_host_api(),
        )? {
            AdminSurfaceResponse::Declared(declaration) => Ok(declaration),
            other => Err(format!(
                "unexpected admin-surface declaration payload: {other:?}"
            )),
        }
    }

    pub(crate) fn start(
        &self,
        instance_id: &str,
        surface_config_path: Option<&std::path::Path>,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, String> {
        match self.invoke(
            &AdminSurfaceRequest::Start {
                instance_id: instance_id.to_string(),
                surface_config_path: surface_config_path.map(|path| path.display().to_string()),
            },
            host_api,
        )? {
            AdminSurfaceResponse::Started(status) => Ok(status),
            other => Err(format!("unexpected admin-surface start payload: {other:?}")),
        }
    }

    pub(crate) fn pause_for_upgrade(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfacePauseView, String> {
        match self.invoke(
            &AdminSurfaceRequest::PauseForUpgrade {
                instance_id: instance_id.to_string(),
            },
            host_api,
        )? {
            AdminSurfaceResponse::Paused(view) => Ok(view),
            other => Err(format!("unexpected admin-surface pause payload: {other:?}")),
        }
    }

    pub(crate) fn resume_from_upgrade(
        &self,
        instance_id: &str,
        surface_config_path: Option<&std::path::Path>,
        resume_payload: &[u8],
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, String> {
        match self.invoke(
            &AdminSurfaceRequest::ResumeFromUpgrade {
                instance_id: instance_id.to_string(),
                surface_config_path: surface_config_path.map(|path| path.display().to_string()),
                resume_payload: resume_payload.to_vec(),
            },
            host_api,
        )? {
            AdminSurfaceResponse::Resumed(status) => Ok(status),
            other => Err(format!(
                "unexpected admin-surface resume payload: {other:?}"
            )),
        }
    }

    pub(crate) fn activate_after_upgrade_commit(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<(), String> {
        match self.invoke(
            &AdminSurfaceRequest::ActivateAfterUpgradeCommit {
                instance_id: instance_id.to_string(),
            },
            host_api,
        )? {
            AdminSurfaceResponse::Activated => Ok(()),
            other => Err(format!(
                "unexpected admin-surface activation payload: {other:?}"
            )),
        }
    }

    pub(crate) fn resume_after_upgrade_rollback(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<AdminSurfaceStatusView, String> {
        match self.invoke(
            &AdminSurfaceRequest::ResumeAfterUpgradeRollback {
                instance_id: instance_id.to_string(),
            },
            host_api,
        )? {
            AdminSurfaceResponse::ResumedAfterRollback(status) => Ok(status),
            other => Err(format!(
                "unexpected admin-surface rollback resume payload: {other:?}"
            )),
        }
    }

    pub(crate) fn shutdown(
        &self,
        instance_id: &str,
        host_api: AdminSurfaceHostApiV1,
    ) -> Result<(), String> {
        match self.invoke(
            &AdminSurfaceRequest::Shutdown {
                instance_id: instance_id.to_string(),
            },
            host_api,
        )? {
            AdminSurfaceResponse::ShutdownComplete => Ok(()),
            other => Err(format!(
                "unexpected admin-surface shutdown payload: {other:?}"
            )),
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
