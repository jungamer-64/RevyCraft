use crate::PluginHostError as RuntimeError;
use crate::config::{BootstrapConfig, RuntimeSelectionConfig};
use crate::registry::ProtocolRegistry;
use crate::runtime::{
    AuthGenerationHandle, AuthProfileHandle, GameplayProfileHandle, RuntimePluginHost,
    RuntimeProtocolTopologyCandidate, RuntimeReloadContext, StagedRuntimeSelection,
    StorageProfileHandle,
};
use bytes::BytesMut;
use libloading::Library;
use mc_core::{
    AdminSurfaceCapability, AdminSurfaceCapabilitySet, AdminSurfaceProfileId, AuthCapability,
    AuthCapabilitySet, AuthProfileId, GameplayCapability, GameplayCapabilitySet, GameplayCommand,
    GameplayProfileId, PlayerId, PluginBuildTag, PluginGenerationId, ProtocolCapability,
    ProtocolCapabilitySet, ServerCore, SessionCapabilitySet, StorageCapability,
    StorageCapabilitySet, StorageProfileId, WorldSnapshot,
};
use mc_plugin_api::abi::{
    ByteSlice, CURRENT_PLUGIN_ABI, OwnedBuffer, PluginAbiVersion, PluginErrorCode, PluginKind,
};
use mc_plugin_api::codec::admin_surface::{
    AdminSurfaceDescriptor, AdminSurfaceInstanceDeclaration, AdminSurfacePauseView,
    AdminSurfaceRequest, AdminSurfaceResponse, AdminSurfaceStatusView,
    decode_admin_surface_response, encode_admin_surface_request,
};
use mc_plugin_api::codec::auth::{
    AuthMode, AuthRequest, AuthResponse, BedrockAuthResult, decode_auth_response,
    encode_auth_request,
};
use mc_plugin_api::codec::gameplay::{
    GameplayRequest, GameplayResponse, GameplaySessionSnapshot, decode_gameplay_response,
    encode_gameplay_request,
};
use mc_plugin_api::codec::protocol::{
    ProtocolRequest, ProtocolResponse, WireFrameDecodeResult, decode_protocol_response,
    encode_protocol_request,
};
use mc_plugin_api::codec::storage::{
    StorageRequest, StorageResponse, decode_storage_response, encode_storage_request,
};
use mc_plugin_api::host_api::{
    AdminSurfaceHostApiV1, AdminSurfacePluginApiV1, AdminSurfacePluginInvokeV1Fn, AuthPluginApiV1,
    GameplayPluginApiV3, GameplayPluginInvokeV3Fn, PluginFreeBufferFn, PluginInvokeFn,
    ProtocolPluginApiV3, StoragePluginApiV1,
};
use mc_plugin_api::manifest::{
    PLUGIN_ADMIN_SURFACE_API_SYMBOL_V1, PLUGIN_AUTH_API_SYMBOL_V1, PLUGIN_GAMEPLAY_API_SYMBOL_V3,
    PLUGIN_MANIFEST_SYMBOL_V1, PLUGIN_PROTOCOL_API_SYMBOL_V3, PLUGIN_STORAGE_API_SYMBOL_V1,
    PluginManifestV1,
};
use mc_proto_common::{
    BedrockListenerDescriptor, ConnectionPhase, Edition, HandshakeIntent, HandshakeProbe,
    LoginRequest, PlayEncodingContext, ProtocolAdapter, ProtocolDescriptor, ProtocolError,
    ServerListStatus, StatusRequest, StorageAdapter, StorageError, TransportKind, WireCodec,
    WireFormatKind,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

mod activation;
mod callbacks;
mod catalog;
mod failure;
mod generation;
mod loader;
mod profiles;
mod reload;
mod status;
mod support;
mod topology;

use self::callbacks::admin_surface_host_api;
use self::callbacks::gameplay_host_api;
#[cfg(test)]
pub(crate) use self::callbacks::with_current_gameplay_transaction;
pub(crate) use self::callbacks::with_gameplay_transaction_and_limits;
pub(crate) use self::catalog::PluginCatalog;
#[cfg(test)]
pub(crate) use self::catalog::current_artifact_key;
use self::catalog::{
    ArtifactIdentity, DynamicCatalogSource, PluginPackage, PluginSource, system_time_ms,
};
#[cfg(any(test, feature = "in-process-testing"))]
pub use self::catalog::{
    InProcessAdminSurfacePlugin, InProcessAuthPlugin, InProcessGameplayPlugin,
    InProcessProtocolPlugin, InProcessStoragePlugin,
};
pub(crate) use self::failure::{
    ArtifactQuarantineRecord, PluginFailureDispatch, PluginFailureStage,
};
pub use self::failure::{PluginFailureAction, PluginFailureMatrix};
pub(crate) use self::generation::{
    AdminSurfaceGeneration, AuthGeneration, GameplayGeneration, GenerationManager,
    ProtocolGeneration, StorageGeneration, decode_plugin_error, write_owned_buffer,
};
pub(crate) use self::loader::PluginLoader;
pub(crate) use self::profiles::{
    HotSwappableAdminSurfaceProfile, HotSwappableAuthProfile, HotSwappableGameplayProfile,
    HotSwappableProtocolAdapter, HotSwappableStorageProfile, ManagedAdminSurfacePlugin,
    ManagedAuthPlugin, ManagedGameplayPlugin, ManagedProtocolPlugin, ManagedStoragePlugin,
};
pub use self::status::{
    AdminSurfacePluginStatusSnapshot, AuthPluginStatusSnapshot, GameplayPluginStatusSnapshot,
    PluginArtifactStatusSnapshot, PluginHostStatusSnapshot, ProtocolPluginStatusSnapshot,
    StoragePluginStatusSnapshot,
};
use self::support::{
    DecodedManifest, ManifestCapabilities, decode_manifest, decode_utf8_slice,
    ensure_known_profiles, ensure_profile_known, expect_admin_surface_capabilities,
    expect_admin_surface_descriptor, expect_auth_capabilities, expect_auth_descriptor,
    expect_gameplay_capabilities, expect_gameplay_descriptor,
    expect_protocol_bedrock_listener_descriptor, expect_protocol_capabilities,
    expect_protocol_descriptor, expect_storage_capabilities, expect_storage_descriptor,
    import_storage_runtime_state, invoke_admin_surface, invoke_auth, invoke_gameplay,
    invoke_protocol, invoke_storage, protocol_reload_compatible, read_byte_slice,
    take_owned_buffer, validate_gameplay_session_migration, validate_protocol_session_migration,
};
pub(crate) use self::topology::PreparedProtocolTopology;

const PLUGIN_RELOAD_POLL_INTERVAL_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginAbiRange {
    pub min: PluginAbiVersion,
    pub max: PluginAbiVersion,
}

impl Default for PluginAbiRange {
    fn default() -> Self {
        Self {
            min: CURRENT_PLUGIN_ABI,
            max: CURRENT_PLUGIN_ABI,
        }
    }
}

impl PluginAbiRange {
    /// Parses a `major.minor` plugin ABI version string.
    ///
    /// # Errors
    ///
    /// Returns an error when the provided value is not a valid `major.minor` ABI version.
    pub fn parse_version(value: &str) -> Result<PluginAbiVersion, RuntimeError> {
        let Some((major, minor)) = value.split_once('.') else {
            return Err(RuntimeError::Config(format!(
                "invalid plugin ABI version `{value}`"
            )));
        };
        Ok(PluginAbiVersion {
            major: major.parse().map_err(|_| {
                RuntimeError::Config(format!("invalid plugin ABI version `{value}`"))
            })?,
            minor: minor.parse().map_err(|_| {
                RuntimeError::Config(format!("invalid plugin ABI version `{value}`"))
            })?,
        })
    }

    fn contains(self, version: PluginAbiVersion) -> bool {
        version >= self.min && version <= self.max
    }
}

pub struct PluginHost {
    bootstrap_config: BootstrapConfig,
    catalog: PluginCatalog,
    dynamic_catalog_source: Option<DynamicCatalogSource>,
    runtime_selection: Mutex<RuntimeSelectionConfig>,
    loader: PluginLoader,
    generations: Arc<GenerationManager>,
    failures: Arc<PluginFailureDispatch>,
    protocols: Mutex<HashMap<String, ManagedProtocolPlugin>>,
    gameplay: Mutex<HashMap<GameplayProfileId, ManagedGameplayPlugin>>,
    storage: Mutex<HashMap<StorageProfileId, ManagedStoragePlugin>>,
    auth: Mutex<HashMap<AuthProfileId, ManagedAuthPlugin>>,
    admin_surface: Mutex<HashMap<AdminSurfaceProfileId, ManagedAdminSurfacePlugin>>,
}

impl PluginHost {
    fn current_runtime_selection(&self) -> RuntimeSelectionConfig {
        self.runtime_selection
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .clone()
    }

    #[must_use]
    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn new(
        catalog: PluginCatalog,
        bootstrap_config: BootstrapConfig,
        abi_range: PluginAbiRange,
        failure_matrix: PluginFailureMatrix,
    ) -> Self {
        Self::new_with_dynamic_catalog_source(
            catalog,
            bootstrap_config,
            abi_range,
            failure_matrix,
            None,
        )
    }

    #[must_use]
    pub(crate) fn new_with_dynamic_catalog_source(
        catalog: PluginCatalog,
        bootstrap_config: BootstrapConfig,
        abi_range: PluginAbiRange,
        failure_matrix: PluginFailureMatrix,
        dynamic_catalog_source: Option<PathBuf>,
    ) -> Self {
        Self {
            bootstrap_config,
            catalog,
            dynamic_catalog_source: dynamic_catalog_source
                .map(|root| DynamicCatalogSource { root }),
            runtime_selection: Mutex::new(RuntimeSelectionConfig::default()),
            loader: PluginLoader::new(abi_range),
            generations: Arc::new(GenerationManager::default()),
            failures: Arc::new(PluginFailureDispatch::new(failure_matrix)),
            protocols: Mutex::new(HashMap::new()),
            gameplay: Mutex::new(HashMap::new()),
            storage: Mutex::new(HashMap::new()),
            auth: Mutex::new(HashMap::new()),
            admin_surface: Mutex::new(HashMap::new()),
        }
    }

    fn protocol_catalog(&self) -> Result<PluginCatalog, RuntimeError> {
        match &self.dynamic_catalog_source {
            Some(source) => PluginCatalog::discover(&source.root, None),
            None => Ok(self.catalog.clone()),
        }
    }

    /// Resolves an active gameplay profile by id.
    ///
    /// # Panics
    ///
    /// Panics if the gameplay plugin registry mutex is poisoned.
    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn resolve_gameplay_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<HotSwappableGameplayProfile>> {
        self.gameplay
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .get(&GameplayProfileId::new(profile_id))
            .map(|managed| Arc::clone(&managed.profile))
    }

    /// Resolves an active storage profile by id.
    ///
    /// # Panics
    ///
    /// Panics if the storage plugin registry mutex is poisoned.
    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn resolve_storage_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<HotSwappableStorageProfile>> {
        self.storage
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .get(&StorageProfileId::new(profile_id))
            .map(|managed| Arc::clone(&managed.profile))
    }

    /// Resolves an active auth profile by id.
    ///
    /// # Panics
    ///
    /// Panics if the auth plugin registry mutex is poisoned.
    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn resolve_auth_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<HotSwappableAuthProfile>> {
        self.auth
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .get(&AuthProfileId::new(profile_id))
            .map(|managed| Arc::clone(&managed.profile))
    }

    /// Resolves an active admin-surface profile by id.
    ///
    /// # Panics
    ///
    /// Panics if the admin-surface plugin registry mutex is poisoned.
    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn resolve_admin_surface_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<HotSwappableAdminSurfaceProfile>> {
        self.admin_surface
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .get(&AdminSurfaceProfileId::new(profile_id))
            .map(|managed| Arc::clone(&managed.profile))
    }

    /// Replaces a managed protocol plugin with a new in-process implementation.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin is not managed by this host or the replacement
    /// generation cannot be loaded.
    ///
    /// # Panics
    ///
    /// Panics if the protocol plugin registry mutex is poisoned.
    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn replace_in_process_protocol_plugin(
        &self,
        plugin: InProcessProtocolPlugin,
    ) -> Result<PluginGenerationId, RuntimeError> {
        let plugin_id = plugin.plugin_id.clone();
        let mut protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned");
        let managed = protocols.get_mut(&plugin_id).ok_or_else(|| {
            RuntimeError::Config(format!(
                "protocol plugin `{}` is not managed by this host",
                plugin_id
            ))
        })?;
        managed.package.source = PluginSource::InProcessProtocol(plugin);
        let generation_id = self.generations.next_generation_id();
        let generation = Arc::new(self.loader.load_protocol_generation(
            &managed.package,
            generation_id,
            self.current_runtime_selection().buffer_limits,
        )?);
        managed.adapter.swap_generation(generation);
        self.failures.clear_plugin_state(&plugin_id);
        managed.loaded_at = managed.package.modified_at()?;
        managed.active_loaded_at = managed.loaded_at;
        drop(protocols);
        Ok(generation_id)
    }

    pub(crate) fn take_pending_fatal_error(&self) -> Option<RuntimeError> {
        self.failures.take_pending_fatal_error()
    }

    pub(crate) fn handle_runtime_failure(
        &self,
        kind: PluginKind,
        plugin_id: &str,
        reason: &str,
    ) -> PluginFailureAction {
        self.failures
            .handle_runtime_failure(kind, plugin_id, reason)
    }

    pub(crate) fn managed_protocol_ids(&self) -> Vec<String> {
        let mut ids = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn artifact_quarantine_reason(&self, plugin_id: &str) -> Option<String> {
        self.failures
            .artifact_record(plugin_id)
            .and_then(|record| record.reason.into())
    }
}

impl RuntimePluginHost for PluginHost {
    fn stage_runtime_selection(
        &self,
        config: &RuntimeSelectionConfig,
    ) -> Result<StagedRuntimeSelection, RuntimeError> {
        Self::stage_runtime_selection(self, config)
    }

    fn stage_runtime_artifacts(&self) -> Result<StagedRuntimeSelection, RuntimeError> {
        Self::stage_runtime_artifacts(self)
    }

    fn finalize_staged_runtime_selection(
        &self,
        staged: StagedRuntimeSelection,
        runtime: &RuntimeReloadContext,
    ) -> Result<crate::runtime::PreparedRuntimeSelection, RuntimeError> {
        Self::finalize_staged_runtime_selection(self, staged, runtime)
    }

    fn prepare_runtime_selection(
        &self,
        config: &RuntimeSelectionConfig,
        runtime: &RuntimeReloadContext,
    ) -> Result<crate::runtime::PreparedRuntimeSelection, RuntimeError> {
        Self::prepare_runtime_selection(self, config, runtime)
    }

    fn prepare_runtime_artifacts(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<crate::runtime::PreparedRuntimeSelection, RuntimeError> {
        Self::prepare_runtime_artifacts(self, runtime)
    }

    fn commit_runtime_selection(&self, prepared: crate::runtime::PreparedRuntimeSelection) {
        Self::commit_runtime_selection(self, prepared);
    }

    fn prepare_protocol_topology_for_reload(
        &self,
    ) -> Result<RuntimeProtocolTopologyCandidate, RuntimeError> {
        let config = self.current_runtime_selection();
        let prepared = self.prepare_protocol_topology_for_reload(&config)?;
        let requires_protocol_swap = self.requires_protocol_swap(&config, &prepared);
        Ok(RuntimeProtocolTopologyCandidate::new(
            prepared,
            requires_protocol_swap,
        ))
    }

    fn activate_protocol_topology(&self, candidate: RuntimeProtocolTopologyCandidate) {
        Self::activate_protocol_topology(self, candidate.into_prepared());
    }

    fn take_pending_fatal_error(&self) -> Option<RuntimeError> {
        Self::take_pending_fatal_error(self)
    }

    fn handle_runtime_failure(
        &self,
        kind: PluginKind,
        plugin_id: &str,
        reason: &str,
    ) -> PluginFailureAction {
        Self::handle_runtime_failure(self, kind, plugin_id, reason)
    }

    fn managed_protocol_ids(&self) -> Vec<String> {
        Self::managed_protocol_ids(self)
    }

    fn status(&self) -> PluginHostStatusSnapshot {
        Self::status(self)
    }
}

/// Builds a plugin host from the current server configuration.
///
/// # Errors
///
/// Returns an error when plugin discovery fails or a configured plugin manifest is invalid.
pub fn plugin_host_from_config(
    config: &BootstrapConfig,
) -> Result<Option<Arc<PluginHost>>, RuntimeError> {
    let abi_range = PluginAbiRange {
        min: config.plugin_abi_min,
        max: config.plugin_abi_max,
    };
    if !abi_range.contains(CURRENT_PLUGIN_ABI) {
        return Err(RuntimeError::Config(format!(
            "plugin ABI range {}..={} does not include current host ABI {}",
            abi_range.min, abi_range.max, CURRENT_PLUGIN_ABI
        )));
    }
    let catalog = PluginCatalog::discover(&config.plugins_dir, None)?;
    if catalog.is_empty() {
        return Ok(None);
    }
    Ok(Some(Arc::new(PluginHost::new_with_dynamic_catalog_source(
        catalog,
        config.clone(),
        abi_range,
        PluginFailureMatrix::default(),
        Some(config.plugins_dir.clone()),
    ))))
}

#[must_use]
pub const fn plugin_reload_poll_interval_ms() -> u64 {
    PLUGIN_RELOAD_POLL_INTERVAL_MS
}

#[cfg(test)]
mod tests;
