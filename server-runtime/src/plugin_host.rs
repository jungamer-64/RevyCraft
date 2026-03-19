use crate::{RuntimeError, RuntimeRegistries, ServerConfig};
use libloading::Library;
use mc_core::{CapabilitySet, PluginGenerationId};
use mc_plugin_api::{
    CURRENT_PLUGIN_ABI, OwnedBuffer, PLUGIN_MANIFEST_SYMBOL_V1, PLUGIN_PROTOCOL_API_SYMBOL_V1,
    PluginAbiVersion, PluginErrorCode, PluginKind, PluginManifestV1, ProtocolPluginApiV1,
    ProtocolRequest, ProtocolResponse,
};
use mc_proto_common::{
    ConnectionPhase, HandshakeIntent, HandshakeProbe, LoginRequest, MinecraftWireCodec,
    PlayEncodingContext, ProtocolAdapter, ProtocolDescriptor, ProtocolError, ServerListStatus,
    StatusRequest, TransportKind, WireCodec,
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

const PLUGIN_RELOAD_POLL_INTERVAL_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginFailurePolicy {
    Quarantine,
}

impl PluginFailurePolicy {
    pub fn parse(value: &str) -> Result<Self, RuntimeError> {
        if value.eq_ignore_ascii_case("quarantine") {
            Ok(Self::Quarantine)
        } else {
            Err(RuntimeError::Config(format!(
                "unsupported plugin-failure-policy `{value}`"
            )))
        }
    }
}

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
    pub fn parse_version(value: &str) -> Result<PluginAbiVersion, RuntimeError> {
        let Some((major, minor)) = value.split_once('.') else {
            return Err(RuntimeError::Config(format!(
                "invalid plugin ABI version `{value}`"
            )));
        };
        Ok(PluginAbiVersion {
            major: major
                .parse()
                .map_err(|_| RuntimeError::Config(format!("invalid plugin ABI version `{value}`")))?,
            minor: minor
                .parse()
                .map_err(|_| RuntimeError::Config(format!("invalid plugin ABI version `{value}`")))?,
        })
    }

    fn contains(self, version: PluginAbiVersion) -> bool {
        version >= self.min && version <= self.max
    }
}

#[derive(Clone, Debug)]
pub struct InProcessProtocolPlugin {
    pub plugin_id: String,
    pub manifest: &'static PluginManifestV1,
    pub api: &'static ProtocolPluginApiV1,
}

#[derive(Clone, Debug)]
enum PluginSource {
    DynamicLibrary {
        manifest_path: PathBuf,
        library_path: PathBuf,
    },
    InProcess(InProcessProtocolPlugin),
}

#[derive(Clone, Debug)]
struct PluginPackage {
    plugin_id: String,
    plugin_kind: PluginKind,
    source: PluginSource,
}

impl PluginPackage {
    fn modified_at(&self) -> Result<SystemTime, RuntimeError> {
        match &self.source {
            PluginSource::DynamicLibrary {
                manifest_path,
                library_path,
            } => Ok(
                fs::metadata(manifest_path)?
                    .modified()?
                    .max(fs::metadata(library_path)?.modified()?),
            ),
            PluginSource::InProcess(_) => Ok(SystemTime::UNIX_EPOCH),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PluginCatalog {
    packages: HashMap<String, PluginPackage>,
}

impl PluginCatalog {
    pub fn discover(
        root: &Path,
        allowlist: Option<&HashSet<String>>,
    ) -> Result<Self, RuntimeError> {
        if !root.exists() {
            return Ok(Self::default());
        }

        let mut packages = HashMap::new();
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let manifest_path = entry.path().join("plugin.toml");
            if !manifest_path.exists() {
                continue;
            }
            let document: PluginPackageDocument = toml::from_str(&fs::read_to_string(&manifest_path)?)
                .map_err(|error| {
                    RuntimeError::Config(format!(
                        "failed to parse plugin manifest {}: {error}",
                        manifest_path.display()
                    ))
                })?;
            let plugin_kind = parse_plugin_kind(&document.plugin.kind)?;
            if let Some(allowlist) = allowlist
                && !allowlist.contains(&document.plugin.id)
            {
                continue;
            }
            let Some(relative_library_path) = document.artifacts.get(&current_artifact_key()) else {
                continue;
            };
            let library_path = manifest_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(relative_library_path);
            packages.insert(
                document.plugin.id.clone(),
                PluginPackage {
                    plugin_id: document.plugin.id,
                    plugin_kind,
                    source: PluginSource::DynamicLibrary {
                        manifest_path,
                        library_path,
                    },
                },
            );
        }

        Ok(Self { packages })
    }

    pub fn register_in_process_protocol_plugin(&mut self, plugin: InProcessProtocolPlugin) {
        self.packages.insert(
            plugin.plugin_id.clone(),
            PluginPackage {
                plugin_id: plugin.plugin_id.clone(),
                plugin_kind: PluginKind::Protocol,
                source: PluginSource::InProcess(plugin),
            },
        );
    }

    fn packages(&self) -> impl Iterator<Item = &PluginPackage> {
        self.packages.values()
    }
}

#[derive(Deserialize)]
struct PluginPackageDocument {
    plugin: PluginPackageMetadata,
    artifacts: HashMap<String, String>,
}

#[derive(Deserialize)]
struct PluginPackageMetadata {
    id: String,
    kind: String,
}

fn parse_plugin_kind(value: &str) -> Result<PluginKind, RuntimeError> {
    match value {
        "protocol" => Ok(PluginKind::Protocol),
        "storage" => Ok(PluginKind::Storage),
        "auth" => Ok(PluginKind::Auth),
        "gameplay" => Ok(PluginKind::Gameplay),
        _ => Err(RuntimeError::Config(format!(
            "unsupported plugin kind `{value}`"
        ))),
    }
}

fn current_artifact_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[derive(Default)]
pub struct GenerationManager {
    next_generation_id: Mutex<u64>,
}

impl GenerationManager {
    fn next_generation_id(&self) -> PluginGenerationId {
        let mut next = self
            .next_generation_id
            .lock()
            .expect("plugin generation mutex should not be poisoned");
        let generation = PluginGenerationId(*next);
        *next = next.saturating_add(1);
        generation
    }
}

#[derive(Default)]
pub struct QuarantineManager {
    reasons: Mutex<HashMap<String, String>>,
}

impl QuarantineManager {
    fn quarantine(&self, plugin_id: &str, reason: impl Into<String>) {
        self.reasons
            .lock()
            .expect("quarantine mutex should not be poisoned")
            .insert(plugin_id.to_string(), reason.into());
    }

    fn is_quarantined(&self, plugin_id: &str) -> bool {
        self.reasons
            .lock()
            .expect("quarantine mutex should not be poisoned")
            .contains_key(plugin_id)
    }

    pub fn reason(&self, plugin_id: &str) -> Option<String> {
        self.reasons
            .lock()
            .expect("quarantine mutex should not be poisoned")
            .get(plugin_id)
            .cloned()
    }
}

#[derive(Clone)]
struct ProtocolGeneration {
    generation_id: PluginGenerationId,
    plugin_id: String,
    descriptor: ProtocolDescriptor,
    capabilities: CapabilitySet,
    invoke: mc_plugin_api::PluginInvokeFn,
    free_buffer: mc_plugin_api::PluginFreeBufferFn,
    _library_guard: Option<Arc<Mutex<Library>>>,
}

impl ProtocolGeneration {
    fn invoke(&self, request: ProtocolRequest) -> Result<ProtocolResponse, ProtocolError> {
        let request_bytes = serde_json::to_vec(&request)
            .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            (self.invoke)(
                mc_plugin_api::ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &mut output,
                &mut error,
            )
        };
        if status != PluginErrorCode::Ok {
            let message = if error.ptr.is_null() {
                format!("plugin `{}` returned {status:?}", self.plugin_id)
            } else {
                let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
                unsafe {
                    (self.free_buffer)(error);
                }
                String::from_utf8(bytes)
                    .unwrap_or_else(|_| format!("plugin `{}` returned invalid utf-8", self.plugin_id))
            };
            return Err(ProtocolError::Plugin(message));
        }

        let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
        unsafe {
            (self.free_buffer)(output);
        }
        serde_json::from_slice(&response_bytes).map_err(|error| ProtocolError::Plugin(error.to_string()))
    }
}

pub struct PluginLoader {
    abi_range: PluginAbiRange,
}

impl PluginLoader {
    #[must_use]
    pub fn new(abi_range: PluginAbiRange) -> Self {
        Self { abi_range }
    }

    fn load_protocol_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
    ) -> Result<ProtocolGeneration, RuntimeError> {
        let (guard, manifest, api) = match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                self.load_dynamic_protocol(library_path)?
            },
            PluginSource::InProcess(plugin) => (None, decode_manifest(plugin.manifest)?, *plugin.api),
        };
        self.validate_manifest(package, &manifest)?;
        let descriptor = match invoke_protocol(&api, ProtocolRequest::Describe)? {
            ProtocolResponse::Descriptor(descriptor) => descriptor,
            other => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` returned unexpected describe payload: {other:?}",
                    package.plugin_id
                )))
            }
        };
        let capabilities = match invoke_protocol(&api, ProtocolRequest::CapabilitySet)? {
            ProtocolResponse::CapabilitySet(capabilities) => capabilities,
            ProtocolResponse::Empty => CapabilitySet::default(),
            other => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` returned unexpected capability payload: {other:?}",
                    package.plugin_id
                )))
            }
        };
        Ok(ProtocolGeneration {
            generation_id,
            plugin_id: package.plugin_id.clone(),
            descriptor,
            capabilities,
            invoke: api.invoke,
            free_buffer: api.free_buffer,
            _library_guard: guard,
        })
    }

    unsafe fn load_dynamic_protocol(
        &self,
        library_path: &Path,
    ) -> Result<(Option<Arc<Mutex<Library>>>, DecodedManifest, ProtocolPluginApiV1), RuntimeError> {
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
        Ok((
            Some(library),
            decode_manifest(manifest_ptr)?,
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
        if !self.abi_range.contains(manifest.plugin_abi) {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` ABI {} is outside host range {}..={}",
                package.plugin_id,
                manifest.plugin_abi,
                self.abi_range.min,
                self.abi_range.max
            )));
        }
        if manifest.min_host_abi > CURRENT_PLUGIN_ABI || manifest.max_host_abi < CURRENT_PLUGIN_ABI
        {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` host ABI range {}..={} does not include {}",
                package.plugin_id,
                manifest.min_host_abi,
                manifest.max_host_abi,
                CURRENT_PLUGIN_ABI
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct DecodedManifest {
    plugin_id: String,
    plugin_kind: PluginKind,
    plugin_abi: PluginAbiVersion,
    min_host_abi: PluginAbiVersion,
    max_host_abi: PluginAbiVersion,
}

fn decode_manifest(manifest: *const PluginManifestV1) -> Result<DecodedManifest, RuntimeError> {
    let manifest = unsafe {
        manifest
            .as_ref()
            .ok_or_else(|| RuntimeError::Config("plugin manifest pointer was null".to_string()))?
    };
    Ok(DecodedManifest {
        plugin_id: decode_utf8_slice(manifest.plugin_id)?,
        plugin_kind: manifest.plugin_kind,
        plugin_abi: manifest.plugin_abi,
        min_host_abi: manifest.min_host_abi,
        max_host_abi: manifest.max_host_abi,
    })
}

fn decode_utf8_slice(slice: mc_plugin_api::Utf8Slice) -> Result<String, RuntimeError> {
    if slice.ptr.is_null() {
        return Err(RuntimeError::Config("plugin utf8 slice was null".to_string()));
    }
    let bytes = unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) };
    String::from_utf8(bytes.to_vec()).map_err(|error| RuntimeError::Config(error.to_string()))
}

fn invoke_protocol(
    api: &ProtocolPluginApiV1,
    request: ProtocolRequest,
) -> Result<ProtocolResponse, RuntimeError> {
    let request_bytes = serde_json::to_vec(&request)
        .map_err(|error| RuntimeError::Config(error.to_string()))?;
    let mut output = OwnedBuffer::empty();
    let mut error = OwnedBuffer::empty();
    let status = unsafe {
        (api.invoke)(
            mc_plugin_api::ByteSlice {
                ptr: request_bytes.as_ptr(),
                len: request_bytes.len(),
            },
            &mut output,
            &mut error,
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
    serde_json::from_slice(&response_bytes).map_err(|error| RuntimeError::Config(error.to_string()))
}

struct HotSwappableProtocolAdapter {
    plugin_id: String,
    generation: RwLock<Arc<ProtocolGeneration>>,
    quarantine: Arc<QuarantineManager>,
}

impl HotSwappableProtocolAdapter {
    fn new(
        plugin_id: String,
        generation: Arc<ProtocolGeneration>,
        quarantine: Arc<QuarantineManager>,
    ) -> Self {
        Self {
            plugin_id,
            generation: RwLock::new(generation),
            quarantine,
        }
    }

    fn current_generation(&self) -> Result<Arc<ProtocolGeneration>, ProtocolError> {
        if self.quarantine.is_quarantined(&self.plugin_id) {
            return Err(ProtocolError::Plugin(
                self.quarantine
                    .reason(&self.plugin_id)
                    .unwrap_or_else(|| "plugin quarantined".to_string()),
            ));
        }
        Ok(self
            .generation
            .read()
            .expect("protocol generation lock should not be poisoned")
            .clone())
    }

    fn swap_generation(&self, generation: Arc<ProtocolGeneration>) {
        *self
            .generation
            .write()
            .expect("protocol generation lock should not be poisoned") = generation;
    }

    fn quarantine_on_error<T>(&self, result: Result<T, ProtocolError>) -> Result<T, ProtocolError> {
        if let Err(ProtocolError::Plugin(message)) = &result {
            self.quarantine.quarantine(&self.plugin_id, message.clone());
        }
        result
    }
}

impl HandshakeProbe for HotSwappableProtocolAdapter {
    fn transport_kind(&self) -> TransportKind {
        self.current_generation()
            .map(|generation| generation.descriptor.transport)
            .unwrap_or(TransportKind::Tcp)
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(match generation.invoke(ProtocolRequest::TryRoute {
            frame: frame.to_vec(),
        })? {
            ProtocolResponse::HandshakeIntent(intent) => Ok(intent),
            other => Err(ProtocolError::Plugin(format!(
                "unexpected try_route response: {other:?}"
            ))),
        })
    }
}

impl mc_proto_common::SessionAdapter for HotSwappableProtocolAdapter {
    fn wire_codec(&self) -> &dyn WireCodec {
        static CODEC: MinecraftWireCodec = MinecraftWireCodec;
        &CODEC
    }

    fn decode_status(&self, frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(match generation.invoke(ProtocolRequest::DecodeStatus {
            frame: frame.to_vec(),
        })? {
            ProtocolResponse::StatusRequest(request) => Ok(request),
            other => Err(ProtocolError::Plugin(format!(
                "unexpected decode_status response: {other:?}"
            ))),
        })
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(match generation.invoke(ProtocolRequest::DecodeLogin {
            frame: frame.to_vec(),
        })? {
            ProtocolResponse::LoginRequest(request) => Ok(request),
            other => Err(ProtocolError::Plugin(format!(
                "unexpected decode_login response: {other:?}"
            ))),
        })
    }

    fn encode_status_response(&self, status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(match generation.invoke(ProtocolRequest::EncodeStatusResponse {
            status: status.clone(),
        })? {
            ProtocolResponse::Frame(frame) => Ok(frame),
            other => Err(ProtocolError::Plugin(format!(
                "unexpected encode_status_response payload: {other:?}"
            ))),
        })
    }

    fn encode_status_pong(&self, payload: i64) -> Result<Vec<u8>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(match generation.invoke(ProtocolRequest::EncodeStatusPong {
            payload,
        })? {
            ProtocolResponse::Frame(frame) => Ok(frame),
            other => Err(ProtocolError::Plugin(format!(
                "unexpected encode_status_pong payload: {other:?}"
            ))),
        })
    }

    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(match generation.invoke(ProtocolRequest::EncodeDisconnect {
            phase,
            reason: reason.to_string(),
        })? {
            ProtocolResponse::Frame(frame) => Ok(frame),
            other => Err(ProtocolError::Plugin(format!(
                "unexpected encode_disconnect payload: {other:?}"
            ))),
        })
    }

    fn encode_login_success(
        &self,
        player: &mc_core::PlayerSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(match generation.invoke(ProtocolRequest::EncodeLoginSuccess {
            player: player.clone(),
        })? {
            ProtocolResponse::Frame(frame) => Ok(frame),
            other => Err(ProtocolError::Plugin(format!(
                "unexpected encode_login_success payload: {other:?}"
            ))),
        })
    }
}

impl mc_proto_common::PlaySyncAdapter for HotSwappableProtocolAdapter {
    fn decode_play(
        &self,
        player_id: mc_core::PlayerId,
        frame: &[u8],
    ) -> Result<Option<mc_core::CoreCommand>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(match generation.invoke(ProtocolRequest::DecodePlay {
            player_id,
            frame: frame.to_vec(),
        })? {
            ProtocolResponse::CoreCommand(command) => Ok(command),
            other => Err(ProtocolError::Plugin(format!(
                "unexpected decode_play payload: {other:?}"
            ))),
        })
    }

    fn encode_play_event(
        &self,
        event: &mc_core::CoreEvent,
        context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(match generation.invoke(ProtocolRequest::EncodePlayEvent {
            event: event.clone(),
            context: *context,
        })? {
            ProtocolResponse::Frames(frames) => Ok(frames),
            other => Err(ProtocolError::Plugin(format!(
                "unexpected encode_play_event payload: {other:?}"
            ))),
        })
    }
}

impl ProtocolAdapter for HotSwappableProtocolAdapter {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.current_generation()
            .map(|generation| generation.descriptor.clone())
            .unwrap_or_else(|_| ProtocolDescriptor {
                adapter_id: self.plugin_id.clone(),
                transport: TransportKind::Tcp,
                edition: mc_proto_common::Edition::Je,
                version_name: "quarantined".to_string(),
                protocol_number: -1,
            })
    }

    fn capability_set(&self) -> CapabilitySet {
        self.current_generation()
            .map(|generation| generation.capabilities.clone())
            .unwrap_or_default()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.current_generation()
            .ok()
            .map(|generation| generation.generation_id)
    }
}

struct ManagedProtocolPlugin {
    package: PluginPackage,
    adapter: Arc<HotSwappableProtocolAdapter>,
    loaded_at: SystemTime,
}

pub struct PluginHost {
    catalog: PluginCatalog,
    loader: PluginLoader,
    generations: Arc<GenerationManager>,
    quarantine: Arc<QuarantineManager>,
    _failure_policy: PluginFailurePolicy,
    protocols: Mutex<HashMap<String, ManagedProtocolPlugin>>,
}

impl PluginHost {
    #[must_use]
    pub fn new(
        catalog: PluginCatalog,
        abi_range: PluginAbiRange,
        failure_policy: PluginFailurePolicy,
    ) -> Self {
        Self {
            catalog,
            loader: PluginLoader::new(abi_range),
            generations: Arc::new(GenerationManager::default()),
            quarantine: Arc::new(QuarantineManager::default()),
            _failure_policy: failure_policy,
            protocols: Mutex::new(HashMap::new()),
        }
    }

    pub fn load_into_registries(
        self: &Arc<Self>,
        registries: &mut RuntimeRegistries,
    ) -> Result<(), RuntimeError> {
        for package in self.catalog.packages() {
            match package.plugin_kind {
                PluginKind::Protocol => {
                    let generation = Arc::new(self.loader.load_protocol_generation(
                        package,
                        self.generations.next_generation_id(),
                    )?);
                    let adapter = Arc::new(HotSwappableProtocolAdapter::new(
                        package.plugin_id.clone(),
                        generation,
                        Arc::clone(&self.quarantine),
                    ));
                    registries.register_adapter(adapter.clone());
                    registries.register_probe(adapter.clone());
                    let loaded_at = package.modified_at()?;
                    self.protocols
                        .lock()
                        .expect("plugin host mutex should not be poisoned")
                        .insert(
                            package.plugin_id.clone(),
                            ManagedProtocolPlugin {
                                package: package.clone(),
                                adapter,
                                loaded_at,
                            },
                        );
                }
                PluginKind::Storage | PluginKind::Auth | PluginKind::Gameplay => {
                    self.quarantine.quarantine(
                        &package.plugin_id,
                        format!(
                            "{} plugin loading is registered but activation is deferred to a later phase",
                            match package.plugin_kind {
                                PluginKind::Storage => "storage",
                                PluginKind::Auth => "auth",
                                PluginKind::Gameplay => "gameplay",
                                PluginKind::Protocol => "protocol",
                            }
                        ),
                    );
                }
            }
        }
        registries.attach_plugin_host(Arc::clone(self));
        Ok(())
    }

    pub fn reload_modified(&self) -> Result<Vec<String>, RuntimeError> {
        let mut reloaded = Vec::new();
        let mut protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for managed in protocols.values_mut() {
            if managed.package.plugin_kind != PluginKind::Protocol {
                continue;
            }
            let modified_at = managed.package.modified_at()?;
            if modified_at <= managed.loaded_at {
                continue;
            }
            let generation = Arc::new(self.loader.load_protocol_generation(
                &managed.package,
                self.generations.next_generation_id(),
            )?);
            managed.adapter.swap_generation(generation);
            managed.loaded_at = modified_at;
            reloaded.push(managed.package.plugin_id.clone());
        }
        Ok(reloaded)
    }

    pub fn replace_in_process_protocol_plugin(
        &self,
        plugin: InProcessProtocolPlugin,
    ) -> Result<PluginGenerationId, RuntimeError> {
        let mut protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned");
        let managed = protocols.get_mut(&plugin.plugin_id).ok_or_else(|| {
            RuntimeError::Config(format!(
                "protocol plugin `{}` is not managed by this host",
                plugin.plugin_id
            ))
        })?;
        managed.package.source = PluginSource::InProcess(plugin);
        let generation_id = self.generations.next_generation_id();
        let generation = Arc::new(
            self.loader
                .load_protocol_generation(&managed.package, generation_id)?,
        );
        managed.adapter.swap_generation(generation);
        managed.loaded_at = managed.package.modified_at()?;
        Ok(generation_id)
    }

    pub fn quarantine_reason(&self, plugin_id: &str) -> Option<String> {
        self.quarantine.reason(plugin_id)
    }
}

pub fn plugin_host_from_config(
    config: &ServerConfig,
) -> Result<Option<Arc<PluginHost>>, RuntimeError> {
    let allowlist = config
        .plugin_allowlist
        .as_ref()
        .map(|entries| entries.iter().cloned().collect::<HashSet<_>>());
    let catalog = PluginCatalog::discover(&config.plugins_dir, allowlist.as_ref())?;
    if catalog.packages.is_empty() {
        return Ok(None);
    }
    Ok(Some(Arc::new(PluginHost::new(
        catalog,
        PluginAbiRange {
            min: config.plugin_abi_min,
            max: config.plugin_abi_max,
        },
        config.plugin_failure_policy,
    ))))
}

pub const fn plugin_reload_poll_interval_ms() -> u64 {
    PLUGIN_RELOAD_POLL_INTERVAL_MS
}

#[cfg(test)]
mod tests {
    use super::{
        InProcessProtocolPlugin, PluginAbiRange, PluginCatalog, PluginFailurePolicy, PluginHost,
        PluginPackage, PluginSource,
    };
    use crate::RuntimeRegistries;
    use mc_plugin_api::{CURRENT_PLUGIN_ABI, PluginAbiVersion, PluginKind, PluginManifestV1, Utf8Slice};
    use mc_plugin_proto_je_1_7_10::in_process_protocol_entrypoints;
    use std::sync::Arc;

    fn manifest_with_abi(
        plugin_id: &'static str,
        plugin_abi: PluginAbiVersion,
    ) -> &'static PluginManifestV1 {
        Box::leak(Box::new(PluginManifestV1 {
            plugin_id: Utf8Slice::from_static_str(plugin_id),
            display_name: Utf8Slice::from_static_str(plugin_id),
            plugin_kind: PluginKind::Protocol,
            plugin_abi,
            min_host_abi: CURRENT_PLUGIN_ABI,
            max_host_abi: CURRENT_PLUGIN_ABI,
            capabilities: std::ptr::null(),
            capabilities_len: 0,
        }))
    }

    #[test]
    fn in_process_protocol_plugin_swaps_generation() {
        let entrypoints = in_process_protocol_entrypoints();
        let mut catalog = PluginCatalog::default();
        catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
            plugin_id: "je-1_7_10".to_string(),
            manifest: entrypoints.manifest,
            api: entrypoints.api,
        });

        let host = Arc::new(PluginHost::new(
            catalog,
            PluginAbiRange::default(),
            PluginFailurePolicy::Quarantine,
        ));
        let mut registries = RuntimeRegistries::new();
        host.load_into_registries(&mut registries)
            .expect("in-process plugin should load");

        let adapter = registries
            .protocols()
            .resolve_adapter("je-1_7_10")
            .expect("registered plugin adapter should resolve");
        let first_generation = adapter
            .plugin_generation_id()
            .expect("plugin-backed adapter should report generation");

        let next_generation = host
            .replace_in_process_protocol_plugin(InProcessProtocolPlugin {
                plugin_id: "je-1_7_10".to_string(),
                manifest: entrypoints.manifest,
                api: entrypoints.api,
            })
            .expect("replacing in-process plugin should succeed");

        let adapter = registries
            .protocols()
            .resolve_adapter("je-1_7_10")
            .expect("registered plugin adapter should resolve");
        assert_eq!(adapter.plugin_generation_id(), Some(next_generation));
        assert_ne!(first_generation, next_generation);
    }

    #[test]
    fn abi_mismatch_is_rejected_before_registration() {
        let entrypoints = in_process_protocol_entrypoints();
        let mut catalog = PluginCatalog::default();
        catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
            plugin_id: "je-1_7_10".to_string(),
            manifest: manifest_with_abi(
                "je-1_7_10",
                PluginAbiVersion {
                    major: 9,
                    minor: 0,
                },
            ),
            api: entrypoints.api,
        });
        let host = Arc::new(PluginHost::new(
            catalog,
            PluginAbiRange::default(),
            PluginFailurePolicy::Quarantine,
        ));
        let mut registries = RuntimeRegistries::new();

        let error = host
            .load_into_registries(&mut registries)
            .expect_err("ABI mismatch should fail before registration");
        assert!(matches!(
            error,
            crate::RuntimeError::Config(message) if message.contains("ABI")
        ));
    }

    #[test]
    fn non_protocol_plugins_are_quarantined_until_later_phase() {
        let mut catalog = PluginCatalog::default();
        catalog.packages.insert(
            "future-storage".to_string(),
            PluginPackage {
                plugin_id: "future-storage".to_string(),
                plugin_kind: PluginKind::Storage,
                source: PluginSource::DynamicLibrary {
                    manifest_path: "dummy.toml".into(),
                    library_path: "dummy.so".into(),
                },
            },
        );
        let host = Arc::new(PluginHost::new(
            catalog,
            PluginAbiRange::default(),
            PluginFailurePolicy::Quarantine,
        ));
        let mut registries = RuntimeRegistries::new();
        host.load_into_registries(&mut registries)
            .expect("phase-gated plugin kinds should not hard fail");

        let reason = host
            .quarantine_reason("future-storage")
            .expect("phase-gated plugin should be quarantined");
        assert!(reason.contains("deferred"));
    }
}
