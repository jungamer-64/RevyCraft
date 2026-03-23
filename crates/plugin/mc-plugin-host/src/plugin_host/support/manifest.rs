use super::{
    AdminUiProfileId, GameplayProfileId, PluginAbiVersion, PluginKind, PluginManifestV1,
    RuntimeError, Utf8Slice,
};

#[derive(Clone, Debug)]
pub(crate) struct DecodedManifest {
    pub(crate) plugin_id: String,
    pub(crate) plugin_kind: PluginKind,
    pub(crate) plugin_abi: PluginAbiVersion,
    pub(crate) min_host_abi: PluginAbiVersion,
    pub(crate) max_host_abi: PluginAbiVersion,
    pub(crate) capabilities: Vec<String>,
}

pub(crate) fn decode_manifest(
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

pub(crate) fn decode_utf8_slice(slice: Utf8Slice) -> Result<String, RuntimeError> {
    if slice.ptr.is_null() {
        return Err(RuntimeError::Config(
            "plugin utf8 slice was null".to_string(),
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) };
    String::from_utf8(bytes.to_vec()).map_err(|error| RuntimeError::Config(error.to_string()))
}

pub(crate) fn manifest_profile_id<P>(
    manifest: &DecodedManifest,
    prefix: &str,
    plugin_id: &str,
    kind: &str,
) -> Result<P, RuntimeError>
where
    P: From<String>,
{
    manifest
        .capabilities
        .iter()
        .find_map(|capability| capability.strip_prefix(prefix))
        .map(|profile_id| P::from(profile_id.to_string()))
        .ok_or_else(|| {
            RuntimeError::Config(format!(
                "{kind} plugin `{plugin_id}` is missing {prefix}<id> manifest capability"
            ))
        })
}

pub(crate) fn gameplay_profile_id_from_manifest(
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

pub(crate) fn admin_ui_profile_id_from_manifest(
    manifest: &DecodedManifest,
    plugin_id: &str,
) -> Result<AdminUiProfileId, RuntimeError> {
    manifest_profile_id(manifest, "admin-ui.profile:", plugin_id, "admin-ui")
}

pub(crate) fn require_manifest_capability(
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
