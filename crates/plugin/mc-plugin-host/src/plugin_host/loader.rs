use super::{
    Arc, AuthGeneration, AuthPluginApiV1, AuthRequest, CURRENT_PLUGIN_ABI, DecodedManifest,
    GameplayGeneration, GameplayPluginApiV1, GameplayRequest, Library, Mutex,
    PLUGIN_AUTH_API_SYMBOL_V1, PLUGIN_GAMEPLAY_API_SYMBOL_V1, PLUGIN_MANIFEST_SYMBOL_V1,
    PLUGIN_PROTOCOL_API_SYMBOL_V1, PLUGIN_STORAGE_API_SYMBOL_V1, Path, PluginErrorCode,
    PluginGenerationId, PluginManifestV1, PluginPackage, PluginSource, ProtocolGeneration,
    ProtocolPluginApiV1, ProtocolRequest, RuntimeError, StorageGeneration, StoragePluginApiV1,
    StorageRequest, decode_manifest, expect_auth_capabilities, expect_auth_descriptor,
    expect_gameplay_capabilities, expect_gameplay_descriptor,
    expect_protocol_bedrock_listener_descriptor, expect_protocol_capabilities,
    expect_protocol_descriptor, expect_storage_capabilities, expect_storage_descriptor,
    gameplay_host_api, gameplay_profile_id_from_manifest, invoke_auth, invoke_gameplay,
    invoke_protocol, invoke_storage, manifest_profile_id, require_manifest_capability,
};

type LibraryGuard = Option<Arc<Mutex<Library>>>;
type LoadedProtocolApi = (LibraryGuard, DecodedManifest, ProtocolPluginApiV1);
type LoadedGameplayApi = (LibraryGuard, DecodedManifest, GameplayPluginApiV1);
type LoadedStorageApi = (LibraryGuard, DecodedManifest, StoragePluginApiV1);
type LoadedAuthApi = (LibraryGuard, DecodedManifest, AuthPluginApiV1);

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
    fn load_protocol_api(package: &PluginPackage) -> Result<LoadedProtocolApi, RuntimeError> {
        match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                Self::load_dynamic_protocol(library_path)
            },
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessProtocol(plugin) => {
                Ok((None, decode_manifest(plugin.manifest)?, *plugin.api))
            }
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAuth(_) => Err(RuntimeError::Config(format!(
                "plugin `{}` is not a protocol plugin",
                package.plugin_id
            ))),
        }
    }

    fn load_gameplay_api(package: &PluginPackage) -> Result<LoadedGameplayApi, RuntimeError> {
        match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                Self::load_dynamic_gameplay(library_path)
            },
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessGameplay(plugin) => {
                let status = unsafe { (plugin.api.set_host_api)(&gameplay_host_api()) };
                if status != PluginErrorCode::Ok {
                    return Err(RuntimeError::Config(format!(
                        "failed to configure gameplay host api for plugin `{}`: {status:?}",
                        package.plugin_id
                    )));
                }
                Ok((None, decode_manifest(plugin.manifest)?, *plugin.api))
            }
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAuth(_) => Err(RuntimeError::Config(format!(
                "plugin `{}` is not a gameplay plugin",
                package.plugin_id
            ))),
        }
    }

    fn load_storage_api(package: &PluginPackage) -> Result<LoadedStorageApi, RuntimeError> {
        match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                Self::load_dynamic_storage(library_path)
            },
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessStorage(plugin) => {
                Ok((None, decode_manifest(plugin.manifest)?, *plugin.api))
            }
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessAuth(_) => Err(RuntimeError::Config(format!(
                "plugin `{}` is not a storage plugin",
                package.plugin_id
            ))),
        }
    }

    fn load_auth_api(package: &PluginPackage) -> Result<LoadedAuthApi, RuntimeError> {
        match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                Self::load_dynamic_auth(library_path)
            },
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessAuth(plugin) => {
                Ok((None, decode_manifest(plugin.manifest)?, *plugin.api))
            }
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessStorage(_) => Err(RuntimeError::Config(format!(
                "plugin `{}` is not an auth plugin",
                package.plugin_id
            ))),
        }
    }

    pub(super) fn load_protocol_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
    ) -> Result<ProtocolGeneration, RuntimeError> {
        let (guard, manifest, api) = Self::load_protocol_api(package)?;
        self.validate_manifest(package, &manifest)?;
        require_manifest_capability(
            &manifest,
            "runtime.reload.protocol",
            &package.plugin_id,
            "protocol",
        )?;
        let descriptor = expect_protocol_descriptor(
            &package.plugin_id,
            invoke_protocol(&api, &ProtocolRequest::Describe)?,
        )?;
        if descriptor.adapter_id != package.plugin_id {
            return Err(RuntimeError::Config(format!(
                "protocol plugin `{}` describe adapter `{}` did not match package id `{}`",
                package.plugin_id, descriptor.adapter_id, package.plugin_id
            )));
        }
        let bedrock_listener_descriptor = expect_protocol_bedrock_listener_descriptor(
            &package.plugin_id,
            invoke_protocol(&api, &ProtocolRequest::DescribeBedrockListener)?,
        )?;
        let capabilities = expect_protocol_capabilities(
            &package.plugin_id,
            invoke_protocol(&api, &ProtocolRequest::CapabilitySet)?,
        )?;
        Ok(ProtocolGeneration {
            generation_id,
            plugin_id: package.plugin_id.clone(),
            descriptor,
            bedrock_listener_descriptor,
            capabilities,
            invoke: api.invoke,
            free_buffer: api.free_buffer,
            _library_guard: guard,
        })
    }

    pub(super) fn load_gameplay_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
    ) -> Result<GameplayGeneration, RuntimeError> {
        let (guard, manifest, api) = Self::load_gameplay_api(package)?;
        self.validate_manifest(package, &manifest)?;
        let profile_id = gameplay_profile_id_from_manifest(&manifest, &package.plugin_id)?;
        require_manifest_capability(
            &manifest,
            "runtime.reload.gameplay",
            &package.plugin_id,
            "gameplay",
        )?;
        let descriptor = expect_gameplay_descriptor(
            &package.plugin_id,
            invoke_gameplay(&package.plugin_id, &api, &GameplayRequest::Describe)?,
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
            invoke_gameplay(&package.plugin_id, &api, &GameplayRequest::CapabilitySet)?,
        )?;
        Ok(GameplayGeneration {
            generation_id,
            plugin_id: package.plugin_id.clone(),
            profile_id,
            capabilities,
            invoke: api.invoke,
            free_buffer: api.free_buffer,
            _library_guard: guard,
        })
    }

    pub(super) fn load_storage_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
    ) -> Result<StorageGeneration, RuntimeError> {
        let (guard, manifest, api) = Self::load_storage_api(package)?;
        self.validate_manifest(package, &manifest)?;
        let profile_id =
            manifest_profile_id(&manifest, "storage.profile:", &package.plugin_id, "storage")?;
        require_manifest_capability(
            &manifest,
            "runtime.reload.storage",
            &package.plugin_id,
            "storage",
        )?;
        let descriptor = expect_storage_descriptor(
            &package.plugin_id,
            invoke_storage(&package.plugin_id, &api, &StorageRequest::Describe)?,
        )?;
        if descriptor.storage_profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "storage plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id, descriptor.storage_profile, profile_id
            )));
        }
        let capabilities = expect_storage_capabilities(
            &package.plugin_id,
            invoke_storage(&package.plugin_id, &api, &StorageRequest::CapabilitySet)?,
        )?;
        Ok(StorageGeneration {
            generation_id,
            plugin_id: package.plugin_id.clone(),
            profile_id,
            capabilities,
            invoke: api.invoke,
            free_buffer: api.free_buffer,
            _library_guard: guard,
        })
    }

    pub(super) fn load_auth_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
    ) -> Result<AuthGeneration, RuntimeError> {
        let (guard, manifest, api) = Self::load_auth_api(package)?;
        self.validate_manifest(package, &manifest)?;
        let profile_id =
            manifest_profile_id(&manifest, "auth.profile:", &package.plugin_id, "auth")?;
        require_manifest_capability(&manifest, "runtime.reload.auth", &package.plugin_id, "auth")?;
        let descriptor = expect_auth_descriptor(
            &package.plugin_id,
            invoke_auth(&package.plugin_id, &api, &AuthRequest::Describe)?,
        )?;
        if descriptor.auth_profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "auth plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id, descriptor.auth_profile, profile_id
            )));
        }
        let capabilities = expect_auth_capabilities(
            &package.plugin_id,
            invoke_auth(&package.plugin_id, &api, &AuthRequest::CapabilitySet)?,
        )?;
        Ok(AuthGeneration {
            generation_id,
            plugin_id: package.plugin_id.clone(),
            profile_id,
            mode: descriptor.mode,
            capabilities,
            invoke: api.invoke,
            free_buffer: api.free_buffer,
            _library_guard: guard,
        })
    }

    unsafe fn load_dynamic_protocol(
        library_path: &Path,
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
            let api_fn: libloading::Symbol<unsafe extern "C" fn() -> *const ProtocolPluginApiV1> =
                unsafe { library.get(PLUGIN_PROTOCOL_API_SYMBOL_V1) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve protocol api symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { *api_fn() }
        };
        Ok((Some(library), decode_manifest(manifest_ptr)?, api))
    }

    unsafe fn load_dynamic_gameplay(
        library_path: &Path,
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
            let api_fn: libloading::Symbol<unsafe extern "C" fn() -> *const GameplayPluginApiV1> =
                unsafe { library.get(PLUGIN_GAMEPLAY_API_SYMBOL_V1) }.map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to resolve gameplay api symbol in {}: {error}",
                        library_path.display()
                    ))
                })?;
            unsafe { *api_fn() }
        };
        let status = unsafe { (api.set_host_api)(&gameplay_host_api()) };
        if status != PluginErrorCode::Ok {
            return Err(RuntimeError::Config(format!(
                "failed to configure gameplay host api in {}: {status:?}",
                library_path.display()
            )));
        }
        Ok((Some(library), decode_manifest(manifest_ptr)?, api))
    }

    unsafe fn load_dynamic_storage(library_path: &Path) -> Result<LoadedStorageApi, RuntimeError> {
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
        Ok((Some(library), decode_manifest(manifest_ptr)?, api))
    }

    unsafe fn load_dynamic_auth(library_path: &Path) -> Result<LoadedAuthApi, RuntimeError> {
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
        Ok((Some(library), decode_manifest(manifest_ptr)?, api))
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
