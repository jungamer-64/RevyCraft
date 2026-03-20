use crate::RuntimeError;
use crate::config::ServerConfig;
use crate::registry::RuntimeRegistries;
use crate::runtime::RuntimeReloadContext;
use libloading::Library;
use mc_core::{
    CapabilitySet, GameplayEffect, GameplayJoinEffect, GameplayPolicyResolver, GameplayProfileId,
    GameplayQuery, PlayerId, PlayerSnapshot, PluginGenerationId, SessionCapabilitySet,
    WorldSnapshot,
};
use mc_plugin_api::{
    AuthMode, AuthPluginApiV1, AuthRequest, AuthResponse, BedrockAuthResult, CURRENT_PLUGIN_ABI,
    GameplayPluginApiV1, GameplayRequest, GameplayResponse, GameplaySessionSnapshot,
    HostApiTableV1, OwnedBuffer, PLUGIN_AUTH_API_SYMBOL_V1, PLUGIN_GAMEPLAY_API_SYMBOL_V1,
    PLUGIN_MANIFEST_SYMBOL_V1, PLUGIN_PROTOCOL_API_SYMBOL_V1, PLUGIN_STORAGE_API_SYMBOL_V1,
    PluginAbiVersion, PluginErrorCode, PluginKind, PluginManifestV1, ProtocolPluginApiV1,
    ProtocolRequest, ProtocolResponse, StoragePluginApiV1, StorageRequest, StorageResponse,
    decode_auth_response, decode_gameplay_response, decode_protocol_response,
    decode_storage_response, encode_auth_request, encode_gameplay_request, encode_protocol_request,
    encode_storage_request,
};
use mc_proto_common::{
    BedrockListenerDescriptor, ConnectionPhase, HandshakeIntent, HandshakeProbe, LoginRequest,
    MinecraftWireCodec, PlayEncodingContext, ProtocolAdapter, ProtocolDescriptor, ProtocolError,
    RawPacketStreamWireCodec, ServerListStatus, StatusRequest, StorageError, TransportKind,
    WireCodec, WireFormatKind,
};
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

#[path = "plugin_host/activation.rs"]
mod activation;
#[path = "plugin_host/loader.rs"]
mod loader;
#[path = "plugin_host/reload.rs"]
mod reload;
#[path = "plugin_host/support.rs"]
mod support;

use self::support::{
    DecodedManifest, decode_manifest, decode_utf8_slice, ensure_known_profiles,
    ensure_profile_known, expect_auth_capabilities, expect_auth_descriptor,
    expect_gameplay_capabilities, expect_gameplay_descriptor,
    expect_protocol_bedrock_listener_descriptor, expect_protocol_capabilities,
    expect_protocol_descriptor, expect_storage_capabilities, expect_storage_descriptor,
    gameplay_profile_id_from_manifest, import_storage_runtime_state, invoke_auth, invoke_gameplay,
    invoke_protocol, invoke_storage, manifest_profile_id, migrate_gameplay_sessions,
    require_manifest_capability,
};

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

#[derive(Clone, Debug)]
pub struct InProcessProtocolPlugin {
    pub plugin_id: String,
    pub manifest: &'static PluginManifestV1,
    pub api: &'static ProtocolPluginApiV1,
}

#[derive(Clone, Debug)]
pub struct InProcessGameplayPlugin {
    pub plugin_id: String,
    pub manifest: &'static PluginManifestV1,
    pub api: &'static GameplayPluginApiV1,
}

#[derive(Clone, Debug)]
pub struct InProcessStoragePlugin {
    pub plugin_id: String,
    pub manifest: &'static PluginManifestV1,
    pub api: &'static StoragePluginApiV1,
}

#[derive(Clone, Debug)]
pub struct InProcessAuthPlugin {
    pub plugin_id: String,
    pub manifest: &'static PluginManifestV1,
    pub api: &'static AuthPluginApiV1,
}

#[derive(Clone, Debug)]
enum PluginSource {
    DynamicLibrary {
        manifest_path: PathBuf,
        library_path: PathBuf,
    },
    InProcessProtocol(InProcessProtocolPlugin),
    InProcessGameplay(InProcessGameplayPlugin),
    InProcessStorage(InProcessStoragePlugin),
    InProcessAuth(InProcessAuthPlugin),
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
            } => Ok(fs::metadata(manifest_path)?
                .modified()?
                .max(fs::metadata(library_path)?.modified()?)),
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAuth(_) => Ok(SystemTime::UNIX_EPOCH),
        }
    }

    fn refresh_dynamic_manifest(&mut self) -> Result<(), RuntimeError> {
        let PluginSource::DynamicLibrary {
            manifest_path,
            library_path,
        } = &mut self.source
        else {
            return Ok(());
        };
        let document: PluginPackageDocument = toml::from_str(&fs::read_to_string(&*manifest_path)?)
            .map_err(|error| {
                RuntimeError::Config(format!(
                    "failed to parse plugin manifest {}: {error}",
                    manifest_path.display()
                ))
            })?;
        let plugin_kind = parse_plugin_kind(&document.plugin.kind)?;
        if document.plugin.id != self.plugin_id {
            return Err(RuntimeError::Config(format!(
                "plugin manifest id `{}` does not match package id `{}`",
                document.plugin.id, self.plugin_id
            )));
        }
        if plugin_kind != self.plugin_kind {
            return Err(RuntimeError::Config(format!(
                "plugin `{}` manifest kind mismatch",
                self.plugin_id
            )));
        }
        let relative_library_path =
            document
                .artifacts
                .get(&current_artifact_key())
                .ok_or_else(|| {
                    RuntimeError::Config(format!(
                        "plugin `{}` does not provide an artifact for {}",
                        self.plugin_id,
                        current_artifact_key()
                    ))
                })?;
        *library_path = manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(relative_library_path);
        Ok(())
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
            if let Some(package) = discover_dynamic_plugin_package(&entry.path(), allowlist)? {
                packages.insert(package.plugin_id.clone(), package);
            }
        }

        Ok(Self { packages })
    }

    pub fn register_in_process_protocol_plugin(&mut self, plugin: InProcessProtocolPlugin) {
        self.packages.insert(
            plugin.plugin_id.clone(),
            PluginPackage {
                plugin_id: plugin.plugin_id.clone(),
                plugin_kind: PluginKind::Protocol,
                source: PluginSource::InProcessProtocol(plugin),
            },
        );
    }

    pub fn register_in_process_gameplay_plugin(&mut self, plugin: InProcessGameplayPlugin) {
        self.packages.insert(
            plugin.plugin_id.clone(),
            PluginPackage {
                plugin_id: plugin.plugin_id.clone(),
                plugin_kind: PluginKind::Gameplay,
                source: PluginSource::InProcessGameplay(plugin),
            },
        );
    }

    pub fn register_in_process_storage_plugin(&mut self, plugin: InProcessStoragePlugin) {
        self.packages.insert(
            plugin.plugin_id.clone(),
            PluginPackage {
                plugin_id: plugin.plugin_id.clone(),
                plugin_kind: PluginKind::Storage,
                source: PluginSource::InProcessStorage(plugin),
            },
        );
    }

    pub fn register_in_process_auth_plugin(&mut self, plugin: InProcessAuthPlugin) {
        self.packages.insert(
            plugin.plugin_id.clone(),
            PluginPackage {
                plugin_id: plugin.plugin_id.clone(),
                plugin_kind: PluginKind::Auth,
                source: PluginSource::InProcessAuth(plugin),
            },
        );
    }

    fn packages(&self) -> impl Iterator<Item = &PluginPackage> {
        self.packages.values()
    }
}

fn discover_dynamic_plugin_package(
    plugin_dir: &Path,
    allowlist: Option<&HashSet<String>>,
) -> Result<Option<PluginPackage>, RuntimeError> {
    if !plugin_dir.is_dir() {
        return Ok(None);
    }
    let manifest_path = plugin_dir.join("plugin.toml");
    if !manifest_path.exists() {
        return Ok(None);
    }
    let document = parse_plugin_package_document(&manifest_path)?;
    if let Some(allowlist) = allowlist
        && !allowlist.contains(&document.plugin.id)
    {
        return Ok(None);
    }
    let Some(relative_library_path) = document.artifacts.get(&current_artifact_key()) else {
        return Ok(None);
    };
    Ok(Some(PluginPackage {
        plugin_id: document.plugin.id.clone(),
        plugin_kind: parse_plugin_kind(&document.plugin.kind)?,
        source: PluginSource::DynamicLibrary {
            manifest_path: manifest_path.clone(),
            library_path: manifest_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(relative_library_path),
        },
    }))
}

fn parse_plugin_package_document(
    manifest_path: &Path,
) -> Result<PluginPackageDocument, RuntimeError> {
    toml::from_str(&fs::read_to_string(manifest_path)?).map_err(|error| {
        RuntimeError::Config(format!(
            "failed to parse plugin manifest {}: {error}",
            manifest_path.display()
        ))
    })
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
    bedrock_listener_descriptor: Option<BedrockListenerDescriptor>,
    capabilities: CapabilitySet,
    invoke: mc_plugin_api::PluginInvokeFn,
    free_buffer: mc_plugin_api::PluginFreeBufferFn,
    _library_guard: Option<Arc<Mutex<Library>>>,
}

fn decode_plugin_error(
    plugin_id: &str,
    status: PluginErrorCode,
    free_buffer: mc_plugin_api::PluginFreeBufferFn,
    error: OwnedBuffer,
) -> String {
    if error.ptr.is_null() {
        format!("plugin `{plugin_id}` returned {status:?}")
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(error.ptr, error.len) }.to_vec();
        unsafe {
            (free_buffer)(error);
        }
        String::from_utf8(bytes)
            .unwrap_or_else(|_| format!("plugin `{plugin_id}` returned invalid utf-8"))
    }
}

impl ProtocolGeneration {
    fn invoke(&self, request: ProtocolRequest) -> Result<ProtocolResponse, ProtocolError> {
        let request_bytes = encode_protocol_request(&request)
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
            return Err(ProtocolError::Plugin(decode_plugin_error(
                &self.plugin_id,
                status,
                self.free_buffer,
                error,
            )));
        }

        let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
        unsafe {
            (self.free_buffer)(output);
        }
        decode_protocol_response(&request, &response_bytes)
            .map_err(|error| ProtocolError::Plugin(error.to_string()))
    }
}

thread_local! {
    static CURRENT_GAMEPLAY_QUERY: RefCell<Option<*const dyn GameplayQuery>> = RefCell::new(None);
}

fn with_gameplay_query<T>(
    query: &dyn GameplayQuery,
    f: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    CURRENT_GAMEPLAY_QUERY.with(|slot| {
        // The pointer never outlives this closure; callbacks run synchronously inside `f`.
        let query_ptr = unsafe {
            std::mem::transmute::<*const dyn GameplayQuery, *const dyn GameplayQuery>(
                query as *const dyn GameplayQuery,
            )
        };
        let previous = slot.replace(Some(query_ptr));
        let result = f();
        let _ = slot.replace(previous);
        result
    })
}

fn with_current_gameplay_query<T>(
    f: impl FnOnce(&dyn GameplayQuery) -> Result<T, String>,
) -> Result<T, String> {
    CURRENT_GAMEPLAY_QUERY.with(|slot| {
        let query =
            slot.borrow().as_ref().copied().ok_or_else(|| {
                "gameplay host callback invoked without an active query".to_string()
            })?;
        let query = unsafe { &*query };
        f(query)
    })
}

unsafe extern "C" fn gameplay_host_log(level: u32, message: mc_plugin_api::Utf8Slice) {
    if let Ok(message) = decode_utf8_slice(message) {
        eprintln!("gameplay[{level}]: {message}");
    }
}

unsafe extern "C" fn gameplay_host_read_player_snapshot(
    _context: *mut std::ffi::c_void,
    payload: mc_plugin_api::ByteSlice,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let payload = unsafe { std::slice::from_raw_parts(payload.ptr, payload.len) };
    let result = with_current_gameplay_query(|query| {
        let player_id = mc_plugin_api::decode_host_player_id_blob(payload)
            .map_err(|error| error.to_string())?;
        let bytes = mc_plugin_api::encode_host_player_snapshot_blob(
            query.player_snapshot(player_id).as_ref(),
        )
        .map_err(|error| error.to_string())?;
        write_owned_buffer(output, bytes);
        Ok(())
    });
    match result {
        Ok(()) => PluginErrorCode::Ok,
        Err(error) => {
            write_error_buffer(error_out, error);
            PluginErrorCode::Internal
        }
    }
}

unsafe extern "C" fn gameplay_host_read_world_meta(
    _context: *mut std::ffi::c_void,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let result = with_current_gameplay_query(|query| {
        let world_meta = query.world_meta();
        let bytes = mc_plugin_api::encode_host_world_meta_blob(&world_meta)
            .map_err(|error| error.to_string())?;
        write_owned_buffer(output, bytes);
        Ok(())
    });
    match result {
        Ok(()) => PluginErrorCode::Ok,
        Err(error) => {
            write_error_buffer(error_out, error);
            PluginErrorCode::Internal
        }
    }
}

unsafe extern "C" fn gameplay_host_read_block_state(
    _context: *mut std::ffi::c_void,
    payload: mc_plugin_api::ByteSlice,
    output: *mut OwnedBuffer,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let payload = unsafe { std::slice::from_raw_parts(payload.ptr, payload.len) };
    let result = with_current_gameplay_query(|query| {
        let position = mc_plugin_api::decode_host_block_pos_blob(payload)
            .map_err(|error| error.to_string())?;
        let bytes = mc_plugin_api::encode_host_block_state_blob(&query.block_state(position))
            .map_err(|error| error.to_string())?;
        write_owned_buffer(output, bytes);
        Ok(())
    });
    match result {
        Ok(()) => PluginErrorCode::Ok,
        Err(error) => {
            write_error_buffer(error_out, error);
            PluginErrorCode::Internal
        }
    }
}

unsafe extern "C" fn gameplay_host_can_edit_block(
    _context: *mut std::ffi::c_void,
    payload: mc_plugin_api::ByteSlice,
    out: *mut bool,
    error_out: *mut OwnedBuffer,
) -> PluginErrorCode {
    let payload = unsafe { std::slice::from_raw_parts(payload.ptr, payload.len) };
    let result = with_current_gameplay_query(|query| {
        let (player_id, position) = mc_plugin_api::decode_host_can_edit_block_key(payload)
            .map_err(|error| error.to_string())?;
        if !out.is_null() {
            unsafe {
                *out = query.can_edit_block(player_id, position);
            }
        }
        Ok(())
    });
    match result {
        Ok(()) => PluginErrorCode::Ok,
        Err(error) => {
            write_error_buffer(error_out, error);
            PluginErrorCode::Internal
        }
    }
}

fn gameplay_host_api() -> HostApiTableV1 {
    HostApiTableV1 {
        abi: CURRENT_PLUGIN_ABI,
        context: std::ptr::null_mut(),
        log: Some(gameplay_host_log),
        read_player_snapshot: Some(gameplay_host_read_player_snapshot),
        read_world_meta: Some(gameplay_host_read_world_meta),
        read_block_state: Some(gameplay_host_read_block_state),
        can_edit_block: Some(gameplay_host_can_edit_block),
    }
}

fn write_owned_buffer(output: *mut OwnedBuffer, mut bytes: Vec<u8>) {
    if output.is_null() {
        return;
    }
    unsafe {
        *output = OwnedBuffer {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            cap: bytes.capacity(),
        };
        std::mem::forget(bytes);
    }
}

fn write_error_buffer(error_out: *mut OwnedBuffer, message: String) {
    write_owned_buffer(error_out, message.into_bytes());
}

#[derive(Clone)]
pub(crate) struct GameplayGeneration {
    generation_id: PluginGenerationId,
    plugin_id: String,
    profile_id: GameplayProfileId,
    capabilities: CapabilitySet,
    invoke: mc_plugin_api::PluginInvokeFn,
    free_buffer: mc_plugin_api::PluginFreeBufferFn,
    _library_guard: Option<Arc<Mutex<Library>>>,
}

impl GameplayGeneration {
    fn invoke(&self, request: GameplayRequest) -> Result<GameplayResponse, String> {
        let request_bytes = encode_gameplay_request(&request).map_err(|error| error.to_string())?;
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
            return Err(decode_plugin_error(
                &self.plugin_id,
                status,
                self.free_buffer,
                error,
            ));
        }
        let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
        unsafe {
            (self.free_buffer)(output);
        }
        decode_gameplay_response(&request, &response_bytes).map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
struct StorageGeneration {
    #[allow(dead_code)]
    generation_id: PluginGenerationId,
    plugin_id: String,
    profile_id: String,
    #[allow(dead_code)]
    capabilities: CapabilitySet,
    invoke: mc_plugin_api::PluginInvokeFn,
    free_buffer: mc_plugin_api::PluginFreeBufferFn,
    _library_guard: Option<Arc<Mutex<Library>>>,
}

impl StorageGeneration {
    fn invoke(&self, request: StorageRequest) -> Result<StorageResponse, StorageError> {
        let request_bytes = encode_storage_request(&request)
            .map_err(|error| StorageError::Plugin(error.to_string()))?;
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
            return Err(StorageError::Plugin(decode_plugin_error(
                &self.plugin_id,
                status,
                self.free_buffer,
                error,
            )));
        }
        let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
        unsafe {
            (self.free_buffer)(output);
        }
        decode_storage_response(&request, &response_bytes)
            .map_err(|error| StorageError::Plugin(error.to_string()))
    }
}

#[derive(Clone)]
pub(crate) struct AuthGeneration {
    pub(crate) generation_id: PluginGenerationId,
    plugin_id: String,
    profile_id: String,
    mode: AuthMode,
    #[allow(dead_code)]
    capabilities: CapabilitySet,
    invoke: mc_plugin_api::PluginInvokeFn,
    free_buffer: mc_plugin_api::PluginFreeBufferFn,
    _library_guard: Option<Arc<Mutex<Library>>>,
}

impl AuthGeneration {
    fn invoke(&self, request: AuthRequest) -> Result<AuthResponse, String> {
        let request_bytes = encode_auth_request(&request).map_err(|error| error.to_string())?;
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
            return Err(decode_plugin_error(
                &self.plugin_id,
                status,
                self.free_buffer,
                error,
            ));
        }
        let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
        unsafe {
            (self.free_buffer)(output);
        }
        decode_auth_response(&request, &response_bytes).map_err(|error| error.to_string())
    }

    pub(crate) fn mode(&self) -> AuthMode {
        self.mode
    }

    pub(crate) fn authenticate_offline(&self, username: &str) -> Result<PlayerId, RuntimeError> {
        match self
            .invoke(AuthRequest::AuthenticateOffline {
                username: username.to_string(),
            })
            .map_err(RuntimeError::Config)?
        {
            AuthResponse::AuthenticatedPlayer(player_id) => Ok(player_id),
            other => Err(RuntimeError::Config(format!(
                "unexpected auth authenticate_offline payload: {other:?}"
            ))),
        }
    }

    pub(crate) fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, RuntimeError> {
        match self
            .invoke(AuthRequest::AuthenticateOnline {
                username: username.to_string(),
                server_hash: server_hash.to_string(),
            })
            .map_err(RuntimeError::Config)?
        {
            AuthResponse::AuthenticatedPlayer(player_id) => Ok(player_id),
            other => Err(RuntimeError::Config(format!(
                "unexpected auth authenticate_online payload: {other:?}"
            ))),
        }
    }

    pub(crate) fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .invoke(AuthRequest::AuthenticateBedrockOffline {
                display_name: display_name.to_string(),
            })
            .map_err(RuntimeError::Config)?
        {
            AuthResponse::AuthenticatedBedrockPlayer(result) => Ok(result),
            other => Err(RuntimeError::Config(format!(
                "unexpected auth authenticate_bedrock_offline payload: {other:?}"
            ))),
        }
    }

    pub(crate) fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .invoke(AuthRequest::AuthenticateBedrockXbl {
                chain_jwts: chain_jwts.to_vec(),
                client_data_jwt: client_data_jwt.to_string(),
            })
            .map_err(RuntimeError::Config)?
        {
            AuthResponse::AuthenticatedBedrockPlayer(result) => Ok(result),
            other => Err(RuntimeError::Config(format!(
                "unexpected auth authenticate_bedrock_xbl payload: {other:?}"
            ))),
        }
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
        self.quarantine_on_error(
            match generation.invoke(ProtocolRequest::TryRoute {
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::HandshakeIntent(intent) => Ok(intent),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected try_route response: {other:?}"
                ))),
            },
        )
    }
}

impl mc_proto_common::SessionAdapter for HotSwappableProtocolAdapter {
    fn wire_codec(&self) -> &dyn WireCodec {
        static MINECRAFT_CODEC: MinecraftWireCodec = MinecraftWireCodec;
        static RAW_PACKET_STREAM_CODEC: RawPacketStreamWireCodec = RawPacketStreamWireCodec;
        match self
            .current_generation()
            .map(|generation| generation.descriptor.wire_format)
            .unwrap_or(WireFormatKind::MinecraftFramed)
        {
            WireFormatKind::MinecraftFramed => &MINECRAFT_CODEC,
            WireFormatKind::RawPacketStream => &RAW_PACKET_STREAM_CODEC,
        }
    }

    fn decode_status(&self, frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(
            match generation.invoke(ProtocolRequest::DecodeStatus {
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::StatusRequest(request) => Ok(request),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected decode_status response: {other:?}"
                ))),
            },
        )
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(
            match generation.invoke(ProtocolRequest::DecodeLogin {
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::LoginRequest(request) => Ok(request),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected decode_login response: {other:?}"
                ))),
            },
        )
    }

    fn encode_status_response(&self, status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(
            match generation.invoke(ProtocolRequest::EncodeStatusResponse {
                status: status.clone(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_status_response payload: {other:?}"
                ))),
            },
        )
    }

    fn encode_status_pong(&self, payload: i64) -> Result<Vec<u8>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(
            match generation.invoke(ProtocolRequest::EncodeStatusPong { payload })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_status_pong payload: {other:?}"
                ))),
            },
        )
    }

    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(
            match generation.invoke(ProtocolRequest::EncodeDisconnect {
                phase,
                reason: reason.to_string(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_disconnect payload: {other:?}"
                ))),
            },
        )
    }

    fn encode_encryption_request(
        &self,
        server_id: &str,
        public_key_der: &[u8],
        verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(
            match generation.invoke(ProtocolRequest::EncodeEncryptionRequest {
                server_id: server_id.to_string(),
                public_key_der: public_key_der.to_vec(),
                verify_token: verify_token.to_vec(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_encryption_request payload: {other:?}"
                ))),
            },
        )
    }

    fn encode_network_settings(
        &self,
        compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(
            match generation.invoke(ProtocolRequest::EncodeNetworkSettings {
                compression_threshold,
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_network_settings payload: {other:?}"
                ))),
            },
        )
    }

    fn encode_login_success(
        &self,
        player: &mc_core::PlayerSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(
            match generation.invoke(ProtocolRequest::EncodeLoginSuccess {
                player: player.clone(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_login_success payload: {other:?}"
                ))),
            },
        )
    }
}

impl mc_proto_common::PlaySyncAdapter for HotSwappableProtocolAdapter {
    fn decode_play(
        &self,
        player_id: mc_core::PlayerId,
        frame: &[u8],
    ) -> Result<Option<mc_core::CoreCommand>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(
            match generation.invoke(ProtocolRequest::DecodePlay {
                player_id,
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::CoreCommand(command) => Ok(command),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected decode_play payload: {other:?}"
                ))),
            },
        )
    }

    fn encode_play_event(
        &self,
        event: &mc_core::CoreEvent,
        context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let generation = self.current_generation()?;
        self.quarantine_on_error(
            match generation.invoke(ProtocolRequest::EncodePlayEvent {
                event: event.clone(),
                context: *context,
            })? {
                ProtocolResponse::Frames(frames) => Ok(frames),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_play_event payload: {other:?}"
                ))),
            },
        )
    }
}

impl ProtocolAdapter for HotSwappableProtocolAdapter {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.current_generation()
            .map(|generation| generation.descriptor.clone())
            .unwrap_or_else(|_| ProtocolDescriptor {
                adapter_id: self.plugin_id.clone(),
                transport: TransportKind::Tcp,
                wire_format: WireFormatKind::MinecraftFramed,
                edition: mc_proto_common::Edition::Je,
                version_name: "quarantined".to_string(),
                protocol_number: -1,
            })
    }

    fn bedrock_listener_descriptor(&self) -> Option<BedrockListenerDescriptor> {
        self.current_generation()
            .ok()
            .and_then(|generation| generation.bedrock_listener_descriptor.clone())
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

pub(crate) struct HotSwappableGameplayProfile {
    plugin_id: String,
    profile_id: GameplayProfileId,
    generation: RwLock<Arc<GameplayGeneration>>,
    quarantine: Arc<QuarantineManager>,
    reload_gate: RwLock<()>,
}

impl HotSwappableGameplayProfile {
    fn new(
        plugin_id: String,
        profile_id: GameplayProfileId,
        generation: Arc<GameplayGeneration>,
        quarantine: Arc<QuarantineManager>,
    ) -> Self {
        Self {
            plugin_id,
            profile_id,
            generation: RwLock::new(generation),
            quarantine,
            reload_gate: RwLock::new(()),
        }
    }

    fn current_generation(&self) -> Result<Arc<GameplayGeneration>, String> {
        if self.quarantine.is_quarantined(&self.plugin_id) {
            return Err(self
                .quarantine
                .reason(&self.plugin_id)
                .unwrap_or_else(|| "plugin quarantined".to_string()));
        }
        Ok(self
            .generation
            .read()
            .expect("gameplay generation lock should not be poisoned")
            .clone())
    }

    fn swap_generation(&self, generation: Arc<GameplayGeneration>) {
        *self
            .generation
            .write()
            .expect("gameplay generation lock should not be poisoned") = generation;
    }

    pub(crate) fn profile_id(&self) -> GameplayProfileId {
        self.profile_id.clone()
    }

    pub(crate) fn capability_set(&self) -> CapabilitySet {
        self.current_generation()
            .map(|generation| generation.capabilities.clone())
            .unwrap_or_default()
    }

    pub(crate) fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.current_generation()
            .ok()
            .map(|generation| generation.generation_id)
    }

    #[expect(
        dead_code,
        reason = "host reload exports from the current generation under the reload gate"
    )]
    pub(crate) fn export_session_state(
        &self,
        session: &GameplaySessionSnapshot,
    ) -> Result<Vec<u8>, RuntimeError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        let generation = self.current_generation().map_err(RuntimeError::Config)?;
        match generation
            .invoke(GameplayRequest::ExportSessionState {
                session: session.clone(),
            })
            .map_err(RuntimeError::Config)?
        {
            GameplayResponse::SessionTransferBlob(blob) => Ok(blob),
            other => Err(RuntimeError::Config(format!(
                "unexpected gameplay export payload: {other:?}"
            ))),
        }
    }

    #[expect(
        dead_code,
        reason = "host reload imports into the candidate generation before swap"
    )]
    pub(crate) fn import_session_state(
        &self,
        session: &GameplaySessionSnapshot,
        blob: &[u8],
    ) -> Result<(), RuntimeError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        let generation = self.current_generation().map_err(RuntimeError::Config)?;
        match generation
            .invoke(GameplayRequest::ImportSessionState {
                session: session.clone(),
                blob: blob.to_vec(),
            })
            .map_err(RuntimeError::Config)?
        {
            GameplayResponse::Empty => Ok(()),
            other => Err(RuntimeError::Config(format!(
                "unexpected gameplay import payload: {other:?}"
            ))),
        }
    }

    pub(crate) fn session_closed(
        &self,
        session: &GameplaySessionSnapshot,
    ) -> Result<(), RuntimeError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        let generation = self.current_generation().map_err(RuntimeError::Config)?;
        match generation
            .invoke(GameplayRequest::SessionClosed {
                session: session.clone(),
            })
            .map_err(RuntimeError::Config)?
        {
            GameplayResponse::Empty => Ok(()),
            other => Err(RuntimeError::Config(format!(
                "unexpected gameplay session_closed payload: {other:?}"
            ))),
        }
    }
}

impl GameplayPolicyResolver for HotSwappableGameplayProfile {
    fn handle_player_join(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        player: &PlayerSnapshot,
    ) -> Result<GameplayJoinEffect, String> {
        let session = GameplaySessionSnapshot {
            phase: ConnectionPhase::Login,
            player_id: Some(player.id),
            entity_id: None,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        let generation = self.current_generation()?;
        with_gameplay_query(query, || {
            match generation.invoke(GameplayRequest::HandlePlayerJoin {
                session,
                player: player.clone(),
            })? {
                GameplayResponse::JoinEffect(effect) => Ok(effect),
                other => Err(format!("unexpected gameplay join payload: {other:?}")),
            }
        })
    }

    fn handle_command(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        command: &mc_core::CoreCommand,
    ) -> Result<GameplayEffect, String> {
        let session = GameplaySessionSnapshot {
            phase: ConnectionPhase::Play,
            player_id: command.player_id(),
            entity_id: None,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        let generation = self.current_generation()?;
        with_gameplay_query(query, || {
            match generation.invoke(GameplayRequest::HandleCommand {
                session,
                command: command.clone(),
            })? {
                GameplayResponse::Effect(effect) => Ok(effect),
                other => Err(format!("unexpected gameplay command payload: {other:?}")),
            }
        })
    }

    fn handle_tick(
        &self,
        query: &dyn GameplayQuery,
        session: &SessionCapabilitySet,
        player_id: mc_core::PlayerId,
        now_ms: u64,
    ) -> Result<GameplayEffect, String> {
        let session = GameplaySessionSnapshot {
            phase: ConnectionPhase::Play,
            player_id: Some(player_id),
            entity_id: None,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        let generation = self.current_generation()?;
        with_gameplay_query(query, || {
            match generation.invoke(GameplayRequest::HandleTick { session, now_ms })? {
                GameplayResponse::Effect(effect) => Ok(effect),
                other => Err(format!("unexpected gameplay tick payload: {other:?}")),
            }
        })
    }
}

pub(crate) struct HotSwappableStorageProfile {
    plugin_id: String,
    #[allow(dead_code)]
    profile_id: String,
    generation: RwLock<Arc<StorageGeneration>>,
    quarantine: Arc<QuarantineManager>,
    reload_gate: RwLock<()>,
}

impl HotSwappableStorageProfile {
    fn new(
        plugin_id: String,
        profile_id: String,
        generation: Arc<StorageGeneration>,
        quarantine: Arc<QuarantineManager>,
    ) -> Self {
        Self {
            plugin_id,
            profile_id,
            generation: RwLock::new(generation),
            quarantine,
            reload_gate: RwLock::new(()),
        }
    }

    fn current_generation(&self) -> Result<Arc<StorageGeneration>, StorageError> {
        if self.quarantine.is_quarantined(&self.plugin_id) {
            return Err(StorageError::Plugin(
                self.quarantine
                    .reason(&self.plugin_id)
                    .unwrap_or_else(|| "plugin quarantined".to_string()),
            ));
        }
        Ok(self
            .generation
            .read()
            .expect("storage generation lock should not be poisoned")
            .clone())
    }

    fn swap_generation(&self, generation: Arc<StorageGeneration>) {
        *self
            .generation
            .write()
            .expect("storage generation lock should not be poisoned") = generation;
    }

    #[expect(
        dead_code,
        reason = "phase 4 tests introspect the active storage profile"
    )]
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    #[allow(dead_code)]
    pub(crate) fn capability_set(&self) -> CapabilitySet {
        self.current_generation()
            .map(|generation| generation.capabilities.clone())
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub(crate) fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.current_generation()
            .ok()
            .map(|generation| generation.generation_id)
    }

    pub(crate) fn load_snapshot(
        &self,
        world_dir: &Path,
    ) -> Result<Option<WorldSnapshot>, StorageError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("storage reload gate should not be poisoned");
        let generation = self.current_generation()?;
        match generation.invoke(StorageRequest::LoadSnapshot {
            world_dir: world_dir.display().to_string(),
        })? {
            StorageResponse::Snapshot(snapshot) => Ok(snapshot),
            other => Err(StorageError::Plugin(format!(
                "unexpected storage load_snapshot payload: {other:?}"
            ))),
        }
    }

    pub(crate) fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("storage reload gate should not be poisoned");
        let generation = self.current_generation()?;
        match generation.invoke(StorageRequest::SaveSnapshot {
            world_dir: world_dir.display().to_string(),
            snapshot: snapshot.clone(),
        })? {
            StorageResponse::Empty => Ok(()),
            other => Err(StorageError::Plugin(format!(
                "unexpected storage save_snapshot payload: {other:?}"
            ))),
        }
    }

    #[expect(
        dead_code,
        reason = "host reload imports into a candidate generation before swap"
    )]
    pub(crate) fn import_runtime_state(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("storage reload gate should not be poisoned");
        let generation = self.current_generation()?;
        match generation.invoke(StorageRequest::ImportRuntimeState {
            world_dir: world_dir.display().to_string(),
            snapshot: snapshot.clone(),
        })? {
            StorageResponse::Empty => Ok(()),
            other => Err(StorageError::Plugin(format!(
                "unexpected storage import_runtime_state payload: {other:?}"
            ))),
        }
    }
}

pub(crate) struct HotSwappableAuthProfile {
    plugin_id: String,
    #[allow(dead_code)]
    profile_id: String,
    generation: RwLock<Arc<AuthGeneration>>,
    quarantine: Arc<QuarantineManager>,
}

impl HotSwappableAuthProfile {
    fn new(
        plugin_id: String,
        profile_id: String,
        generation: Arc<AuthGeneration>,
        quarantine: Arc<QuarantineManager>,
    ) -> Self {
        Self {
            plugin_id,
            profile_id,
            generation: RwLock::new(generation),
            quarantine,
        }
    }

    fn current_generation(&self) -> Result<Arc<AuthGeneration>, String> {
        if self.quarantine.is_quarantined(&self.plugin_id) {
            return Err(self
                .quarantine
                .reason(&self.plugin_id)
                .unwrap_or_else(|| "plugin quarantined".to_string()));
        }
        Ok(self
            .generation
            .read()
            .expect("auth generation lock should not be poisoned")
            .clone())
    }

    fn swap_generation(&self, generation: Arc<AuthGeneration>) {
        *self
            .generation
            .write()
            .expect("auth generation lock should not be poisoned") = generation;
    }

    #[expect(dead_code, reason = "phase 4 tests introspect the active auth profile")]
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    #[allow(dead_code)]
    pub(crate) fn capability_set(&self) -> CapabilitySet {
        self.current_generation()
            .map(|generation| generation.capabilities.clone())
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub(crate) fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.current_generation()
            .ok()
            .map(|generation| generation.generation_id)
    }

    pub(crate) fn mode(&self) -> Result<AuthMode, RuntimeError> {
        self.current_generation()
            .map(|generation| generation.mode())
            .map_err(RuntimeError::Config)
    }

    pub(crate) fn capture_generation(&self) -> Result<Arc<AuthGeneration>, RuntimeError> {
        self.current_generation().map_err(RuntimeError::Config)
    }

    pub(crate) fn authenticate_offline(&self, username: &str) -> Result<PlayerId, RuntimeError> {
        self.capture_generation()?.authenticate_offline(username)
    }

    pub(crate) fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, RuntimeError> {
        self.capture_generation()?
            .authenticate_online(username, server_hash)
    }

    pub(crate) fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        self.capture_generation()?
            .authenticate_bedrock_offline(display_name)
    }

    pub(crate) fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        self.capture_generation()?
            .authenticate_bedrock_xbl(chain_jwts, client_data_jwt)
    }
}

struct ManagedProtocolPlugin {
    package: PluginPackage,
    adapter: Arc<HotSwappableProtocolAdapter>,
    loaded_at: SystemTime,
}

struct ManagedGameplayPlugin {
    package: PluginPackage,
    profile_id: GameplayProfileId,
    profile: Arc<HotSwappableGameplayProfile>,
    loaded_at: SystemTime,
}

struct ManagedStoragePlugin {
    package: PluginPackage,
    profile_id: String,
    profile: Arc<HotSwappableStorageProfile>,
    loaded_at: SystemTime,
}

struct ManagedAuthPlugin {
    package: PluginPackage,
    profile_id: String,
    profile: Arc<HotSwappableAuthProfile>,
    loaded_at: SystemTime,
}

pub struct PluginHost {
    catalog: PluginCatalog,
    loader: PluginLoader,
    generations: Arc<GenerationManager>,
    quarantine: Arc<QuarantineManager>,
    _failure_policy: PluginFailurePolicy,
    protocols: Mutex<HashMap<String, ManagedProtocolPlugin>>,
    gameplay: Mutex<HashMap<String, ManagedGameplayPlugin>>,
    storage: Mutex<HashMap<String, ManagedStoragePlugin>>,
    auth: Mutex<HashMap<String, ManagedAuthPlugin>>,
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
            gameplay: Mutex::new(HashMap::new()),
            storage: Mutex::new(HashMap::new()),
            auth: Mutex::new(HashMap::new()),
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
                PluginKind::Gameplay => {}
                PluginKind::Storage | PluginKind::Auth => {}
            }
        }
        registries.attach_plugin_host(Arc::clone(self));
        Ok(())
    }

    pub(crate) fn resolve_gameplay_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<HotSwappableGameplayProfile>> {
        self.gameplay
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .get(profile_id)
            .map(|managed| Arc::clone(&managed.profile))
    }

    pub(crate) fn resolve_storage_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<HotSwappableStorageProfile>> {
        self.storage
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .get(profile_id)
            .map(|managed| Arc::clone(&managed.profile))
    }

    pub(crate) fn resolve_auth_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<HotSwappableAuthProfile>> {
        self.auth
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .get(profile_id)
            .map(|managed| Arc::clone(&managed.profile))
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
        managed.package.source = PluginSource::InProcessProtocol(plugin);
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
mod tests;
