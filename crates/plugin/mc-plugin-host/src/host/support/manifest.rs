use super::{
    GameplayProfileId, PluginAbiVersion, PluginKind, PluginManifestV1, RuntimeError, Utf8Slice,
    decode_utf8_slice_with_limit, read_checked_slice,
};
use crate::config::PluginBufferLimits;
use mc_core::{AdminSurfaceProfileId, AuthProfileId, StorageProfileId};
use mc_plugin_api::abi::CapabilityDescriptorV1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProtocolManifestCapabilities {
    pub(crate) reload_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProfileManifestCapabilities<P> {
    pub(crate) profile_id: P,
    pub(crate) reload_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ManifestCapabilities {
    Protocol(ProtocolManifestCapabilities),
    Gameplay(ProfileManifestCapabilities<GameplayProfileId>),
    Storage(ProfileManifestCapabilities<StorageProfileId>),
    Auth(ProfileManifestCapabilities<AuthProfileId>),
    AdminSurface(ProfileManifestCapabilities<AdminSurfaceProfileId>),
}

#[derive(Clone, Debug)]
pub(crate) struct DecodedManifest {
    pub(crate) plugin_id: String,
    pub(crate) plugin_kind: PluginKind,
    pub(crate) plugin_abi: PluginAbiVersion,
    pub(crate) min_host_abi: PluginAbiVersion,
    pub(crate) max_host_abi: PluginAbiVersion,
    pub(crate) capabilities: ManifestCapabilities,
}

pub(crate) fn decode_manifest(
    manifest: *const PluginManifestV1,
    limits: PluginBufferLimits,
) -> Result<DecodedManifest, RuntimeError> {
    let manifest = unsafe {
        manifest
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("plugin manifest pointer was null".to_string()))?
    };
    let plugin_id = decode_utf8_slice(manifest.plugin_id, limits.metadata_bytes)?;
    let raw_capabilities = if manifest.capabilities.is_null() || manifest.capabilities_len == 0 {
        Vec::new()
    } else {
        let descriptors = read_checked_slice::<CapabilityDescriptorV1>(
            manifest.capabilities,
            manifest.capabilities_len,
            limits.metadata_bytes,
            "plugin manifest capabilities",
        )?;
        let mut capabilities = Vec::with_capacity(descriptors.len());
        for descriptor in descriptors {
            capabilities.push(decode_utf8_slice(descriptor.name, limits.metadata_bytes)?);
        }
        capabilities
    };
    let capabilities =
        decode_manifest_capabilities(&plugin_id, manifest.plugin_kind, &raw_capabilities)?;
    Ok(DecodedManifest {
        plugin_id,
        plugin_kind: manifest.plugin_kind,
        plugin_abi: manifest.plugin_abi,
        min_host_abi: manifest.min_host_abi,
        max_host_abi: manifest.max_host_abi,
        capabilities,
    })
}

pub(crate) fn decode_utf8_slice(
    slice: Utf8Slice,
    max_bytes: usize,
) -> Result<String, RuntimeError> {
    decode_utf8_slice_with_limit(slice, max_bytes, "plugin utf8 slice")
}

fn decode_manifest_capabilities(
    plugin_id: &str,
    plugin_kind: PluginKind,
    capabilities: &[String],
) -> Result<ManifestCapabilities, RuntimeError> {
    match plugin_kind {
        PluginKind::Protocol => Ok(ManifestCapabilities::Protocol(parse_protocol_manifest(
            plugin_id,
            capabilities,
        )?)),
        PluginKind::Gameplay => Ok(ManifestCapabilities::Gameplay(parse_profile_manifest(
            plugin_id,
            "gameplay",
            capabilities,
            "gameplay.profile:",
            "runtime.reload.gameplay",
            GameplayProfileId::new,
        )?)),
        PluginKind::Storage => Ok(ManifestCapabilities::Storage(parse_profile_manifest(
            plugin_id,
            "storage",
            capabilities,
            "storage.profile:",
            "runtime.reload.storage",
            StorageProfileId::new,
        )?)),
        PluginKind::Auth => Ok(ManifestCapabilities::Auth(parse_profile_manifest(
            plugin_id,
            "auth",
            capabilities,
            "auth.profile:",
            "runtime.reload.auth",
            AuthProfileId::new,
        )?)),
        PluginKind::AdminSurface => Ok(ManifestCapabilities::AdminSurface(parse_profile_manifest(
            plugin_id,
            "admin-surface",
            capabilities,
            "admin-surface.profile:",
            "runtime.reload.admin-surface",
            AdminSurfaceProfileId::new,
        )?)),
    }
}

fn parse_protocol_manifest(
    plugin_id: &str,
    capabilities: &[String],
) -> Result<ProtocolManifestCapabilities, RuntimeError> {
    let reload_token = "runtime.reload.protocol";
    let mut reload_required = false;
    for capability in capabilities {
        if capability == reload_token {
            if reload_required {
                return Err(RuntimeError::Config(format!(
                    "protocol plugin `{plugin_id}` duplicated {reload_token} manifest capability"
                )));
            }
            reload_required = true;
            continue;
        }
        return Err(RuntimeError::Config(format!(
            "protocol plugin `{plugin_id}` has unknown manifest capability `{capability}`"
        )));
    }
    if !reload_required {
        return Err(RuntimeError::Config(format!(
            "protocol plugin `{plugin_id}` is missing {reload_token} capability"
        )));
    }
    Ok(ProtocolManifestCapabilities { reload_required })
}

fn parse_profile_manifest<P>(
    plugin_id: &str,
    kind: &str,
    capabilities: &[String],
    profile_prefix: &str,
    reload_token: &str,
    make_profile: impl Fn(String) -> P,
) -> Result<ProfileManifestCapabilities<P>, RuntimeError> {
    let mut profile_id = None;
    let mut reload_required = false;
    for capability in capabilities {
        if capability == reload_token {
            if reload_required {
                return Err(RuntimeError::Config(format!(
                    "{kind} plugin `{plugin_id}` duplicated {reload_token} manifest capability"
                )));
            }
            reload_required = true;
            continue;
        }
        if let Some(raw_profile_id) = capability.strip_prefix(profile_prefix) {
            if raw_profile_id.is_empty() {
                return Err(RuntimeError::Config(format!(
                    "{kind} plugin `{plugin_id}` has empty {profile_prefix}<id> manifest capability"
                )));
            }
            if profile_id.is_some() {
                return Err(RuntimeError::Config(format!(
                    "{kind} plugin `{plugin_id}` duplicated {profile_prefix}<id> manifest capability"
                )));
            }
            profile_id = Some(make_profile(raw_profile_id.to_string()));
            continue;
        }
        return Err(RuntimeError::Config(format!(
            "{kind} plugin `{plugin_id}` has unknown manifest capability `{capability}`"
        )));
    }
    let Some(profile_id) = profile_id else {
        return Err(RuntimeError::Config(format!(
            "{kind} plugin `{plugin_id}` is missing {profile_prefix}<id> manifest capability"
        )));
    };
    if !reload_required {
        return Err(RuntimeError::Config(format!(
            "{kind} plugin `{plugin_id}` is missing {reload_token} capability"
        )));
    }
    Ok(ProfileManifestCapabilities {
        profile_id,
        reload_required,
    })
}
