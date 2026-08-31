use super::{
    AdminSurfaceCapability, AdminSurfaceGeneration, AdminSurfaceInvocation,
    AdminSurfacePluginApiV9, AdminSurfaceRequest, Arc, AuthCapability, AuthGeneration,
    AuthInvocation, AuthPluginApiV9, AuthRequest, CURRENT_PLUGIN_ABI, DecodedManifest,
    GameplayCapability, GameplayGeneration, GameplayInvocation, GameplayPluginApiV9,
    GameplayRequest, Library, ManifestCapabilities, Mutex, PLUGIN_ADMIN_SURFACE_API_SYMBOL_V9,
    PLUGIN_AUTH_API_SYMBOL_V9, PLUGIN_GAMEPLAY_API_SYMBOL_V9, PLUGIN_MANIFEST_SYMBOL_V9,
    PLUGIN_PROTOCOL_API_SYMBOL_V9, PLUGIN_STORAGE_API_SYMBOL_V9, Path, PluginApiHeaderV9,
    PluginGenerationId, PluginManifestV9, PluginPackage, ProtocolCapability, ProtocolGeneration,
    ProtocolInvocation, ProtocolPluginApiV9, ProtocolRequest, RuntimeError, StorageCapability,
    StorageGeneration, StorageInvocation, StoragePluginApiV9, StorageRequest,
    admin_surface_host_api, decode_manifest, expect_admin_surface_capabilities,
    expect_admin_surface_descriptor, expect_auth_capabilities, expect_auth_descriptor,
    expect_gameplay_capabilities, expect_gameplay_descriptor,
    expect_protocol_bedrock_listener_descriptor, expect_protocol_capabilities,
    expect_protocol_descriptor, expect_storage_capabilities, expect_storage_descriptor,
    gameplay_metadata_host_api,
};
use crate::config::PluginBufferLimits;

type LibraryLease = Arc<Mutex<Library>>;
type LoadedDynamicProtocolApi = (LibraryLease, DecodedManifest, ProtocolPluginApiV9);
type LoadedDynamicGameplayApi = (LibraryLease, DecodedManifest, GameplayPluginApiV9);
type LoadedDynamicStorageApi = (LibraryLease, DecodedManifest, StoragePluginApiV9);
type LoadedDynamicAuthApi = (LibraryLease, DecodedManifest, AuthPluginApiV9);
type LoadedDynamicAdminSurfaceApi = (LibraryLease, DecodedManifest, AdminSurfacePluginApiV9);
type LoadedProtocolBackend = (DecodedManifest, ProtocolInvocation);
type LoadedGameplayBackend = (DecodedManifest, GameplayInvocation);
type LoadedStorageBackend = (DecodedManifest, StorageInvocation);
type LoadedAuthBackend = (DecodedManifest, AuthInvocation);
type LoadedAdminSurfaceBackend = (DecodedManifest, AdminSurfaceInvocation);

fn required_api_entry<T: Copy>(
    entry: Option<T>,
    plugin_id: &str,
    entry_name: &str,
) -> Result<T, RuntimeError> {
    entry.ok_or_else(|| {
        RuntimeError::Config(format!(
            "plugin `{plugin_id}` ABI 9 API omitted required `{entry_name}` entry"
        ))
    })
}

unsafe fn read_plugin_api<T: Copy>(api: *const T, api_name: &str) -> Result<T, RuntimeError> {
    if api.is_null() {
        return Err(RuntimeError::Config(format!(
            "{api_name} plugin API pointer was null"
        )));
    }
    if !(api as usize).is_multiple_of(std::mem::align_of::<T>()) {
        return Err(RuntimeError::Config(format!(
            "{api_name} plugin API pointer was not properly aligned"
        )));
    }
    let header = unsafe { api.cast::<PluginApiHeaderV9>().read() };
    if header.abi != CURRENT_PLUGIN_ABI {
        return Err(RuntimeError::Config(format!(
            "{api_name} plugin API ABI {} did not match host ABI {}",
            header.abi, CURRENT_PLUGIN_ABI
        )));
    }
    if header.struct_size < std::mem::size_of::<T>() {
        return Err(RuntimeError::Config(format!(
            "{api_name} plugin API size {} was smaller than ABI 9 size {}",
            header.struct_size,
            std::mem::size_of::<T>()
        )));
    }
    Ok(unsafe { api.read() })
}

pub(crate) struct PluginLoader {
    abi_range: super::PluginAbiRange,
}

impl PluginLoader {
    #[must_use]
    pub(crate) const fn new(abi_range: super::PluginAbiRange) -> Self {
        Self { abi_range }
    }
}

impl PluginLoader {
    fn load_protocol_backend(
        package: &PluginPackage,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedProtocolBackend, RuntimeError> {
        let (guard, manifest, api) =
            unsafe { Self::load_dynamic_protocol(&package.library_path, buffer_limits) }?;
        Ok((
            manifest,
            ProtocolInvocation {
                invoke: required_api_entry(api.invoke, &package.plugin_id, "invoke")?,
                free_buffer: required_api_entry(
                    api.free_buffer,
                    &package.plugin_id,
                    "free_buffer",
                )?,
                _library_lease: guard,
            },
        ))
    }

    fn load_gameplay_backend(
        package: &PluginPackage,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedGameplayBackend, RuntimeError> {
        let (guard, manifest, api) =
            unsafe { Self::load_dynamic_gameplay(&package.library_path, buffer_limits) }?;
        Ok((
            manifest,
            GameplayInvocation {
                invoke: required_api_entry(api.invoke, &package.plugin_id, "invoke")?,
                free_buffer: required_api_entry(
                    api.free_buffer,
                    &package.plugin_id,
                    "free_buffer",
                )?,
                _library_lease: guard,
            },
        ))
    }

    fn load_storage_backend(
        package: &PluginPackage,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedStorageBackend, RuntimeError> {
        let (guard, manifest, api) =
            unsafe { Self::load_dynamic_storage(&package.library_path, buffer_limits) }?;
        Ok((
            manifest,
            StorageInvocation {
                invoke: required_api_entry(api.invoke, &package.plugin_id, "invoke")?,
                free_buffer: required_api_entry(
                    api.free_buffer,
                    &package.plugin_id,
                    "free_buffer",
                )?,
                _library_lease: guard,
            },
        ))
    }

    fn load_auth_backend(
        package: &PluginPackage,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedAuthBackend, RuntimeError> {
        let (guard, manifest, api) =
            unsafe { Self::load_dynamic_auth(&package.library_path, buffer_limits) }?;
        Ok((
            manifest,
            AuthInvocation {
                invoke: required_api_entry(api.invoke, &package.plugin_id, "invoke")?,
                free_buffer: required_api_entry(
                    api.free_buffer,
                    &package.plugin_id,
                    "free_buffer",
                )?,
                _library_lease: guard,
            },
        ))
    }

    fn load_admin_surface_backend(
        package: &PluginPackage,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedAdminSurfaceBackend, RuntimeError> {
        let (guard, manifest, api) =
            unsafe { Self::load_dynamic_admin_surface(&package.library_path, buffer_limits) }?;
        Ok((
            manifest,
            AdminSurfaceInvocation {
                invoke: required_api_entry(api.invoke, &package.plugin_id, "invoke")?,
                free_buffer: required_api_entry(
                    api.free_buffer,
                    &package.plugin_id,
                    "free_buffer",
                )?,
                _library_lease: guard,
            },
        ))
    }

    pub(super) fn load_protocol_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
        buffer_limits: PluginBufferLimits,
    ) -> Result<ProtocolGeneration, RuntimeError> {
        let (manifest, backend) = Self::load_protocol_backend(package, buffer_limits)?;
        self.validate_manifest(package, &manifest)?;
        let descriptor = expect_protocol_descriptor(
            &package.plugin_id,
            backend
                .invoke(
                    &package.plugin_id,
                    &ProtocolRequest::Describe,
                    buffer_limits,
                )
                .map_err(RuntimeError::Config)?,
        )?;
        if descriptor.adapter_id != package.plugin_id {
            return Err(RuntimeError::Config(format!(
                "protocol plugin `{}` describe adapter `{}` did not match package id `{}`",
                package.plugin_id, descriptor.adapter_id, package.plugin_id
            )));
        }
        let bedrock_listener_descriptor = expect_protocol_bedrock_listener_descriptor(
            &package.plugin_id,
            backend
                .invoke(
                    &package.plugin_id,
                    &ProtocolRequest::DescribeBedrockListener,
                    buffer_limits,
                )
                .map_err(RuntimeError::Config)?,
        )?;
        let capabilities = expect_protocol_capabilities(
            &package.plugin_id,
            backend
                .invoke(
                    &package.plugin_id,
                    &ProtocolRequest::CapabilitySet,
                    buffer_limits,
                )
                .map_err(RuntimeError::Config)?,
        )?;
        if !capabilities.contains(ProtocolCapability::RuntimeReload) {
            return Err(RuntimeError::Config(format!(
                "protocol plugin `{}` is missing {} capability",
                package.plugin_id,
                ProtocolCapability::RuntimeReload.as_str()
            )));
        }
        Ok(ProtocolGeneration {
            generation_id,
            plugin_id: package.plugin_id.clone(),
            descriptor,
            bedrock_listener_descriptor,
            capabilities: capabilities.capabilities,
            buffer_limits,
            build_tag: capabilities.build_tag,
            invocation: backend,
        })
    }

    pub(super) fn load_gameplay_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
        buffer_limits: PluginBufferLimits,
    ) -> Result<GameplayGeneration, RuntimeError> {
        let (manifest, backend) = Self::load_gameplay_backend(package, buffer_limits)?;
        self.validate_manifest(package, &manifest)?;
        let ManifestCapabilities::Gameplay(manifest_capabilities) = &manifest.capabilities else {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` manifest kind mismatch",
                package.plugin_id
            )));
        };
        let profile_id = manifest_capabilities.profile_id.clone();
        let descriptor = expect_gameplay_descriptor(
            &package.plugin_id,
            backend
                .invoke(
                    &package.plugin_id,
                    &GameplayRequest::Describe,
                    buffer_limits,
                    gameplay_metadata_host_api(),
                )
                .map_err(RuntimeError::Config)?,
        )?;
        if descriptor.profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "gameplay plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id,
                descriptor.profile.as_str(),
                profile_id.as_str()
            )));
        }
        let capabilities = expect_gameplay_capabilities(
            &package.plugin_id,
            backend
                .invoke(
                    &package.plugin_id,
                    &GameplayRequest::CapabilitySet,
                    buffer_limits,
                    gameplay_metadata_host_api(),
                )
                .map_err(RuntimeError::Config)?,
        )?;
        if !capabilities.contains(GameplayCapability::RuntimeReload) {
            return Err(RuntimeError::Config(format!(
                "gameplay plugin `{}` is missing {} capability",
                package.plugin_id,
                GameplayCapability::RuntimeReload.as_str()
            )));
        }
        Ok(GameplayGeneration {
            generation_id,
            plugin_id: package.plugin_id.clone(),
            profile_id,
            capabilities: capabilities.capabilities,
            buffer_limits,
            build_tag: capabilities.build_tag,
            invocation: backend,
        })
    }

    pub(super) fn load_storage_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
        buffer_limits: PluginBufferLimits,
    ) -> Result<StorageGeneration, RuntimeError> {
        let (manifest, backend) = Self::load_storage_backend(package, buffer_limits)?;
        self.validate_manifest(package, &manifest)?;
        let ManifestCapabilities::Storage(manifest_capabilities) = &manifest.capabilities else {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` manifest kind mismatch",
                package.plugin_id
            )));
        };
        let profile_id = manifest_capabilities.profile_id.clone();
        let descriptor = expect_storage_descriptor(
            &package.plugin_id,
            backend
                .invoke(&package.plugin_id, &StorageRequest::Describe, buffer_limits)
                .map_err(RuntimeError::Config)?,
        )?;
        if descriptor.storage_profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "storage plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id, descriptor.storage_profile, profile_id
            )));
        }
        let capabilities = expect_storage_capabilities(
            &package.plugin_id,
            backend
                .invoke(
                    &package.plugin_id,
                    &StorageRequest::CapabilitySet,
                    buffer_limits,
                )
                .map_err(RuntimeError::Config)?,
        )?;
        if !capabilities.contains(StorageCapability::RuntimeReload) {
            return Err(RuntimeError::Config(format!(
                "storage plugin `{}` is missing {} capability",
                package.plugin_id,
                StorageCapability::RuntimeReload.as_str()
            )));
        }
        Ok(StorageGeneration {
            generation_id,
            plugin_id: package.plugin_id.clone(),
            profile_id,
            capabilities: capabilities.capabilities,
            buffer_limits,
            build_tag: capabilities.build_tag,
            invocation: backend,
        })
    }

    pub(super) fn load_auth_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
        buffer_limits: PluginBufferLimits,
    ) -> Result<AuthGeneration, RuntimeError> {
        let (manifest, backend) = Self::load_auth_backend(package, buffer_limits)?;
        self.validate_manifest(package, &manifest)?;
        let ManifestCapabilities::Auth(manifest_capabilities) = &manifest.capabilities else {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` manifest kind mismatch",
                package.plugin_id
            )));
        };
        let profile_id = manifest_capabilities.profile_id.clone();
        let descriptor = expect_auth_descriptor(
            &package.plugin_id,
            backend
                .invoke(&package.plugin_id, &AuthRequest::Describe, buffer_limits)
                .map_err(RuntimeError::Config)?,
        )?;
        if descriptor.auth_profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "auth plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id, descriptor.auth_profile, profile_id
            )));
        }
        let capabilities = expect_auth_capabilities(
            &package.plugin_id,
            backend
                .invoke(
                    &package.plugin_id,
                    &AuthRequest::CapabilitySet,
                    buffer_limits,
                )
                .map_err(RuntimeError::Config)?,
        )?;
        if !capabilities.contains(AuthCapability::RuntimeReload) {
            return Err(RuntimeError::Config(format!(
                "auth plugin `{}` is missing {} capability",
                package.plugin_id,
                AuthCapability::RuntimeReload.as_str()
            )));
        }
        Ok(AuthGeneration {
            generation_id,
            plugin_id: package.plugin_id.clone(),
            profile_id,
            mode: descriptor.mode,
            capabilities: capabilities.capabilities,
            buffer_limits,
            build_tag: capabilities.build_tag,
            invocation: backend,
        })
    }

    pub(super) fn load_admin_surface_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
        buffer_limits: PluginBufferLimits,
    ) -> Result<AdminSurfaceGeneration, RuntimeError> {
        let (manifest, backend) = Self::load_admin_surface_backend(package, buffer_limits)?;
        self.validate_manifest(package, &manifest)?;
        let ManifestCapabilities::AdminSurface(manifest_capabilities) = &manifest.capabilities
        else {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` manifest kind mismatch",
                package.plugin_id
            )));
        };
        let profile_id = manifest_capabilities.profile_id.clone();
        let descriptor = expect_admin_surface_descriptor(
            &package.plugin_id,
            backend
                .invoke(
                    &package.plugin_id,
                    &AdminSurfaceRequest::Describe,
                    buffer_limits,
                    admin_surface_host_api(),
                )
                .map_err(RuntimeError::Config)?,
        )?;
        if descriptor.surface_profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "admin-surface plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id,
                descriptor.surface_profile.as_str(),
                profile_id.as_str()
            )));
        }
        let capabilities = expect_admin_surface_capabilities(
            &package.plugin_id,
            backend
                .invoke(
                    &package.plugin_id,
                    &AdminSurfaceRequest::CapabilitySet,
                    buffer_limits,
                    admin_surface_host_api(),
                )
                .map_err(RuntimeError::Config)?,
        )?;
        if !capabilities.contains(AdminSurfaceCapability::RuntimeReload) {
            return Err(RuntimeError::Config(format!(
                "admin-surface plugin `{}` is missing {} capability",
                package.plugin_id,
                AdminSurfaceCapability::RuntimeReload.as_str()
            )));
        }
        Ok(AdminSurfaceGeneration {
            generation_id,
            plugin_id: package.plugin_id.clone(),
            profile_id,
            capabilities: capabilities.capabilities,
            buffer_limits,
            build_tag: capabilities.build_tag,
            invocation: backend,
        })
    }

    unsafe fn load_dynamic_protocol(
        library_path: &Path,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedDynamicProtocolApi, RuntimeError> {
        let library = Arc::new(Mutex::new(unsafe { Library::new(library_path) }?));
        let manifest_ptr = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let manifest_fn: libloading::Symbol<unsafe extern "C" fn() -> *const PluginManifestV9> =
                unsafe { library.get(PLUGIN_MANIFEST_SYMBOL_V9) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve plugin manifest symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { manifest_fn() }
        };
        let api = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let api_fn: libloading::Symbol<unsafe extern "C" fn() -> *const ProtocolPluginApiV9> =
                unsafe { library.get(PLUGIN_PROTOCOL_API_SYMBOL_V9) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve protocol api symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { read_plugin_api(api_fn(), "protocol") }?
        };
        Ok((library, decode_manifest(manifest_ptr, buffer_limits)?, api))
    }

    unsafe fn load_dynamic_gameplay(
        library_path: &Path,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedDynamicGameplayApi, RuntimeError> {
        let library = Arc::new(Mutex::new(unsafe { Library::new(library_path) }?));
        let manifest_ptr = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let manifest_fn: libloading::Symbol<unsafe extern "C" fn() -> *const PluginManifestV9> =
                unsafe { library.get(PLUGIN_MANIFEST_SYMBOL_V9) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve plugin manifest symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { manifest_fn() }
        };
        let api = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let api_fn: libloading::Symbol<unsafe extern "C" fn() -> *const GameplayPluginApiV9> =
                unsafe { library.get(PLUGIN_GAMEPLAY_API_SYMBOL_V9) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve gameplay api symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { read_plugin_api(api_fn(), "gameplay") }?
        };
        Ok((library, decode_manifest(manifest_ptr, buffer_limits)?, api))
    }

    unsafe fn load_dynamic_storage(
        library_path: &Path,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedDynamicStorageApi, RuntimeError> {
        let library = Arc::new(Mutex::new(unsafe { Library::new(library_path) }?));
        let manifest_ptr = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let manifest_fn: libloading::Symbol<unsafe extern "C" fn() -> *const PluginManifestV9> =
                unsafe { library.get(PLUGIN_MANIFEST_SYMBOL_V9) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve plugin manifest symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { manifest_fn() }
        };
        let api = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let api_fn: libloading::Symbol<unsafe extern "C" fn() -> *const StoragePluginApiV9> =
                unsafe { library.get(PLUGIN_STORAGE_API_SYMBOL_V9) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve storage api symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { read_plugin_api(api_fn(), "storage") }?
        };
        Ok((library, decode_manifest(manifest_ptr, buffer_limits)?, api))
    }

    unsafe fn load_dynamic_auth(
        library_path: &Path,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedDynamicAuthApi, RuntimeError> {
        let library = Arc::new(Mutex::new(unsafe { Library::new(library_path) }?));
        let manifest_ptr = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let manifest_fn: libloading::Symbol<unsafe extern "C" fn() -> *const PluginManifestV9> =
                unsafe { library.get(PLUGIN_MANIFEST_SYMBOL_V9) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve plugin manifest symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { manifest_fn() }
        };
        let api = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let api_fn: libloading::Symbol<unsafe extern "C" fn() -> *const AuthPluginApiV9> =
                unsafe { library.get(PLUGIN_AUTH_API_SYMBOL_V9) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve auth api symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { read_plugin_api(api_fn(), "auth") }?
        };
        Ok((library, decode_manifest(manifest_ptr, buffer_limits)?, api))
    }

    unsafe fn load_dynamic_admin_surface(
        library_path: &Path,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedDynamicAdminSurfaceApi, RuntimeError> {
        let library = Arc::new(Mutex::new(unsafe { Library::new(library_path) }?));
        let manifest_ptr = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let manifest_fn: libloading::Symbol<unsafe extern "C" fn() -> *const PluginManifestV9> =
                unsafe { library.get(PLUGIN_MANIFEST_SYMBOL_V9) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve plugin manifest symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { manifest_fn() }
        };
        let api = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let api_fn: libloading::Symbol<
                unsafe extern "C" fn() -> *const AdminSurfacePluginApiV9,
            > = unsafe { library.get(PLUGIN_ADMIN_SURFACE_API_SYMBOL_V9) }.map_err(|error| {
                RuntimeError::Config(format!(
                    "failed to resolve admin-surface api symbol in {}: {error}",
                    library_path.display()
                ))
            })?;
            unsafe { read_plugin_api(api_fn(), "admin-surface") }?
        };
        Ok((library, decode_manifest(manifest_ptr, buffer_limits)?, api))
    }

    fn validate_manifest(
        &self,
        package: &PluginPackage,
        manifest: &DecodedManifest,
    ) -> Result<(), RuntimeError> {
        if manifest.plugin_id != package.plugin_id {
            return Err(RuntimeError::Config(format!(
                "plugin manifest id `{}` does not match package id `{}`",
                manifest.plugin_id, package.plugin_id
            )));
        }
        if manifest.plugin_kind != package.plugin_kind {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` manifest kind mismatch",
                package.plugin_id
            )));
        }
        if manifest.plugin_abi != CURRENT_PLUGIN_ABI {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` ABI {} did not match current host ABI {}",
                package.plugin_id, manifest.plugin_abi, CURRENT_PLUGIN_ABI
            )));
        }
        if manifest.min_host_abi > manifest.max_host_abi {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` declared an inverted host ABI range {}..={}",
                package.plugin_id, manifest.min_host_abi, manifest.max_host_abi
            )));
        }
        if !self.abi_range.contains(manifest.plugin_abi) {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` ABI {} is outside host range {}..={}",
                package.plugin_id, manifest.plugin_abi, self.abi_range.min, self.abi_range.max
            )));
        }
        if manifest.min_host_abi > CURRENT_PLUGIN_ABI || manifest.max_host_abi < CURRENT_PLUGIN_ABI
        {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` host ABI range {}..={} does not include {}",
                package.plugin_id, manifest.min_host_abi, manifest.max_host_abi, CURRENT_PLUGIN_ABI
            )));
        }
        Ok(())
    }
}
