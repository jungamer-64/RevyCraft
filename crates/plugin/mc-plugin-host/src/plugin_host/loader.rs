use super::{
    AdminUiCapability, AdminUiGeneration, AdminUiInput, AdminUiPluginApiV1, Arc, AuthCapability,
    AuthGeneration, AuthPluginApiV1, AuthRequest, CURRENT_PLUGIN_ABI, DecodedManifest,
    GameplayCapability, GameplayGeneration, GameplayPluginApiV2, GameplayRequest, Library,
    ManifestCapabilities, Mutex, PLUGIN_ADMIN_UI_API_SYMBOL_V1, PLUGIN_AUTH_API_SYMBOL_V1,
    PLUGIN_GAMEPLAY_API_SYMBOL_V2, PLUGIN_MANIFEST_SYMBOL_V1, PLUGIN_PROTOCOL_API_SYMBOL_V2,
    PLUGIN_STORAGE_API_SYMBOL_V1, Path, PluginGenerationId, PluginManifestV1, PluginPackage,
    PluginSource, ProtocolCapability, ProtocolGeneration, ProtocolPluginApiV2, ProtocolRequest,
    RuntimeError, StorageCapability, StorageGeneration, StoragePluginApiV1, StorageRequest,
    decode_manifest, expect_admin_ui_capabilities, expect_admin_ui_descriptor,
    expect_auth_capabilities, expect_auth_descriptor, expect_gameplay_capabilities,
    expect_gameplay_descriptor, expect_protocol_bedrock_listener_descriptor,
    expect_protocol_capabilities, expect_protocol_descriptor, expect_storage_capabilities,
    expect_storage_descriptor, invoke_admin_ui, invoke_auth, invoke_gameplay, invoke_protocol,
    invoke_storage,
};
use crate::config::PluginBufferLimits;

type LibraryGuard = Option<Arc<Mutex<Library>>>;
type LoadedProtocolApi = (LibraryGuard, DecodedManifest, ProtocolPluginApiV2);
type LoadedGameplayApi = (LibraryGuard, DecodedManifest, GameplayPluginApiV2);
type LoadedStorageApi = (LibraryGuard, DecodedManifest, StoragePluginApiV1);
type LoadedAuthApi = (LibraryGuard, DecodedManifest, AuthPluginApiV1);
type LoadedAdminUiApi = (LibraryGuard, DecodedManifest, AdminUiPluginApiV1);

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
    fn load_protocol_api(
        package: &PluginPackage,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedProtocolApi, RuntimeError> {
        match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                Self::load_dynamic_protocol(library_path, buffer_limits)
            },
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessProtocol(plugin) => Ok((
                None,
                decode_manifest(plugin.manifest, buffer_limits)?,
                *plugin.api,
            )),
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAuth(_)
            | PluginSource::InProcessAdminUi(_) => Err(RuntimeError::Config(format!(
                "plugin `{}` is not a protocol plugin",
                package.plugin_id
            ))),
        }
    }

    fn load_gameplay_api(
        package: &PluginPackage,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedGameplayApi, RuntimeError> {
        match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                Self::load_dynamic_gameplay(library_path, buffer_limits)
            },
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessGameplay(plugin) => Ok((
                None,
                decode_manifest(plugin.manifest, buffer_limits)?,
                *plugin.api,
            )),
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAuth(_)
            | PluginSource::InProcessAdminUi(_) => Err(RuntimeError::Config(format!(
                "plugin `{}` is not a gameplay plugin",
                package.plugin_id
            ))),
        }
    }

    fn load_storage_api(
        package: &PluginPackage,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedStorageApi, RuntimeError> {
        match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                Self::load_dynamic_storage(library_path, buffer_limits)
            },
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessStorage(plugin) => Ok((
                None,
                decode_manifest(plugin.manifest, buffer_limits)?,
                *plugin.api,
            )),
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessAuth(_)
            | PluginSource::InProcessAdminUi(_) => Err(RuntimeError::Config(format!(
                "plugin `{}` is not a storage plugin",
                package.plugin_id
            ))),
        }
    }

    fn load_auth_api(
        package: &PluginPackage,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedAuthApi, RuntimeError> {
        match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                Self::load_dynamic_auth(library_path, buffer_limits)
            },
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessAuth(plugin) => Ok((
                None,
                decode_manifest(plugin.manifest, buffer_limits)?,
                *plugin.api,
            )),
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAdminUi(_) => Err(RuntimeError::Config(format!(
                "plugin `{}` is not an auth plugin",
                package.plugin_id
            ))),
        }
    }

    fn load_admin_ui_api(
        package: &PluginPackage,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedAdminUiApi, RuntimeError> {
        match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                Self::load_dynamic_admin_ui(library_path, buffer_limits)
            },
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessAdminUi(plugin) => Ok((
                None,
                decode_manifest(plugin.manifest, buffer_limits)?,
                *plugin.api,
            )),
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAuth(_) => Err(RuntimeError::Config(format!(
                "plugin `{}` is not an admin-ui plugin",
                package.plugin_id
            ))),
        }
    }

    pub(super) fn load_protocol_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
        buffer_limits: PluginBufferLimits,
    ) -> Result<ProtocolGeneration, RuntimeError> {
        let (guard, manifest, api) = Self::load_protocol_api(package, buffer_limits)?;
        self.validate_manifest(package, &manifest)?;
        let descriptor = expect_protocol_descriptor(
            &package.plugin_id,
            invoke_protocol(&api, &ProtocolRequest::Describe, buffer_limits)?,
        )?;
        if descriptor.adapter_id != package.plugin_id {
            return Err(RuntimeError::Config(format!(
                "protocol plugin `{}` describe adapter `{}` did not match package id `{}`",
                package.plugin_id, descriptor.adapter_id, package.plugin_id
            )));
        }
        let bedrock_listener_descriptor = expect_protocol_bedrock_listener_descriptor(
            &package.plugin_id,
            invoke_protocol(
                &api,
                &ProtocolRequest::DescribeBedrockListener,
                buffer_limits,
            )?,
        )?;
        let capabilities = expect_protocol_capabilities(
            &package.plugin_id,
            invoke_protocol(&api, &ProtocolRequest::CapabilitySet, buffer_limits)?,
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
            invoke: api.invoke,
            free_buffer: api.free_buffer,
            _library_guard: guard,
        })
    }

    pub(super) fn load_gameplay_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
        buffer_limits: PluginBufferLimits,
    ) -> Result<GameplayGeneration, RuntimeError> {
        let (guard, manifest, api) = Self::load_gameplay_api(package, buffer_limits)?;
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
            invoke_gameplay(
                &package.plugin_id,
                &api,
                &GameplayRequest::Describe,
                buffer_limits,
            )?,
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
            invoke_gameplay(
                &package.plugin_id,
                &api,
                &GameplayRequest::CapabilitySet,
                buffer_limits,
            )?,
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
            invoke: api.invoke,
            free_buffer: api.free_buffer,
            _library_guard: guard,
        })
    }

    pub(super) fn load_storage_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
        buffer_limits: PluginBufferLimits,
    ) -> Result<StorageGeneration, RuntimeError> {
        let (guard, manifest, api) = Self::load_storage_api(package, buffer_limits)?;
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
            invoke_storage(
                &package.plugin_id,
                &api,
                &StorageRequest::Describe,
                buffer_limits,
            )?,
        )?;
        if descriptor.storage_profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "storage plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id, descriptor.storage_profile, profile_id
            )));
        }
        let capabilities = expect_storage_capabilities(
            &package.plugin_id,
            invoke_storage(
                &package.plugin_id,
                &api,
                &StorageRequest::CapabilitySet,
                buffer_limits,
            )?,
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
            invoke: api.invoke,
            free_buffer: api.free_buffer,
            _library_guard: guard,
        })
    }

    pub(super) fn load_auth_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
        buffer_limits: PluginBufferLimits,
    ) -> Result<AuthGeneration, RuntimeError> {
        let (guard, manifest, api) = Self::load_auth_api(package, buffer_limits)?;
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
            invoke_auth(
                &package.plugin_id,
                &api,
                &AuthRequest::Describe,
                buffer_limits,
            )?,
        )?;
        if descriptor.auth_profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "auth plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id, descriptor.auth_profile, profile_id
            )));
        }
        let capabilities = expect_auth_capabilities(
            &package.plugin_id,
            invoke_auth(
                &package.plugin_id,
                &api,
                &AuthRequest::CapabilitySet,
                buffer_limits,
            )?,
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
            invoke: api.invoke,
            free_buffer: api.free_buffer,
            _library_guard: guard,
        })
    }

    pub(super) fn load_admin_ui_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
        buffer_limits: PluginBufferLimits,
    ) -> Result<AdminUiGeneration, RuntimeError> {
        let (guard, manifest, api) = Self::load_admin_ui_api(package, buffer_limits)?;
        self.validate_manifest(package, &manifest)?;
        let ManifestCapabilities::AdminUi(manifest_capabilities) = &manifest.capabilities else {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` manifest kind mismatch",
                package.plugin_id
            )));
        };
        let profile_id = manifest_capabilities.profile_id.clone();
        let descriptor = expect_admin_ui_descriptor(
            &package.plugin_id,
            invoke_admin_ui(
                &package.plugin_id,
                &api,
                &AdminUiInput::Describe,
                buffer_limits,
            )?,
        )?;
        if descriptor.ui_profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "admin-ui plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id, descriptor.ui_profile, profile_id
            )));
        }
        let capabilities = expect_admin_ui_capabilities(
            &package.plugin_id,
            invoke_admin_ui(
                &package.plugin_id,
                &api,
                &AdminUiInput::CapabilitySet,
                buffer_limits,
            )?,
        )?;
        if !capabilities.contains(AdminUiCapability::RuntimeReload) {
            return Err(RuntimeError::Config(format!(
                "admin-ui plugin `{}` is missing {} capability",
                package.plugin_id,
                AdminUiCapability::RuntimeReload.as_str()
            )));
        }
        Ok(AdminUiGeneration {
            generation_id,
            plugin_id: package.plugin_id.clone(),
            profile_id,
            capabilities: capabilities.capabilities,
            buffer_limits,
            build_tag: capabilities.build_tag,
            invoke: api.invoke,
            free_buffer: api.free_buffer,
            _library_guard: guard,
        })
    }

    unsafe fn load_dynamic_protocol(
        library_path: &Path,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedProtocolApi, RuntimeError> {
        let library = Arc::new(Mutex::new(unsafe { Library::new(library_path) }?));
        let manifest_ptr = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let manifest_fn: libloading::Symbol<unsafe extern "C" fn() -> *const PluginManifestV1> =
                unsafe { library.get(PLUGIN_MANIFEST_SYMBOL_V1) }.map_err(|error| {
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
            let api_fn: libloading::Symbol<unsafe extern "C" fn() -> *const ProtocolPluginApiV2> =
                unsafe { library.get(PLUGIN_PROTOCOL_API_SYMBOL_V2) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve protocol api symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { *api_fn() }
        };
        Ok((
            Some(library),
            decode_manifest(manifest_ptr, buffer_limits)?,
            api,
        ))
    }

    unsafe fn load_dynamic_gameplay(
        library_path: &Path,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedGameplayApi, RuntimeError> {
        let library = Arc::new(Mutex::new(unsafe { Library::new(library_path) }?));
        let manifest_ptr = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let manifest_fn: libloading::Symbol<unsafe extern "C" fn() -> *const PluginManifestV1> =
                unsafe { library.get(PLUGIN_MANIFEST_SYMBOL_V1) }.map_err(|error| {
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
            let api_fn: libloading::Symbol<unsafe extern "C" fn() -> *const GameplayPluginApiV2> =
                unsafe { library.get(PLUGIN_GAMEPLAY_API_SYMBOL_V2) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve gameplay api symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { *api_fn() }
        };
        Ok((
            Some(library),
            decode_manifest(manifest_ptr, buffer_limits)?,
            api,
        ))
    }

    unsafe fn load_dynamic_storage(
        library_path: &Path,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedStorageApi, RuntimeError> {
        let library = Arc::new(Mutex::new(unsafe { Library::new(library_path) }?));
        let manifest_ptr = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let manifest_fn: libloading::Symbol<unsafe extern "C" fn() -> *const PluginManifestV1> =
                unsafe { library.get(PLUGIN_MANIFEST_SYMBOL_V1) }.map_err(|error| {
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
            let api_fn: libloading::Symbol<unsafe extern "C" fn() -> *const StoragePluginApiV1> =
                unsafe { library.get(PLUGIN_STORAGE_API_SYMBOL_V1) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve storage api symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { *api_fn() }
        };
        Ok((
            Some(library),
            decode_manifest(manifest_ptr, buffer_limits)?,
            api,
        ))
    }

    unsafe fn load_dynamic_auth(
        library_path: &Path,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedAuthApi, RuntimeError> {
        let library = Arc::new(Mutex::new(unsafe { Library::new(library_path) }?));
        let manifest_ptr = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let manifest_fn: libloading::Symbol<unsafe extern "C" fn() -> *const PluginManifestV1> =
                unsafe { library.get(PLUGIN_MANIFEST_SYMBOL_V1) }.map_err(|error| {
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
            let api_fn: libloading::Symbol<unsafe extern "C" fn() -> *const AuthPluginApiV1> =
                unsafe { library.get(PLUGIN_AUTH_API_SYMBOL_V1) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve auth api symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { *api_fn() }
        };
        Ok((
            Some(library),
            decode_manifest(manifest_ptr, buffer_limits)?,
            api,
        ))
    }

    unsafe fn load_dynamic_admin_ui(
        library_path: &Path,
        buffer_limits: PluginBufferLimits,
    ) -> Result<LoadedAdminUiApi, RuntimeError> {
        let library = Arc::new(Mutex::new(unsafe { Library::new(library_path) }?));
        let manifest_ptr = {
            let library = library
                .lock()
                .expect("dynamic library mutex should not be poisoned");
            let manifest_fn: libloading::Symbol<unsafe extern "C" fn() -> *const PluginManifestV1> =
                unsafe { library.get(PLUGIN_MANIFEST_SYMBOL_V1) }.map_err(|error| {
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
            let api_fn: libloading::Symbol<unsafe extern "C" fn() -> *const AdminUiPluginApiV1> =
                unsafe { library.get(PLUGIN_ADMIN_UI_API_SYMBOL_V1) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve admin-ui api symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { *api_fn() }
        };
        Ok((
            Some(library),
            decode_manifest(manifest_ptr, buffer_limits)?,
            api,
        ))
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
