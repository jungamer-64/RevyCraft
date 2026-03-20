use super::{
    Arc, AuthPluginApiV1, AuthRequest, AuthResponse, BedrockListenerDescriptor, CapabilitySet,
    GameplayGeneration, GameplayPluginApiV1, GameplayProfileId, GameplayRequest, GameplayResponse,
    GameplaySessionSnapshot, HashMap, HashSet, ManagedGameplayPlugin, ManagedProtocolPlugin,
    OwnedBuffer, PluginAbiVersion, PluginErrorCode, PluginKind, PluginManifestV1,
    ProtocolDescriptor, ProtocolGeneration, ProtocolPluginApiV1, ProtocolRequest, ProtocolResponse,
    RuntimeError, RuntimeReloadContext, StorageGeneration, StoragePluginApiV1, StorageRequest,
    StorageResponse, decode_auth_response, decode_gameplay_response, decode_plugin_error,
    decode_protocol_response, decode_storage_response, encode_auth_request,
    encode_gameplay_request, encode_protocol_request, encode_storage_request,
};
use crate::runtime::ProtocolReloadSession;
use mc_plugin_api::ProtocolSessionSnapshot;

#[derive(Clone, Debug)]
pub(super) struct DecodedManifest {
    pub(super) plugin_id: String,
    pub(super) plugin_kind: PluginKind,
    pub(super) plugin_abi: PluginAbiVersion,
    pub(super) min_host_abi: PluginAbiVersion,
    pub(super) max_host_abi: PluginAbiVersion,
    pub(super) capabilities: Vec<String>,
}

pub(super) fn decode_manifest(
    manifest: *const PluginManifestV1,
) -> Result<DecodedManifest, RuntimeError> {
    let manifest = unsafe {
        manifest
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("plugin manifest pointer was null".to_string()))?
    };
    let capabilities = if manifest.capabilities.is_null() || manifest.capabilities_len == 0 {
        Vec::new()
    } else {
        let descriptors =
            unsafe { std::slice::from_raw_parts(manifest.capabilities, manifest.capabilities_len) };
        let mut capabilities = Vec::with_capacity(descriptors.len());
        for descriptor in descriptors {
            capabilities.push(decode_utf8_slice(descriptor.name)?);
        }
        capabilities
    };
    Ok(DecodedManifest {
        plugin_id: decode_utf8_slice(manifest.plugin_id)?,
        plugin_kind: manifest.plugin_kind,
        plugin_abi: manifest.plugin_abi,
        min_host_abi: manifest.min_host_abi,
        max_host_abi: manifest.max_host_abi,
        capabilities,
    })
}

pub(super) fn decode_utf8_slice(slice: mc_plugin_api::Utf8Slice) -> Result<String, RuntimeError> {
    if slice.ptr.is_null() {
        return Err(RuntimeError::Config(
            "plugin utf8 slice was null".to_string(),
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) };
    String::from_utf8(bytes.to_vec()).map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(super) fn invoke_protocol(
    api: &ProtocolPluginApiV1,
    request: &ProtocolRequest,
) -> Result<ProtocolResponse, RuntimeError> {
    let request_bytes = encode_protocol_request(request)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
            mc_plugin_api::ByteSlice {
                ptr: request_bytes.as_ptr(),
                len: request_bytes.len(),
            },
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        let message = if error.ptr.is_null() {
            format!("plugin returned {status:?}")
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
            unsafe {
                (api.free_buffer)(error);
            }
            String::from_utf8(bytes).unwrap_or_else(|_| "plugin returned invalid utf-8".to_string())
        };
        return Err(RuntimeError::Config(message));
    }

    let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_protocol_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(super) fn invoke_gameplay(
    plugin_id: &str,
    api: &GameplayPluginApiV1,
    request: &GameplayRequest,
) -> Result<GameplayResponse, RuntimeError> {
    let request_bytes = encode_gameplay_request(request)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
            mc_plugin_api::ByteSlice {
                ptr: request_bytes.as_ptr(),
                len: request_bytes.len(),
            },
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        return Err(RuntimeError::Config(decode_plugin_error(
            plugin_id,
            status,
            api.free_buffer,
            error,
        )));
    }
    let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_gameplay_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(super) fn invoke_storage(
    plugin_id: &str,
    api: &StoragePluginApiV1,
    request: &StorageRequest,
) -> Result<StorageResponse, RuntimeError> {
    let request_bytes =
        encode_storage_request(request).map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
            mc_plugin_api::ByteSlice {
                ptr: request_bytes.as_ptr(),
                len: request_bytes.len(),
            },
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        return Err(RuntimeError::Config(decode_plugin_error(
            plugin_id,
            status,
            api.free_buffer,
            error,
        )));
    }

    let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_storage_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(super) fn invoke_auth(
    plugin_id: &str,
    api: &AuthPluginApiV1,
    request: &AuthRequest,
) -> Result<AuthResponse, RuntimeError> {
    let request_bytes =
        encode_auth_request(request).map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
            mc_plugin_api::ByteSlice {
                ptr: request_bytes.as_ptr(),
                len: request_bytes.len(),
            },
            &raw mut output,
            &raw mut error,
        )
    };
    if status != PluginErrorCode::Ok {
        return Err(RuntimeError::Config(decode_plugin_error(
            plugin_id,
            status,
            api.free_buffer,
            error,
        )));
    }

    let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_auth_response(request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(super) fn manifest_profile_id(
    manifest: &DecodedManifest,
    prefix: &str,
    plugin_id: &str,
    kind: &str,
) -> Result<String, RuntimeError> {
    manifest
        .capabilities
        .iter()
        .find_map(|capability| capability.strip_prefix(prefix))
        .map(ToString::to_string)
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "{kind} plugin `{plugin_id}` is missing {prefix}<id> manifest capability"
            ))
        })
}

pub(super) fn gameplay_profile_id_from_manifest(
    manifest: &DecodedManifest,
    plugin_id: &str,
) -> Result<GameplayProfileId, RuntimeError> {
    manifest
        .capabilities
        .iter()
        .find_map(|capability| capability.strip_prefix("gameplay.profile:"))
        .map(GameplayProfileId::new)
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "gameplay plugin `{plugin_id}` is missing gameplay.profile:<id> manifest capability"
            ))
        })
}

pub(super) fn require_manifest_capability(
    manifest: &DecodedManifest,
    capability: &str,
    plugin_id: &str,
    kind: &str,
) -> Result<(), RuntimeError> {
    if manifest.capabilities.iter().any(|item| item == capability) {
        Ok(())
    } else {
        Err(RuntimeError::Config(format!(
            "{kind} plugin `{plugin_id}` is missing {capability} capability"
        )))
    }
}

pub(super) fn expect_protocol_descriptor(
    plugin_id: &str,
    response: ProtocolResponse,
) -> Result<ProtocolDescriptor, RuntimeError> {
    match response {
        ProtocolResponse::Descriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected describe payload: {other:?}"
        ))),
    }
}

pub(super) fn expect_protocol_bedrock_listener_descriptor(
    plugin_id: &str,
    response: ProtocolResponse,
) -> Result<Option<BedrockListenerDescriptor>, RuntimeError> {
    match response {
        ProtocolResponse::BedrockListenerDescriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected bedrock listener payload: {other:?}"
        ))),
    }
}

pub(super) fn expect_protocol_capabilities(
    plugin_id: &str,
    response: ProtocolResponse,
) -> Result<CapabilitySet, RuntimeError> {
    match response {
        ProtocolResponse::CapabilitySet(capabilities) => Ok(capabilities),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected capability payload: {other:?}"
        ))),
    }
}

pub(super) fn expect_gameplay_descriptor(
    plugin_id: &str,
    response: GameplayResponse,
) -> Result<mc_plugin_api::GameplayDescriptor, RuntimeError> {
    match response {
        GameplayResponse::Descriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected gameplay describe payload: {other:?}"
        ))),
    }
}

pub(super) fn expect_gameplay_capabilities(
    plugin_id: &str,
    response: GameplayResponse,
) -> Result<CapabilitySet, RuntimeError> {
    match response {
        GameplayResponse::CapabilitySet(capabilities) => Ok(capabilities),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected gameplay capability payload: {other:?}"
        ))),
    }
}

pub(super) fn expect_storage_descriptor(
    plugin_id: &str,
    response: StorageResponse,
) -> Result<mc_plugin_api::StorageDescriptor, RuntimeError> {
    match response {
        StorageResponse::Descriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected storage describe payload: {other:?}"
        ))),
    }
}

pub(super) fn expect_storage_capabilities(
    plugin_id: &str,
    response: StorageResponse,
) -> Result<CapabilitySet, RuntimeError> {
    match response {
        StorageResponse::CapabilitySet(capabilities) => Ok(capabilities),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected storage capability payload: {other:?}"
        ))),
    }
}

pub(super) fn expect_auth_descriptor(
    plugin_id: &str,
    response: AuthResponse,
) -> Result<mc_plugin_api::AuthDescriptor, RuntimeError> {
    match response {
        AuthResponse::Descriptor(descriptor) => Ok(descriptor),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected auth describe payload: {other:?}"
        ))),
    }
}

pub(super) fn expect_auth_capabilities(
    plugin_id: &str,
    response: AuthResponse,
) -> Result<CapabilitySet, RuntimeError> {
    match response {
        AuthResponse::CapabilitySet(capabilities) => Ok(capabilities),
        other => Err(RuntimeError::Config(format!(
            "plugin `{plugin_id}` returned unexpected auth capability payload: {other:?}"
        ))),
    }
}

pub(super) fn migrate_gameplay_sessions(
    managed: &ManagedGameplayPlugin,
    generation: &Arc<GameplayGeneration>,
    runtime: &RuntimeReloadContext,
) -> Result<bool, RuntimeError> {
    let _reload_guard = managed
        .profile
        .reload_gate
        .write()
        .expect("gameplay reload gate should not be poisoned");
    let current_generation = managed.profile.current_generation();
    let relevant_sessions = runtime
        .gameplay_sessions
        .iter()
        .filter(|session| session.gameplay_profile.as_str() == managed.profile_id.as_str())
        .cloned()
        .collect::<Vec<_>>();

    for session in &relevant_sessions {
        let Some(blob) =
            export_gameplay_session_blob(&managed.package.plugin_id, &current_generation, session)
        else {
            return Ok(false);
        };
        if !import_gameplay_session_blob(&managed.package.plugin_id, generation, session, blob) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn protocol_reload_compatible(
    plugin_id: &str,
    current: &ProtocolGeneration,
    candidate: &ProtocolGeneration,
) -> bool {
    let compatible = current.descriptor.adapter_id == candidate.descriptor.adapter_id
        && current.descriptor.transport == candidate.descriptor.transport
        && current.descriptor.edition == candidate.descriptor.edition
        && current.descriptor.protocol_number == candidate.descriptor.protocol_number
        && current.descriptor.wire_format == candidate.descriptor.wire_format
        && current.bedrock_listener_descriptor == candidate.bedrock_listener_descriptor;
    if !compatible {
        eprintln!(
            "protocol reload rejected for `{plugin_id}` because route/listener metadata changed"
        );
    }
    compatible
}

pub(super) fn migrate_protocol_sessions(
    managed: &ManagedProtocolPlugin,
    generation: &Arc<ProtocolGeneration>,
    protocol_sessions: &[ProtocolReloadSession],
) -> Result<bool, RuntimeError> {
    let _reload_guard = managed
        .adapter
        .reload_gate
        .write()
        .expect("protocol reload gate should not be poisoned");
    let current_generation = managed
        .adapter
        .current_generation()
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let relevant_sessions = protocol_sessions
        .iter()
        .filter(|session| session.adapter_id == managed.package.plugin_id)
        .map(|session| session.session.clone())
        .collect::<Vec<_>>();

    for session in &relevant_sessions {
        let Some(blob) =
            export_protocol_session_blob(&managed.package.plugin_id, &current_generation, session)
        else {
            return Ok(false);
        };
        if !import_protocol_session_blob(&managed.package.plugin_id, generation, session, blob) {
            return Ok(false);
        }
    }

    managed
        .adapter
        .swap_generation_while_reloading(Arc::clone(generation));
    Ok(true)
}

fn export_gameplay_session_blob(
    plugin_id: &str,
    generation: &GameplayGeneration,
    session: &GameplaySessionSnapshot,
) -> Option<Vec<u8>> {
    match generation.invoke(&GameplayRequest::ExportSessionState {
        session: session.clone(),
    }) {
        Ok(GameplayResponse::SessionTransferBlob(blob)) => Some(blob),
        Ok(other) => {
            eprintln!(
                "gameplay reload export returned unexpected payload for `{plugin_id}`: {other:?}"
            );
            None
        }
        Err(error) => {
            eprintln!("gameplay reload export failed for `{plugin_id}`: {error}");
            None
        }
    }
}

fn export_protocol_session_blob(
    plugin_id: &str,
    generation: &ProtocolGeneration,
    session: &ProtocolSessionSnapshot,
) -> Option<Vec<u8>> {
    match generation.invoke(&ProtocolRequest::ExportSessionState {
        session: session.clone(),
    }) {
        Ok(ProtocolResponse::SessionTransferBlob(blob)) => Some(blob),
        Ok(other) => {
            eprintln!(
                "protocol reload export returned unexpected payload for `{plugin_id}`: {other:?}"
            );
            None
        }
        Err(error) => {
            eprintln!("protocol reload export failed for `{plugin_id}`: {error}");
            None
        }
    }
}

fn import_protocol_session_blob(
    plugin_id: &str,
    generation: &ProtocolGeneration,
    session: &ProtocolSessionSnapshot,
    blob: Vec<u8>,
) -> bool {
    match generation.invoke(&ProtocolRequest::ImportSessionState {
        session: session.clone(),
        blob,
    }) {
        Ok(ProtocolResponse::Empty) => true,
        Ok(other) => {
            eprintln!(
                "protocol reload import returned unexpected payload for `{plugin_id}`: {other:?}"
            );
            false
        }
        Err(error) => {
            eprintln!("protocol reload import failed for `{plugin_id}`: {error}");
            false
        }
    }
}

fn import_gameplay_session_blob(
    plugin_id: &str,
    generation: &GameplayGeneration,
    session: &GameplaySessionSnapshot,
    blob: Vec<u8>,
) -> bool {
    match generation.invoke(&GameplayRequest::ImportSessionState {
        session: session.clone(),
        blob,
    }) {
        Ok(GameplayResponse::Empty) => true,
        Ok(other) => {
            eprintln!(
                "gameplay reload import returned unexpected payload for `{plugin_id}`: {other:?}"
            );
            false
        }
        Err(error) => {
            eprintln!("gameplay reload import failed for `{plugin_id}`: {error}");
            false
        }
    }
}

pub(super) fn import_storage_runtime_state(
    plugin_id: &str,
    generation: &StorageGeneration,
    runtime: &RuntimeReloadContext,
) -> bool {
    match generation.invoke(&StorageRequest::ImportRuntimeState {
        world_dir: runtime.world_dir.display().to_string(),
        snapshot: runtime.snapshot.clone(),
    }) {
        Ok(StorageResponse::Empty) => true,
        Ok(other) => {
            eprintln!(
                "storage reload import returned unexpected payload for `{plugin_id}`: {other:?}"
            );
            false
        }
        Err(error) => {
            eprintln!("storage reload import failed for `{plugin_id}`: {error}");
            false
        }
    }
}

pub(super) fn ensure_known_profiles<T>(
    active_profiles: &HashMap<String, T>,
    requested_profiles: &HashSet<String>,
    profile_kind: &str,
) -> Result<(), RuntimeError> {
    for profile_id in requested_profiles {
        ensure_profile_known(active_profiles, profile_id, profile_kind)?;
    }
    Ok(())
}

pub(super) fn ensure_profile_known<T>(
    active_profiles: &HashMap<String, T>,
    profile_id: &str,
    profile_kind: &str,
) -> Result<(), RuntimeError> {
    if active_profiles.contains_key(profile_id) {
        return Ok(());
    }
    Err(RuntimeError::Config(format!(
        "unknown {profile_kind} profile `{profile_id}`"
    )))
}
