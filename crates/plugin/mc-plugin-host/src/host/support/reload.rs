use super::{
    Arc, GameplayGeneration, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
    ManagedGameplayPlugin, ManagedProtocolPlugin, ProtocolGeneration, ProtocolReloadSession,
    ProtocolRequest, ProtocolResponse, ProtocolSessionSnapshot, RuntimeError, RuntimeReloadContext,
    StorageGeneration, StorageRequest, StorageResponse,
};

pub(crate) fn validate_gameplay_session_migration(
    managed: &ManagedGameplayPlugin,
    generation: &Arc<GameplayGeneration>,
    runtime: &RuntimeReloadContext,
) -> Result<bool, RuntimeError> {
    let relevant_sessions = runtime
        .gameplay_sessions
        .iter()
        .filter(|session| session.gameplay_profile.as_str() == managed.profile_id.as_str())
        .cloned()
        .collect::<Vec<_>>();

    Ok(managed.profile.with_reload_write(|current_generation| {
        for session in &relevant_sessions {
            let Some(blob) = export_gameplay_session_blob(
                &managed.package.plugin_id,
                &current_generation,
                session,
            ) else {
                return false;
            };
            if !import_gameplay_session_blob(&managed.package.plugin_id, generation, session, blob)
            {
                return false;
            }
        }
        true
    }))
}

pub(crate) fn protocol_reload_compatible(
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

pub(crate) fn validate_protocol_session_migration(
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

pub(crate) fn import_storage_runtime_state(
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
