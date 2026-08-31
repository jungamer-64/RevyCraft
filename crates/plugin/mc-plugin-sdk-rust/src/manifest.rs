use super::*;

pub enum StaticPluginCapabilities {
    Protocol,
    Gameplay { profile_id: &'static str },
    Storage { profile_id: &'static str },
    Auth { profile_id: &'static str },
    AdminSurface { profile_id: &'static str },
}

pub struct StaticPluginManifest {
    pub plugin_id: &'static str,
    pub display_name: &'static str,
    pub plugin_kind: PluginKind,
    pub plugin_abi: PluginAbiVersion,
    pub min_host_abi: PluginAbiVersion,
    pub max_host_abi: PluginAbiVersion,
    pub max_session_handoff_bytes: usize,
    pub capabilities: StaticPluginCapabilities,
}

impl StaticPluginManifest {
    #[must_use]
    pub const fn protocol(plugin_id: &'static str, display_name: &'static str) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Protocol,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            max_session_handoff_bytes: 0,
            capabilities: StaticPluginCapabilities::Protocol,
        }
    }

    #[must_use]
    pub const fn gameplay(
        plugin_id: &'static str,
        display_name: &'static str,
        profile_id: &'static str,
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Gameplay,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            max_session_handoff_bytes: 0,
            capabilities: StaticPluginCapabilities::Gameplay { profile_id },
        }
    }

    #[must_use]
    pub const fn storage(
        plugin_id: &'static str,
        display_name: &'static str,
        profile_id: &'static str,
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Storage,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            max_session_handoff_bytes: 0,
            capabilities: StaticPluginCapabilities::Storage { profile_id },
        }
    }

    #[must_use]
    pub const fn auth(
        plugin_id: &'static str,
        display_name: &'static str,
        profile_id: &'static str,
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::Auth,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            max_session_handoff_bytes: 0,
            capabilities: StaticPluginCapabilities::Auth { profile_id },
        }
    }

    #[must_use]
    pub const fn admin_surface(
        plugin_id: &'static str,
        display_name: &'static str,
        profile_id: &'static str,
    ) -> Self {
        Self {
            plugin_id,
            display_name,
            plugin_kind: PluginKind::AdminSurface,
            plugin_abi: CURRENT_PLUGIN_ABI,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            max_session_handoff_bytes: 0,
            capabilities: StaticPluginCapabilities::AdminSurface { profile_id },
        }
    }

    #[must_use]
    pub const fn with_max_session_handoff_bytes(mut self, limit: usize) -> Self {
        self.max_session_handoff_bytes = limit;
        self
    }
}

pub struct ExportedPluginManifest {
    manifest: PluginManifestV9,
    _capability_names: Box<[String]>,
    _capability_descriptors: Box<[CapabilityDescriptorV9]>,
}

// SAFETY: the raw manifest pointers refer only to the immutable boxed allocations owned by this
// value. Moving the boxes does not move their allocations, and the exported value is initialized
// once before it is shared for the remainder of the loaded plugin generation.
unsafe impl Send for ExportedPluginManifest {}
// SAFETY: after construction neither the backing allocations nor the raw manifest are mutated.
unsafe impl Sync for ExportedPluginManifest {}

impl ExportedPluginManifest {
    #[must_use]
    pub fn manifest(&self) -> &PluginManifestV9 {
        &self.manifest
    }
}

fn utf8_slice_from_str(value: &str) -> Utf8Slice {
    Utf8Slice {
        ptr: value.as_ptr(),
        len: value.len(),
    }
}

fn manifest_capability_strings(manifest: &StaticPluginManifest) -> Vec<String> {
    match manifest.capabilities {
        StaticPluginCapabilities::Protocol => {
            vec![ProtocolCapability::RuntimeReload.as_str().to_string()]
        }
        StaticPluginCapabilities::Gameplay { profile_id } => vec![
            format!("gameplay.profile:{profile_id}"),
            GameplayCapability::RuntimeReload.as_str().to_string(),
        ],
        StaticPluginCapabilities::Storage { profile_id } => vec![
            format!("storage.profile:{profile_id}"),
            StorageCapability::RuntimeReload.as_str().to_string(),
        ],
        StaticPluginCapabilities::Auth { profile_id } => vec![
            format!("auth.profile:{profile_id}"),
            AuthCapability::RuntimeReload.as_str().to_string(),
        ],
        StaticPluginCapabilities::AdminSurface { profile_id } => vec![
            format!("admin-surface.profile:{profile_id}"),
            AdminSurfaceCapability::RuntimeReload.as_str().to_string(),
        ],
    }
}

#[must_use]
pub fn manifest_from_static(manifest: &StaticPluginManifest) -> ExportedPluginManifest {
    let capability_names = manifest_capability_strings(manifest).into_boxed_slice();
    let capability_descriptors = capability_names
        .iter()
        .map(|capability| CapabilityDescriptorV9 {
            name: utf8_slice_from_str(capability),
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let (capabilities, capabilities_len) = if capability_descriptors.is_empty() {
        (std::ptr::null(), 0)
    } else {
        (
            capability_descriptors.as_ptr(),
            capability_descriptors.len(),
        )
    };
    ExportedPluginManifest {
        manifest: PluginManifestV9 {
            plugin_abi: manifest.plugin_abi,
            struct_size: std::mem::size_of::<PluginManifestV9>(),
            plugin_id: Utf8Slice::from_static_str(manifest.plugin_id),
            display_name: Utf8Slice::from_static_str(manifest.display_name),
            plugin_kind: manifest.plugin_kind.into(),
            min_host_abi: manifest.min_host_abi,
            max_host_abi: manifest.max_host_abi,
            capabilities,
            capabilities_len,
            max_session_handoff_bytes: manifest.max_session_handoff_bytes,
        },
        _capability_names: capability_names,
        _capability_descriptors: capability_descriptors,
    }
}
