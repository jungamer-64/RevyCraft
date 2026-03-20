use super::*;

pub struct StaticPluginManifest {
    pub plugin_id: &'static str,
    pub display_name: &'static str,
    pub plugin_kind: PluginKind,
    pub plugin_abi: PluginAbiVersion,
    pub min_host_abi: PluginAbiVersion,
    pub max_host_abi: PluginAbiVersion,
    pub capabilities: &'static [&'static str],
}

impl StaticPluginManifest {
    #[must_use]
    pub const fn protocol(plugin_id: &'static str, display_name: &'static str) -> Self {
        Self::protocol_with_capabilities(plugin_id, display_name, &[])
    }

    #[must_use]
    pub const fn protocol_with_capabilities(
        plugin_id: &'static str,
        display_name: &'static str,
        capabilities: &'static [&'static str],
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Protocol,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            capabilities,
        }
    }

    #[must_use]
    pub const fn gameplay(
        plugin_id: &'static str,
        display_name: &'static str,
        capabilities: &'static [&'static str],
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Gameplay,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            capabilities,
        }
    }

    #[must_use]
    pub const fn storage(
        plugin_id: &'static str,
        display_name: &'static str,
        capabilities: &'static [&'static str],
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Storage,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            capabilities,
        }
    }

    #[must_use]
    pub const fn auth(
        plugin_id: &'static str,
        display_name: &'static str,
        capabilities: &'static [&'static str],
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Auth,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            capabilities,
        }
    }
}

#[must_use]
pub fn manifest_from_static(manifest: &StaticPluginManifest) -> PluginManifestV1 {
    let (capabilities, capabilities_len) = if manifest.capabilities.is_empty() {
        (std::ptr::null(), 0)
    } else {
        let descriptors = manifest
            .capabilities
            .iter()
            .map(|capability| CapabilityDescriptorV1 {
                name: Utf8Slice::from_static_str(capability),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let leaked = Box::leak(descriptors);
        (leaked.as_ptr(), leaked.len())
    };
    PluginManifestV1 {
        plugin_id: Utf8Slice::from_static_str(manifest.plugin_id),
        display_name: Utf8Slice::from_static_str(manifest.display_name),
        plugin_kind: manifest.plugin_kind,
        plugin_abi: manifest.plugin_abi,
        min_host_abi: manifest.min_host_abi,
        max_host_abi: manifest.max_host_abi,
        capabilities,
        capabilities_len,
    }
}
