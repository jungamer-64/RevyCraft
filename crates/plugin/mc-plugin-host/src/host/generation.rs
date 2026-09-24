use super::{
    AdminSurfaceCapabilitySet, AdminSurfaceHostApiV9, AdminSurfaceInstanceDeclaration,
    AdminSurfacePauseView, AdminSurfacePluginInvokeV9Fn, AdminSurfaceProfileId,
    AdminSurfaceRequest, AdminSurfaceResponse, AdminSurfaceStatusView, Arc, AuthCapabilitySet,
    AuthGenerationHandle, AuthMode, AuthProfileId, AuthRequest, AuthResponse, BedrockAuthResult,
    BedrockListenerDescriptor, ByteSlice, GameplayCapabilitySet, GameplayPluginInvokeV9Fn,
    GameplayProfileId, GameplayRequest, GameplayResponse, Library, Mutex, OwnedBuffer, PlayerId,
    PluginBuildTag, PluginFreeBufferFn, PluginGenerationId, PluginInvokeFn, PluginStatus,
    ProtocolCapabilitySet, ProtocolDescriptor, ProtocolError, ProtocolRequest, ProtocolResponse,
    RuntimeError, StorageCapabilitySet, StorageError, StorageProfileId, StorageRequest,
    StorageResponse, admin_surface_host_api, decode_admin_surface_response, decode_auth_response,
    decode_gameplay_response, decode_protocol_response, decode_storage_response,
    encode_admin_surface_request, encode_auth_request, encode_gameplay_request,
    encode_protocol_request, encode_storage_request, gameplay_host_api, gameplay_metadata_host_api,
    release_owned_buffer, take_owned_buffer,
};
use crate::config::PluginBufferLimits;
use mc_plugin_abi::host::{PluginCreateInstanceFn, PluginDestroyInstanceFn};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;

/// One mutable object, independent of other loads of the same library. All callers
/// and returned buffers hold this lease; the object is destroyed before code unload.
pub(crate) struct PluginInstance {
    handle: NonNull<c_void>,
    destroy: PluginDestroyInstanceFn,
    _library: Arc<Mutex<Library>>,
}

// SAFETY: ABI creation requires an object movable between calling threads, including
// destruction. Only the last Arc consumes it, after all synchronous callers finish.
unsafe impl Send for PluginInstance {}
// SAFETY: ABI objects permit concurrent invocations. The handle is never mutated by
// the host, and every call/buffer owns an Arc preventing concurrent destruction.
unsafe impl Sync for PluginInstance {}

impl PluginInstance {
    /// # Errors
    /// Rejects constructor failure or a successful null handle. Any partial object
    /// is retired on failure. Library lifetime covers all cleanup.
    pub(crate) fn create(
        plugin_id: &str,
        create: PluginCreateInstanceFn,
        destroy: PluginDestroyInstanceFn,
        free_buffer: PluginFreeBufferFn,
        library: Arc<Mutex<Library>>,
        max_error_bytes: usize,
    ) -> Result<Arc<Self>, RuntimeError> {
        let mut handle = std::ptr::null_mut();
        let mut error = OwnedBuffer::empty();
        // SAFETY: callbacks belong to the validated table in the retained library;
        // both output pointers address exclusively borrowed, initialized storage.
        let status = unsafe { create(&raw mut handle, &raw mut error) };
        if status != PluginStatus::OK {
            // Release constructor-owned buffers while the partial object is alive.
            let message = decode_plugin_error(
                plugin_id,
                status,
                free_buffer,
                Arc::clone(&library),
                error,
                max_error_bytes,
            );
            // A non-null constructor output transfers ownership even on failure;
            // retire that partial object before reporting the failure.
            if !handle.is_null() {
                // SAFETY: a non-null constructor output belongs to this table;
                // no invocation has occurred and the library is still retained.
                unsafe { destroy(handle) };
            }
            return Err(RuntimeError::Config(message));
        }
        release_owned_buffer(free_buffer, Arc::clone(&library), error);
        let handle = NonNull::new(handle).ok_or_else(|| {
            RuntimeError::Config(format!(
                "plugin `{plugin_id}` returned a null instance on successful creation"
            ))
        })?;
        Ok(Arc::new(Self {
            handle,
            destroy,
            _library: library,
        }))
    }
}

impl Drop for PluginInstance {
    fn drop(&mut self) {
        // SAFETY: the final Arc owns the unique handle. All borrowers/returned
        // buffers have finished; the library field stays alive until after destroy.
        unsafe { (self.destroy)(self.handle.as_ptr()) };
    }
}

#[derive(Default)]
pub(crate) struct GenerationManager {
    bindings: Mutex<HashMap<PluginGenerationId, [u8; 32]>>,
}

impl GenerationManager {
    pub(crate) fn generation_for_binding(
        &self,
        artifact_sha256: [u8; 32],
        buffer_limits: PluginBufferLimits,
    ) -> Result<PluginGenerationId, RuntimeError> {
        let mut digest = Sha256::new();
        digest.update(b"RevyCraft plugin generation binding v1\0");
        digest.update(artifact_sha256);
        for limit in [
            buffer_limits.protocol_response_bytes,
            buffer_limits.gameplay_response_bytes,
            buffer_limits.storage_response_bytes,
            buffer_limits.auth_response_bytes,
            buffer_limits.admin_surface_response_bytes,
            buffer_limits.callback_payload_bytes,
            buffer_limits.metadata_bytes,
        ] {
            digest.update(
                u64::try_from(limit)
                    .map_err(|_| {
                        RuntimeError::Config(
                            "plugin buffer limit does not fit generation identity".to_string(),
                        )
                    })?
                    .to_be_bytes(),
            );
        }
        let binding_sha256: [u8; 32] = digest.finalize().into();
        let generation_id = PluginGenerationId(u64::from_be_bytes(
            binding_sha256[..8]
                .try_into()
                .expect("SHA-256 prefix has an exact u64 width"),
        ));
        let mut bindings = self
            .bindings
            .lock()
            .expect("plugin generation mutex should not be poisoned");
        match bindings.get(&generation_id) {
            Some(active) if *active == binding_sha256 => Ok(generation_id),
            Some(_) => Err(RuntimeError::Config(format!(
                "plugin binding generation id collision for {generation_id:?}"
            ))),
            None => {
                bindings.insert(generation_id, binding_sha256);
                Ok(generation_id)
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProtocolInvocation {
    pub(crate) invoke: PluginInvokeFn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) instance: Arc<PluginInstance>,
}

#[derive(Clone)]
pub(crate) struct StorageInvocation {
    pub(crate) invoke: PluginInvokeFn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) instance: Arc<PluginInstance>,
}

#[derive(Clone)]
pub(crate) struct AuthInvocation {
    pub(crate) invoke: PluginInvokeFn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) instance: Arc<PluginInstance>,
}

#[derive(Clone)]
pub(crate) struct GameplayInvocation {
    pub(crate) invoke: GameplayPluginInvokeV9Fn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) instance: Arc<PluginInstance>,
}

#[derive(Clone)]
pub(crate) struct AdminSurfaceInvocation {
    pub(crate) invoke: AdminSurfacePluginInvokeV9Fn,
    pub(crate) free_buffer: PluginFreeBufferFn,
    pub(crate) instance: Arc<PluginInstance>,
}

#[derive(Clone)]
pub(crate) struct ProtocolGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) artifact_sha256: [u8; 32],
    pub(crate) descriptor: ProtocolDescriptor,
    pub(crate) bedrock_listener_descriptor: Option<BedrockListenerDescriptor>,
    pub(crate) capabilities: ProtocolCapabilitySet,
    pub(crate) max_session_handoff_bytes: usize,
    pub(crate) buffer_limits: PluginBufferLimits,
    pub(crate) build_tag: Option<PluginBuildTag>,
    pub(crate) invocation: ProtocolInvocation,
}

pub(crate) fn decode_plugin_error<L>(
    plugin_id: &str,
    status: PluginStatus,
    free_buffer: PluginFreeBufferFn,
    generation_lease: L,
    error: OwnedBuffer,
    max_bytes: usize,
) -> String {
    let status = match mc_plugin_abi::raw::ValidatedPluginStatus::try_from(status) {
        Ok(status) => format!("{status:?}"),
        Err(error) => format!("invalid status tag {}", error.0),
    };
    let bytes = match take_owned_buffer(
        free_buffer,
        generation_lease,
        error,
        max_bytes,
        "plugin error buffer",
    ) {
        Ok(bytes) => bytes,
        Err(error) => {
            return format!("plugin `{plugin_id}` returned invalid error buffer: {error}");
        }
    };
    if bytes.is_empty() {
        format!("plugin `{plugin_id}` returned {status}")
    } else {
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

impl ProtocolInvocation {
    pub(crate) fn invoke(
        &self,
        plugin_id: &str,
        request: &ProtocolRequest,
        buffer_limits: PluginBufferLimits,
    ) -> Result<ProtocolResponse, String> {
        let request_bytes = encode_protocol_request(request).map_err(|error| error.to_string())?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        // SAFETY: instance retains the matching object and library for this entire
        // call. Input is borrowed readable storage; outputs are exclusive stack values.
        let status = unsafe {
            (self.invoke)(
                self.instance.handle.as_ptr(),
                ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginStatus::OK {
            release_owned_buffer(self.free_buffer, Arc::clone(&self.instance), output);
            return Err(decode_plugin_error(
                plugin_id,
                status,
                self.free_buffer,
                Arc::clone(&self.instance),
                error,
                buffer_limits.metadata_bytes,
            ));
        }
        release_owned_buffer(self.free_buffer, Arc::clone(&self.instance), error);

        let response_bytes = take_owned_buffer(
            self.free_buffer,
            Arc::clone(&self.instance),
            output,
            buffer_limits.protocol_response_bytes,
            "protocol response buffer",
        )?;
        decode_protocol_response(request, &response_bytes).map_err(|error| error.to_string())
    }
}

impl ProtocolGeneration {
    pub(crate) fn invoke(
        &self,
        request: &ProtocolRequest,
    ) -> Result<ProtocolResponse, ProtocolError> {
        self.invocation
            .invoke(&self.plugin_id, request, self.buffer_limits)
            .map_err(ProtocolError::Plugin)
    }
}

#[derive(Clone)]
pub(crate) struct GameplayGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) artifact_sha256: [u8; 32],
    pub(crate) profile_id: GameplayProfileId,
    pub(crate) capabilities: GameplayCapabilitySet,
    pub(crate) max_session_handoff_bytes: usize,
    pub(crate) buffer_limits: PluginBufferLimits,
    pub(crate) build_tag: Option<PluginBuildTag>,
    pub(crate) invocation: GameplayInvocation,
}

impl GameplayInvocation {
    pub(crate) fn invoke(
        &self,
        plugin_id: &str,
        request: &GameplayRequest,
        buffer_limits: PluginBufferLimits,
        host_api: mc_plugin_abi::host::GameplayHostApiV9,
    ) -> Result<GameplayResponse, String> {
        let request_bytes = encode_gameplay_request(request).map_err(|error| error.to_string())?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        // SAFETY: instance retains the matching object and library for this entire
        // call. Input is borrowed readable storage; outputs are exclusive stack values.
        let status = unsafe {
            (self.invoke)(
                self.instance.handle.as_ptr(),
                ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw const host_api,
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginStatus::OK {
            release_owned_buffer(self.free_buffer, Arc::clone(&self.instance), output);
            return Err(decode_plugin_error(
                plugin_id,
                status,
                self.free_buffer,
                Arc::clone(&self.instance),
                error,
                buffer_limits.metadata_bytes,
            ));
        }
        release_owned_buffer(self.free_buffer, Arc::clone(&self.instance), error);
        let response_bytes = take_owned_buffer(
            self.free_buffer,
            Arc::clone(&self.instance),
            output,
            buffer_limits.gameplay_response_bytes,
            "gameplay response buffer",
        )?;
        decode_gameplay_response(request, &response_bytes).map_err(|error| error.to_string())
    }
}

impl GameplayGeneration {
    pub(crate) fn invoke(&self, request: &GameplayRequest) -> Result<GameplayResponse, String> {
        self.invocation.invoke(
            &self.plugin_id,
            request,
            self.buffer_limits,
            gameplay_metadata_host_api(),
        )
    }

    pub(crate) fn invoke_with_scope(
        &self,
        request: &GameplayRequest,
        scope: &mut super::GameplayInvocationScope,
    ) -> Result<GameplayResponse, String> {
        self.invocation.invoke(
            &self.plugin_id,
            request,
            self.buffer_limits,
            gameplay_host_api(scope),
        )
    }
}

#[derive(Clone)]
pub(crate) struct StorageGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) artifact_sha256: [u8; 32],
    pub(crate) profile_id: StorageProfileId,
    pub(crate) capabilities: StorageCapabilitySet,
    pub(crate) buffer_limits: PluginBufferLimits,
    pub(crate) build_tag: Option<PluginBuildTag>,
    pub(crate) invocation: StorageInvocation,
}

impl StorageInvocation {
    pub(crate) fn invoke(
        &self,
        plugin_id: &str,
        request: &StorageRequest,
        buffer_limits: PluginBufferLimits,
    ) -> Result<StorageResponse, String> {
        let request_bytes = encode_storage_request(request).map_err(|error| error.to_string())?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        // SAFETY: instance retains the matching object and library for this entire
        // call. Input is borrowed readable storage; outputs are exclusive stack values.
        let status = unsafe {
            (self.invoke)(
                self.instance.handle.as_ptr(),
                ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginStatus::OK {
            release_owned_buffer(self.free_buffer, Arc::clone(&self.instance), output);
            return Err(decode_plugin_error(
                plugin_id,
                status,
                self.free_buffer,
                Arc::clone(&self.instance),
                error,
                buffer_limits.metadata_bytes,
            ));
        }
        release_owned_buffer(self.free_buffer, Arc::clone(&self.instance), error);
        let response_bytes = take_owned_buffer(
            self.free_buffer,
            Arc::clone(&self.instance),
            output,
            buffer_limits.storage_response_bytes,
            "storage response buffer",
        )?;
        decode_storage_response(request, &response_bytes).map_err(|error| error.to_string())
    }
}

impl StorageGeneration {
    pub(crate) fn invoke(&self, request: &StorageRequest) -> Result<StorageResponse, StorageError> {
        self.invocation
            .invoke(&self.plugin_id, request, self.buffer_limits)
            .map_err(StorageError::Plugin)
    }
}

#[derive(Clone)]
pub(crate) struct AuthGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) artifact_sha256: [u8; 32],
    pub(crate) profile_id: AuthProfileId,
    pub(crate) mode: AuthMode,
    pub(crate) capabilities: AuthCapabilitySet,
    pub(crate) buffer_limits: PluginBufferLimits,
    pub(crate) build_tag: Option<PluginBuildTag>,
    pub(crate) invocation: AuthInvocation,
}

#[derive(Clone)]
pub(crate) struct AdminSurfaceGeneration {
    pub(crate) generation_id: PluginGenerationId,
    pub(crate) plugin_id: String,
    pub(crate) artifact_sha256: [u8; 32],
    pub(crate) profile_id: AdminSurfaceProfileId,
    pub(crate) capabilities: AdminSurfaceCapabilitySet,
    pub(crate) buffer_limits: PluginBufferLimits,
    pub(crate) build_tag: Option<PluginBuildTag>,
    pub(crate) invocation: AdminSurfaceInvocation,
}

impl AuthInvocation {
    pub(crate) fn invoke(
        &self,
        plugin_id: &str,
        request: &AuthRequest,
        buffer_limits: PluginBufferLimits,
    ) -> Result<AuthResponse, String> {
        let request_bytes = encode_auth_request(request).map_err(|error| error.to_string())?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        // SAFETY: instance retains the matching object and library for this entire
        // call. Input is borrowed readable storage; outputs are exclusive stack values.
        let status = unsafe {
            (self.invoke)(
                self.instance.handle.as_ptr(),
                ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginStatus::OK {
            release_owned_buffer(self.free_buffer, Arc::clone(&self.instance), output);
            return Err(decode_plugin_error(
                plugin_id,
                status,
                self.free_buffer,
                Arc::clone(&self.instance),
                error,
                buffer_limits.metadata_bytes,
            ));
        }
        release_owned_buffer(self.free_buffer, Arc::clone(&self.instance), error);

        let response_bytes = take_owned_buffer(
            self.free_buffer,
            Arc::clone(&self.instance),
            output,
            buffer_limits.auth_response_bytes,
            "auth response buffer",
        )?;
        decode_auth_response(request, &response_bytes).map_err(|error| error.to_string())
    }
}

impl AdminSurfaceInvocation {
    pub(crate) fn invoke(
        &self,
        plugin_id: &str,
        request: &AdminSurfaceRequest,
        buffer_limits: PluginBufferLimits,
        host_api: AdminSurfaceHostApiV9,
    ) -> Result<AdminSurfaceResponse, String> {
        let request_bytes =
            encode_admin_surface_request(request).map_err(|error| error.to_string())?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        // SAFETY: instance retains the matching object and library for this entire
        // call. Input is borrowed readable storage; outputs are exclusive stack values.
        let status = unsafe {
            (self.invoke)(
                self.instance.handle.as_ptr(),
                ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw const host_api,
                &raw mut output,
                &raw mut error,
            )
        };
        if status != PluginStatus::OK {
            release_owned_buffer(self.free_buffer, Arc::clone(&self.instance), output);
            return Err(decode_plugin_error(
                plugin_id,
                status,
                self.free_buffer,
                Arc::clone(&self.instance),
                error,
                buffer_limits.metadata_bytes,
            ));
        }
        release_owned_buffer(self.free_buffer, Arc::clone(&self.instance), error);

        let response_bytes = take_owned_buffer(
            self.free_buffer,
            Arc::clone(&self.instance),
            output,
            buffer_limits.admin_surface_response_bytes,
            "admin-surface response buffer",
        )?;
        decode_admin_surface_response(request, &response_bytes).map_err(|error| error.to_string())
    }
}

impl AuthGeneration {
    fn invoke(&self, request: &AuthRequest) -> Result<AuthResponse, String> {
        self.invocation
            .invoke(&self.plugin_id, request, self.buffer_limits)
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
        host_api: AdminSurfaceHostApiV9,
    ) -> Result<AdminSurfaceResponse, String> {
        self.invocation
            .invoke(&self.plugin_id, request, self.buffer_limits, host_api)
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
        host_api: AdminSurfaceHostApiV9,
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
        host_api: AdminSurfaceHostApiV9,
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
        host_api: AdminSurfaceHostApiV9,
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
        host_api: AdminSurfaceHostApiV9,
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
        host_api: AdminSurfaceHostApiV9,
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
        host_api: AdminSurfaceHostApiV9,
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

#[cfg(test)]
mod generation_identity_tests {
    use super::*;

    #[test]
    fn generation_identity_tracks_artifact_and_validated_binding() {
        let generations = GenerationManager::default();
        let artifact = [7_u8; 32];
        let limits = PluginBufferLimits::default();
        let mut changed_limits = limits;
        changed_limits.storage_response_bytes = limits.storage_response_bytes / 2;

        let first_id = generations
            .generation_for_binding(artifact, limits)
            .unwrap();
        assert_eq!(
            generations
                .generation_for_binding(artifact, limits)
                .unwrap(),
            first_id
        );
        assert_ne!(
            generations
                .generation_for_binding(artifact, changed_limits)
                .unwrap(),
            first_id
        );
        assert_ne!(
            generations
                .generation_for_binding([8_u8; 32], limits)
                .unwrap(),
            first_id
        );
    }
}
