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
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let manifest_path = entry.path().join("plugin.toml");
            if !manifest_path.exists() {
                continue;
            }
            let document: PluginPackageDocument =
                toml::from_str(&fs::read_to_string(&manifest_path)?).map_err(|error| {
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
            let Some(relative_library_path) = document.artifacts.get(&current_artifact_key())
            else {
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

    fn load_protocol_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
    ) -> Result<ProtocolGeneration, RuntimeError> {
        let (guard, manifest, api) = match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                self.load_dynamic_protocol(library_path)?
            },
            PluginSource::InProcessProtocol(plugin) => {
                (None, decode_manifest(plugin.manifest)?, *plugin.api)
            }
            PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAuth(_) => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` is not a protocol plugin",
                    package.plugin_id
                )));
            }
        };
        self.validate_manifest(package, &manifest)?;
        let descriptor = match invoke_protocol(&api, ProtocolRequest::Describe)? {
            ProtocolResponse::Descriptor(descriptor) => descriptor,
            other => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` returned unexpected describe payload: {other:?}",
                    package.plugin_id
                )));
            }
        };
        let bedrock_listener_descriptor =
            match invoke_protocol(&api, ProtocolRequest::DescribeBedrockListener)? {
                ProtocolResponse::BedrockListenerDescriptor(descriptor) => descriptor,
                other => {
                    return Err(RuntimeError::Config(format!(
                        "plugin `{}` returned unexpected bedrock listener payload: {other:?}",
                        package.plugin_id
                    )));
                }
            };
        let capabilities = match invoke_protocol(&api, ProtocolRequest::CapabilitySet)? {
            ProtocolResponse::CapabilitySet(capabilities) => capabilities,
            other => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` returned unexpected capability payload: {other:?}",
                    package.plugin_id
                )));
            }
        };
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

    fn load_gameplay_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
    ) -> Result<GameplayGeneration, RuntimeError> {
        let (guard, manifest, api) = match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                self.load_dynamic_gameplay(library_path)?
            },
            PluginSource::InProcessGameplay(plugin) => {
                let status = unsafe { (plugin.api.set_host_api)(&gameplay_host_api()) };
                if status != PluginErrorCode::Ok {
                    return Err(RuntimeError::Config(format!(
                        "failed to configure gameplay host api for plugin `{}`: {status:?}",
                        package.plugin_id
                    )));
                }
                (None, decode_manifest(plugin.manifest)?, *plugin.api)
            }
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAuth(_) => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` is not a gameplay plugin",
                    package.plugin_id
                )));
            }
        };
        self.validate_manifest(package, &manifest)?;
        let profile_id = manifest
            .capabilities
            .iter()
            .find_map(|capability| capability.strip_prefix("gameplay.profile:"))
            .map(GameplayProfileId::new)
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "gameplay plugin `{}` is missing gameplay.profile:<id> manifest capability",
                    package.plugin_id
                ))
            })?;
        if !manifest
            .capabilities
            .iter()
            .any(|capability| capability == "runtime.reload.gameplay")
        {
            return Err(RuntimeError::Config(format!(
                "gameplay plugin `{}` is missing runtime.reload.gameplay capability",
                package.plugin_id
            )));
        }
        let descriptor = match invoke_gameplay(&package.plugin_id, &api, GameplayRequest::Describe)?
        {
            GameplayResponse::Descriptor(descriptor) => descriptor,
            other => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` returned unexpected gameplay describe payload: {other:?}",
                    package.plugin_id
                )));
            }
        };
        if descriptor.profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "gameplay plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id,
                descriptor.profile.as_str(),
                profile_id.as_str()
            )));
        }
        let capabilities =
            match invoke_gameplay(&package.plugin_id, &api, GameplayRequest::CapabilitySet)? {
                GameplayResponse::CapabilitySet(capabilities) => capabilities,
                other => {
                    return Err(RuntimeError::Config(format!(
                        "plugin `{}` returned unexpected gameplay capability payload: {other:?}",
                        package.plugin_id
                    )));
                }
            };
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

    fn load_storage_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
    ) -> Result<StorageGeneration, RuntimeError> {
        let (guard, manifest, api) = match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                self.load_dynamic_storage(library_path)?
            },
            PluginSource::InProcessStorage(plugin) => {
                (None, decode_manifest(plugin.manifest)?, *plugin.api)
            }
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessAuth(_) => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` is not a storage plugin",
                    package.plugin_id
                )));
            }
        };
        self.validate_manifest(package, &manifest)?;
        let profile_id = manifest
            .capabilities
            .iter()
            .find_map(|capability| capability.strip_prefix("storage.profile:"))
            .map(ToString::to_string)
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "storage plugin `{}` is missing storage.profile:<id> manifest capability",
                    package.plugin_id
                ))
            })?;
        if !manifest
            .capabilities
            .iter()
            .any(|capability| capability == "runtime.reload.storage")
        {
            return Err(RuntimeError::Config(format!(
                "storage plugin `{}` is missing runtime.reload.storage capability",
                package.plugin_id
            )));
        }
        let descriptor = match invoke_storage(&package.plugin_id, &api, StorageRequest::Describe)? {
            StorageResponse::Descriptor(descriptor) => descriptor,
            other => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` returned unexpected storage describe payload: {other:?}",
                    package.plugin_id
                )));
            }
        };
        if descriptor.storage_profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "storage plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id, descriptor.storage_profile, profile_id
            )));
        }
        let capabilities =
            match invoke_storage(&package.plugin_id, &api, StorageRequest::CapabilitySet)? {
                StorageResponse::CapabilitySet(capabilities) => capabilities,
                other => {
                    return Err(RuntimeError::Config(format!(
                        "plugin `{}` returned unexpected storage capability payload: {other:?}",
                        package.plugin_id
                    )));
                }
            };
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

    fn load_auth_generation(
        &self,
        package: &PluginPackage,
        generation_id: PluginGenerationId,
    ) -> Result<AuthGeneration, RuntimeError> {
        let (guard, manifest, api) = match &package.source {
            PluginSource::DynamicLibrary { library_path, .. } => unsafe {
                self.load_dynamic_auth(library_path)?
            },
            PluginSource::InProcessAuth(plugin) => {
                (None, decode_manifest(plugin.manifest)?, *plugin.api)
            }
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessStorage(_) => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` is not an auth plugin",
                    package.plugin_id
                )));
            }
        };
        self.validate_manifest(package, &manifest)?;
        let profile_id = manifest
            .capabilities
            .iter()
            .find_map(|capability| capability.strip_prefix("auth.profile:"))
            .map(ToString::to_string)
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "auth plugin `{}` is missing auth.profile:<id> manifest capability",
                    package.plugin_id
                ))
            })?;
        if !manifest
            .capabilities
            .iter()
            .any(|capability| capability == "runtime.reload.auth")
        {
            return Err(RuntimeError::Config(format!(
                "auth plugin `{}` is missing runtime.reload.auth capability",
                package.plugin_id
            )));
        }
        let descriptor = match invoke_auth(&package.plugin_id, &api, AuthRequest::Describe)? {
            AuthResponse::Descriptor(descriptor) => descriptor,
            other => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` returned unexpected auth describe payload: {other:?}",
                    package.plugin_id
                )));
            }
        };
        if descriptor.auth_profile != profile_id {
            return Err(RuntimeError::Config(format!(
                "auth plugin `{}` describe profile `{}` did not match manifest profile `{}`",
                package.plugin_id, descriptor.auth_profile, profile_id
            )));
        }
        let capabilities = match invoke_auth(&package.plugin_id, &api, AuthRequest::CapabilitySet)?
        {
            AuthResponse::CapabilitySet(capabilities) => capabilities,
            other => {
                return Err(RuntimeError::Config(format!(
                    "plugin `{}` returned unexpected auth capability payload: {other:?}",
                    package.plugin_id
                )));
            }
        };
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
        &self,
        library_path: &Path,
    ) -> Result<
        (
            Option<Arc<Mutex<Library>>>,
            DecodedManifest,
            ProtocolPluginApiV1,
        ),
        RuntimeError,
    > {
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
        &self,
        library_path: &Path,
    ) -> Result<
        (
            Option<Arc<Mutex<Library>>>,
            DecodedManifest,
            GameplayPluginApiV1,
        ),
        RuntimeError,
    > {
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

    unsafe fn load_dynamic_storage(
        &self,
        library_path: &Path,
    ) -> Result<
        (
            Option<Arc<Mutex<Library>>>,
            DecodedManifest,
            StoragePluginApiV1,
        ),
        RuntimeError,
    > {
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

    unsafe fn load_dynamic_auth(
        &self,
        library_path: &Path,
    ) -> Result<
        (
            Option<Arc<Mutex<Library>>>,
            DecodedManifest,
            AuthPluginApiV1,
        ),
        RuntimeError,
    > {
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

#[derive(Clone, Debug)]
struct DecodedManifest {
    plugin_id: String,
    plugin_kind: PluginKind,
    plugin_abi: PluginAbiVersion,
    min_host_abi: PluginAbiVersion,
    max_host_abi: PluginAbiVersion,
    capabilities: Vec<String>,
}

fn decode_manifest(manifest: *const PluginManifestV1) -> Result<DecodedManifest, RuntimeError> {
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

fn decode_utf8_slice(slice: mc_plugin_api::Utf8Slice) -> Result<String, RuntimeError> {
    if slice.ptr.is_null() {
        return Err(RuntimeError::Config(
            "plugin utf8 slice was null".to_string(),
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(slice.ptr, slice.len) };
    String::from_utf8(bytes.to_vec()).map_err(|error| RuntimeError::Config(error.to_string()))
}

fn invoke_protocol(
    api: &ProtocolPluginApiV1,
    request: ProtocolRequest,
) -> Result<ProtocolResponse, RuntimeError> {
    let request_bytes = encode_protocol_request(&request)
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
    decode_protocol_response(&request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

fn invoke_gameplay(
    plugin_id: &str,
    api: &GameplayPluginApiV1,
    request: GameplayRequest,
) -> Result<GameplayResponse, RuntimeError> {
    let request_bytes = encode_gameplay_request(&request)
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
        return Err(RuntimeError::Config(decode_plugin_error(
            plugin_id,
            status,
            api.free_buffer,
            error,
        )));
    }
    let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_gameplay_response(&request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

fn invoke_storage(
    plugin_id: &str,
    api: &StoragePluginApiV1,
    request: StorageRequest,
) -> Result<StorageResponse, RuntimeError> {
    let request_bytes = encode_storage_request(&request)
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
        return Err(RuntimeError::Config(decode_plugin_error(
            plugin_id,
            status,
            api.free_buffer,
            error,
        )));
    }

    let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_storage_response(&request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
}

fn invoke_auth(
    plugin_id: &str,
    api: &AuthPluginApiV1,
    request: AuthRequest,
) -> Result<AuthResponse, RuntimeError> {
    let request_bytes =
        encode_auth_request(&request).map_err(|error| RuntimeError::Config(error.to_string()))?;
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
        return Err(RuntimeError::Config(decode_plugin_error(
            plugin_id,
            status,
            api.free_buffer,
            error,
        )));
    }

    let response_bytes = unsafe { std::slice::from_raw_parts(output.ptr, output.len) }.to_vec();
    unsafe {
        (api.free_buffer)(output);
    }
    decode_auth_response(&request, &response_bytes)
        .map_err(|error| RuntimeError::Config(error.to_string()))
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

    pub fn activate_gameplay_profiles(&self, config: &ServerConfig) -> Result<(), RuntimeError> {
        let mut required_profiles = HashSet::new();
        required_profiles.insert(config.default_gameplay_profile.clone());
        required_profiles.extend(config.gameplay_profile_map.values().cloned());

        let mut gameplay = self
            .gameplay
            .lock()
            .expect("plugin host mutex should not be poisoned");
        gameplay.clear();

        for package in self.catalog.packages() {
            if package.plugin_kind != PluginKind::Gameplay {
                continue;
            }
            let generation = Arc::new(
                self.loader
                    .load_gameplay_generation(package, self.generations.next_generation_id())?,
            );
            if !required_profiles.contains(generation.profile_id.as_str()) {
                continue;
            }
            if gameplay.contains_key(generation.profile_id.as_str()) {
                return Err(RuntimeError::Config(format!(
                    "duplicate gameplay profile `{}` discovered",
                    generation.profile_id.as_str()
                )));
            }
            gameplay.insert(
                generation.profile_id.as_str().to_string(),
                ManagedGameplayPlugin {
                    package: package.clone(),
                    profile_id: generation.profile_id.clone(),
                    profile: Arc::new(HotSwappableGameplayProfile::new(
                        package.plugin_id.clone(),
                        generation.profile_id.clone(),
                        generation,
                        Arc::clone(&self.quarantine),
                    )),
                    loaded_at: package.modified_at()?,
                },
            );
        }

        for profile in required_profiles {
            if !gameplay.contains_key(&profile) {
                return Err(RuntimeError::Config(format!(
                    "unknown gameplay profile `{profile}`"
                )));
            }
        }

        Ok(())
    }

    pub fn activate_storage_profile(&self, storage_profile: &str) -> Result<(), RuntimeError> {
        let mut storage = self
            .storage
            .lock()
            .expect("plugin host mutex should not be poisoned");
        storage.clear();

        for package in self.catalog.packages() {
            if package.plugin_kind != PluginKind::Storage {
                continue;
            }
            let generation = Arc::new(
                self.loader
                    .load_storage_generation(package, self.generations.next_generation_id())?,
            );
            if generation.profile_id != storage_profile {
                continue;
            }
            if storage.contains_key(storage_profile) {
                return Err(RuntimeError::Config(format!(
                    "duplicate storage profile `{storage_profile}` discovered"
                )));
            }
            storage.insert(
                storage_profile.to_string(),
                ManagedStoragePlugin {
                    package: package.clone(),
                    profile_id: storage_profile.to_string(),
                    profile: Arc::new(HotSwappableStorageProfile::new(
                        package.plugin_id.clone(),
                        storage_profile.to_string(),
                        generation,
                        Arc::clone(&self.quarantine),
                    )),
                    loaded_at: package.modified_at()?,
                },
            );
        }

        if !storage.contains_key(storage_profile) {
            return Err(RuntimeError::Config(format!(
                "unknown storage profile `{storage_profile}`"
            )));
        }

        Ok(())
    }

    pub fn activate_auth_profiles(&self, auth_profiles: &[String]) -> Result<(), RuntimeError> {
        let mut auth = self
            .auth
            .lock()
            .expect("plugin host mutex should not be poisoned");
        auth.clear();
        let requested = auth_profiles
            .iter()
            .filter(|profile_id| !profile_id.is_empty())
            .cloned()
            .collect::<HashSet<_>>();
        if requested.is_empty() {
            return Err(RuntimeError::Config(
                "at least one auth profile must be activated".to_string(),
            ));
        }

        for package in self.catalog.packages() {
            if package.plugin_kind != PluginKind::Auth {
                continue;
            }
            let generation = Arc::new(
                self.loader
                    .load_auth_generation(package, self.generations.next_generation_id())?,
            );
            if !requested.contains(&generation.profile_id) {
                continue;
            }
            if auth.contains_key(&generation.profile_id) {
                return Err(RuntimeError::Config(format!(
                    "duplicate auth profile `{}` discovered",
                    generation.profile_id
                )));
            }
            let profile_id = generation.profile_id.clone();
            auth.insert(
                profile_id.clone(),
                ManagedAuthPlugin {
                    package: package.clone(),
                    profile_id: profile_id.clone(),
                    profile: Arc::new(HotSwappableAuthProfile::new(
                        package.plugin_id.clone(),
                        profile_id,
                        generation,
                        Arc::clone(&self.quarantine),
                    )),
                    loaded_at: package.modified_at()?,
                },
            );
        }

        for profile_id in &requested {
            if !auth.contains_key(profile_id) {
                return Err(RuntimeError::Config(format!(
                    "unknown auth profile `{profile_id}`"
                )));
            }
        }

        Ok(())
    }

    pub fn activate_auth_profile(&self, auth_profile: &str) -> Result<(), RuntimeError> {
        self.activate_auth_profiles(&[auth_profile.to_string()])
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
            managed.package.refresh_dynamic_manifest()?;
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

    pub(crate) async fn reload_modified_with_context(
        &self,
        runtime: &RuntimeReloadContext,
    ) -> Result<Vec<String>, RuntimeError> {
        let mut reloaded = self.reload_modified()?;
        {
            let mut gameplay = self
                .gameplay
                .lock()
                .expect("plugin host mutex should not be poisoned");
            for managed in gameplay.values_mut() {
                managed.package.refresh_dynamic_manifest()?;
                let modified_at = managed.package.modified_at()?;
                if modified_at <= managed.loaded_at {
                    continue;
                }
                let generation = match self.loader.load_gameplay_generation(
                    &managed.package,
                    self.generations.next_generation_id(),
                ) {
                    Ok(generation) => Arc::new(generation),
                    Err(error) => {
                        eprintln!(
                            "gameplay reload load failed for `{}`: {error}",
                            managed.package.plugin_id
                        );
                        managed.loaded_at = modified_at;
                        continue;
                    }
                };
                if generation.profile_id != managed.profile_id {
                    eprintln!(
                        "gameplay plugin `{}` changed profile from `{}` to `{}` during reload",
                        managed.package.plugin_id,
                        managed.profile_id.as_str(),
                        generation.profile_id.as_str()
                    );
                    managed.loaded_at = modified_at;
                    continue;
                }
                let _reload_guard = managed
                    .profile
                    .reload_gate
                    .write()
                    .expect("gameplay reload gate should not be poisoned");
                let current_generation = managed
                    .profile
                    .current_generation()
                    .map_err(RuntimeError::Config)?;
                let relevant_sessions = runtime
                    .gameplay_sessions
                    .iter()
                    .filter(|session| {
                        session.gameplay_profile.as_str() == managed.profile_id.as_str()
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let mut migration_failed = false;
                for session in &relevant_sessions {
                    let blob = match current_generation.invoke(
                        GameplayRequest::ExportSessionState {
                            session: session.clone(),
                        },
                    ) {
                        Ok(GameplayResponse::SessionTransferBlob(blob)) => blob,
                        Ok(other) => {
                            eprintln!(
                                "gameplay reload export returned unexpected payload for `{}`: {other:?}",
                                managed.package.plugin_id
                            );
                            migration_failed = true;
                            break;
                        }
                        Err(error) => {
                            eprintln!(
                                "gameplay reload export failed for `{}`: {error}",
                                managed.package.plugin_id
                            );
                            migration_failed = true;
                            break;
                        }
                    };
                    match generation.invoke(GameplayRequest::ImportSessionState {
                        session: session.clone(),
                        blob,
                    }) {
                        Ok(GameplayResponse::Empty) => {}
                        Ok(other) => {
                            eprintln!(
                                "gameplay reload import returned unexpected payload for `{}`: {other:?}",
                                managed.package.plugin_id
                            );
                            migration_failed = true;
                            break;
                        }
                        Err(error) => {
                            eprintln!(
                                "gameplay reload import failed for `{}`: {error}",
                                managed.package.plugin_id
                            );
                            migration_failed = true;
                            break;
                        }
                    }
                }
                managed.loaded_at = modified_at;
                if migration_failed {
                    continue;
                }
                managed.profile.swap_generation(generation);
                reloaded.push(managed.package.plugin_id.clone());
            }
        }

        let mut storage = self
            .storage
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for managed in storage.values_mut() {
            managed.package.refresh_dynamic_manifest()?;
            let modified_at = managed.package.modified_at()?;
            if modified_at <= managed.loaded_at {
                continue;
            }
            let generation = match self
                .loader
                .load_storage_generation(&managed.package, self.generations.next_generation_id())
            {
                Ok(generation) => Arc::new(generation),
                Err(error) => {
                    eprintln!(
                        "storage reload load failed for `{}`: {error}",
                        managed.package.plugin_id
                    );
                    managed.loaded_at = modified_at;
                    continue;
                }
            };
            if generation.profile_id != managed.profile_id {
                eprintln!(
                    "storage plugin `{}` changed profile from `{}` to `{}` during reload",
                    managed.package.plugin_id, managed.profile_id, generation.profile_id
                );
                managed.loaded_at = modified_at;
                continue;
            }
            let _reload_guard = managed
                .profile
                .reload_gate
                .write()
                .expect("storage reload gate should not be poisoned");
            match generation.invoke(StorageRequest::ImportRuntimeState {
                world_dir: runtime.world_dir.display().to_string(),
                snapshot: runtime.snapshot.clone(),
            }) {
                Ok(StorageResponse::Empty) => {
                    managed.profile.swap_generation(generation);
                    managed.loaded_at = modified_at;
                    reloaded.push(managed.package.plugin_id.clone());
                }
                Ok(other) => {
                    eprintln!(
                        "storage reload import returned unexpected payload for `{}`: {other:?}",
                        managed.package.plugin_id
                    );
                    managed.loaded_at = modified_at;
                }
                Err(error) => {
                    eprintln!(
                        "storage reload import failed for `{}`: {error}",
                        managed.package.plugin_id
                    );
                    managed.loaded_at = modified_at;
                }
            }
        }
        drop(storage);

        let mut auth = self
            .auth
            .lock()
            .expect("plugin host mutex should not be poisoned");
        for managed in auth.values_mut() {
            managed.package.refresh_dynamic_manifest()?;
            let modified_at = managed.package.modified_at()?;
            if modified_at <= managed.loaded_at {
                continue;
            }
            let generation = match self
                .loader
                .load_auth_generation(&managed.package, self.generations.next_generation_id())
            {
                Ok(generation) => Arc::new(generation),
                Err(error) => {
                    eprintln!(
                        "auth reload load failed for `{}`: {error}",
                        managed.package.plugin_id
                    );
                    managed.loaded_at = modified_at;
                    continue;
                }
            };
            if generation.profile_id != managed.profile_id {
                eprintln!(
                    "auth plugin `{}` changed profile from `{}` to `{}` during reload",
                    managed.package.plugin_id, managed.profile_id, generation.profile_id
                );
                managed.loaded_at = modified_at;
                continue;
            }
            managed.profile.swap_generation(generation);
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
mod tests {
    use super::{
        InProcessAuthPlugin, InProcessGameplayPlugin, InProcessProtocolPlugin,
        InProcessStoragePlugin, PluginAbiRange, PluginCatalog, PluginFailurePolicy, PluginHost,
    };
    use crate::config::ServerConfig;
    use crate::host::plugin_host_from_config;
    use crate::registry::RuntimeRegistries;
    use mc_plugin_api::{
        CURRENT_PLUGIN_ABI, PluginAbiVersion, PluginKind, PluginManifestV1, Utf8Slice,
    };
    use mc_plugin_auth_offline::in_process_auth_entrypoints as offline_auth_entrypoints;
    use mc_plugin_gameplay_canonical::in_process_gameplay_entrypoints as canonical_gameplay_entrypoints;
    use mc_plugin_gameplay_readonly::in_process_gameplay_entrypoints as readonly_gameplay_entrypoints;
    use mc_plugin_proto_be_26_3::in_process_protocol_entrypoints as be_26_3_entrypoints;
    use mc_plugin_proto_be_placeholder::in_process_protocol_entrypoints as be_placeholder_entrypoints;
    use mc_plugin_proto_je_1_7_10::in_process_protocol_entrypoints;
    use mc_plugin_proto_je_1_8_x::in_process_protocol_entrypoints as je_1_8_x_entrypoints;
    use mc_plugin_proto_je_1_12_2::in_process_protocol_entrypoints as je_1_12_2_entrypoints;
    use mc_plugin_storage_je_anvil_1_7_10::in_process_storage_entrypoints as storage_entrypoints;
    use mc_proto_common::{Edition, PacketWriter, TransportKind, WireFormatKind};
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::tempdir;

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
    fn all_protocol_plugins_register_and_resolve() {
        let mut catalog = PluginCatalog::default();
        for (plugin_id, entrypoints) in [
            ("je-1_7_10", in_process_protocol_entrypoints()),
            ("je-1_8_x", je_1_8_x_entrypoints()),
            ("je-1_12_2", je_1_12_2_entrypoints()),
            ("be-placeholder", be_placeholder_entrypoints()),
        ] {
            catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
                plugin_id: plugin_id.to_string(),
                manifest: entrypoints.manifest,
                api: entrypoints.api,
            });
        }

        let host = Arc::new(PluginHost::new(
            catalog,
            PluginAbiRange::default(),
            PluginFailurePolicy::Quarantine,
        ));
        let mut registries = RuntimeRegistries::new();
        host.load_into_registries(&mut registries)
            .expect("protocol plugins should load");

        for adapter_id in ["je-1_7_10", "je-1_8_x", "je-1_12_2", "be-placeholder"] {
            assert!(
                registries.protocols().resolve_adapter(adapter_id).is_some(),
                "adapter `{adapter_id}` should resolve"
            );
        }

        let je_handshake = je_handshake_frame(340);
        let je_intent = registries
            .protocols()
            .route_handshake(TransportKind::Tcp, &je_handshake)
            .expect("tcp probe should not fail")
            .expect("tcp handshake should resolve");
        assert_eq!(je_intent.edition, Edition::Je);
        assert_eq!(je_intent.protocol_number, 340);

        let be_intent = registries
            .protocols()
            .route_handshake(TransportKind::Udp, &raknet_unconnected_ping())
            .expect("udp probe should not fail")
            .expect("udp datagram should resolve");
        assert_eq!(be_intent.edition, Edition::Be);
    }

    #[test]
    fn protocol_plugins_preserve_wire_format_and_optional_bedrock_listener_metadata() {
        let mut catalog = PluginCatalog::default();
        for (plugin_id, entrypoints) in [
            ("je-1_7_10", in_process_protocol_entrypoints()),
            ("be-26_3", be_26_3_entrypoints()),
            ("be-placeholder", be_placeholder_entrypoints()),
        ] {
            catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
                plugin_id: plugin_id.to_string(),
                manifest: entrypoints.manifest,
                api: entrypoints.api,
            });
        }

        let host = Arc::new(PluginHost::new(
            catalog,
            PluginAbiRange::default(),
            PluginFailurePolicy::Quarantine,
        ));
        let mut registries = RuntimeRegistries::new();
        host.load_into_registries(&mut registries)
            .expect("protocol plugins should load");

        let je_adapter = registries
            .protocols()
            .resolve_adapter("je-1_7_10")
            .expect("je adapter should resolve");
        assert_eq!(
            je_adapter.descriptor().wire_format,
            WireFormatKind::MinecraftFramed
        );
        assert!(je_adapter.bedrock_listener_descriptor().is_none());

        let bedrock_adapter = registries
            .protocols()
            .resolve_adapter("be-26_3")
            .expect("bedrock adapter should resolve");
        assert_eq!(
            bedrock_adapter.descriptor().wire_format,
            WireFormatKind::RawPacketStream
        );
        assert!(bedrock_adapter.bedrock_listener_descriptor().is_some());

        let placeholder_adapter = registries
            .protocols()
            .resolve_adapter("be-placeholder")
            .expect("placeholder adapter should resolve");
        assert_eq!(
            placeholder_adapter.descriptor().wire_format,
            WireFormatKind::RawPacketStream
        );
        assert!(placeholder_adapter.bedrock_listener_descriptor().is_none());
    }

    #[test]
    fn abi_mismatch_is_rejected_before_registration() {
        let entrypoints = in_process_protocol_entrypoints();
        let mut catalog = PluginCatalog::default();
        catalog.register_in_process_protocol_plugin(InProcessProtocolPlugin {
            plugin_id: "je-1_7_10".to_string(),
            manifest: manifest_with_abi("je-1_7_10", PluginAbiVersion { major: 9, minor: 0 }),
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
    fn storage_and_auth_plugins_are_managed_without_quarantine() {
        let mut catalog = PluginCatalog::default();
        let storage = storage_entrypoints();
        catalog.register_in_process_storage_plugin(InProcessStoragePlugin {
            plugin_id: "storage-je-anvil-1_7_10".to_string(),
            manifest: storage.manifest,
            api: storage.api,
        });
        let auth = offline_auth_entrypoints();
        catalog.register_in_process_auth_plugin(InProcessAuthPlugin {
            plugin_id: "auth-offline".to_string(),
            manifest: auth.manifest,
            api: auth.api,
        });
        let host = Arc::new(PluginHost::new(
            catalog,
            PluginAbiRange::default(),
            PluginFailurePolicy::Quarantine,
        ));
        let mut registries = RuntimeRegistries::new();
        host.load_into_registries(&mut registries)
            .expect("storage/auth plugin kinds should register with the host");

        assert!(host.quarantine_reason("storage-je-anvil-1_7_10").is_none());
        assert!(host.quarantine_reason("auth-offline").is_none());
    }

    #[test]
    fn gameplay_profiles_activate_and_resolve() {
        let mut catalog = PluginCatalog::default();
        let canonical = canonical_gameplay_entrypoints();
        catalog.register_in_process_gameplay_plugin(InProcessGameplayPlugin {
            plugin_id: "gameplay-canonical".to_string(),
            manifest: canonical.manifest,
            api: canonical.api,
        });
        let readonly = readonly_gameplay_entrypoints();
        catalog.register_in_process_gameplay_plugin(InProcessGameplayPlugin {
            plugin_id: "gameplay-readonly".to_string(),
            manifest: readonly.manifest,
            api: readonly.api,
        });

        let host = Arc::new(PluginHost::new(
            catalog,
            PluginAbiRange::default(),
            PluginFailurePolicy::Quarantine,
        ));
        host.activate_gameplay_profiles(&ServerConfig {
            default_gameplay_profile: "canonical".to_string(),
            gameplay_profile_map: [("je-1_7_10".to_string(), "readonly".to_string())]
                .into_iter()
                .collect(),
            ..ServerConfig::default()
        })
        .expect("known gameplay profiles should activate");

        assert!(host.resolve_gameplay_profile("canonical").is_some());
        assert!(host.resolve_gameplay_profile("readonly").is_some());
    }

    #[test]
    fn unknown_gameplay_profile_fails_activation() {
        let mut catalog = PluginCatalog::default();
        let canonical = canonical_gameplay_entrypoints();
        catalog.register_in_process_gameplay_plugin(InProcessGameplayPlugin {
            plugin_id: "gameplay-canonical".to_string(),
            manifest: canonical.manifest,
            api: canonical.api,
        });

        let host = Arc::new(PluginHost::new(
            catalog,
            PluginAbiRange::default(),
            PluginFailurePolicy::Quarantine,
        ));
        let error = host
            .activate_gameplay_profiles(&ServerConfig {
                default_gameplay_profile: "readonly".to_string(),
                ..ServerConfig::default()
            })
            .expect_err("unknown gameplay profile should fail fast");
        assert!(matches!(
            error,
            crate::RuntimeError::Config(message) if message.contains("unknown gameplay profile")
        ));
    }

    #[test]
    fn storage_and_auth_profiles_activate_and_resolve() {
        let mut catalog = PluginCatalog::default();
        let storage = storage_entrypoints();
        catalog.register_in_process_storage_plugin(InProcessStoragePlugin {
            plugin_id: "storage-je-anvil-1_7_10".to_string(),
            manifest: storage.manifest,
            api: storage.api,
        });
        let auth = offline_auth_entrypoints();
        catalog.register_in_process_auth_plugin(InProcessAuthPlugin {
            plugin_id: "auth-offline".to_string(),
            manifest: auth.manifest,
            api: auth.api,
        });

        let host = Arc::new(PluginHost::new(
            catalog,
            PluginAbiRange::default(),
            PluginFailurePolicy::Quarantine,
        ));
        host.activate_storage_profile("je-anvil-1_7_10")
            .expect("known storage profile should activate");
        host.activate_auth_profile("offline-v1")
            .expect("known auth profile should activate");

        assert!(host.resolve_storage_profile("je-anvil-1_7_10").is_some());
        assert!(host.resolve_auth_profile("offline-v1").is_some());
    }

    #[test]
    fn unknown_storage_and_auth_profiles_fail_activation() {
        let host = Arc::new(PluginHost::new(
            PluginCatalog::default(),
            PluginAbiRange::default(),
            PluginFailurePolicy::Quarantine,
        ));
        let storage = host
            .activate_storage_profile("missing")
            .expect_err("unknown storage profile should fail fast");
        assert!(matches!(
            storage,
            crate::RuntimeError::Config(message) if message.contains("unknown storage profile")
        ));

        let auth = host
            .activate_auth_profile("missing")
            .expect_err("unknown auth profile should fail fast");
        assert!(matches!(
            auth,
            crate::RuntimeError::Config(message) if message.contains("unknown auth profile")
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn packaged_protocol_plugins_load_via_dlopen() -> Result<(), crate::RuntimeError> {
        let temp_dir = tempdir()?;
        let dist_dir = temp_dir.path().join("runtime").join("plugins");
        crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;

        let config = ServerConfig {
            plugins_dir: dist_dir,
            ..ServerConfig::default()
        };
        let host =
            plugin_host_from_config(&config)?.expect("packaged plugins should be discovered");
        let mut registries = RuntimeRegistries::new();
        host.load_into_registries(&mut registries)?;

        for adapter_id in ["je-1_7_10", "je-1_8_x", "je-1_12_2", "be-placeholder"] {
            let adapter = registries
                .protocols()
                .resolve_adapter(adapter_id)
                .expect("packaged plugin adapter should resolve");
            assert!(
                adapter.capability_set().contains(&format!(
                    "build-tag:{}",
                    crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
                )),
                "adapter `{adapter_id}` should expose build tag capability"
            );
        }

        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn packaged_protocol_reload_replaces_generation() -> Result<(), crate::RuntimeError> {
        let temp_dir = tempdir()?;
        let dist_dir = temp_dir.path().join("runtime").join("plugins");
        let target_dir = crate::packaged_plugin_test_target_dir("plugin-host-reload");
        crate::seed_packaged_plugins_from_test_harness(&dist_dir)?;

        let config = ServerConfig {
            plugins_dir: dist_dir.clone(),
            ..ServerConfig::default()
        };
        let host =
            plugin_host_from_config(&config)?.expect("packaged plugins should be discovered");
        let mut registries = RuntimeRegistries::new();
        host.load_into_registries(&mut registries)?;

        let adapter = registries
            .protocols()
            .resolve_adapter("je-1_7_10")
            .expect("packaged je-1_7_10 adapter should resolve");
        let first_generation = adapter
            .plugin_generation_id()
            .expect("packaged adapter should report plugin generation");
        assert!(adapter.capability_set().contains(&format!(
            "build-tag:{}",
            crate::PACKAGED_PLUGIN_TEST_HARNESS_TAG
        )));

        std::thread::sleep(Duration::from_secs(1));
        package_single_protocol_plugin(
            "mc-plugin-proto-je-1_7_10",
            "je-1_7_10",
            &dist_dir,
            &target_dir,
            "reload-v2",
        )?;

        let reloaded = host.reload_modified()?;
        assert_eq!(reloaded, vec!["je-1_7_10".to_string()]);

        let adapter = registries
            .protocols()
            .resolve_adapter("je-1_7_10")
            .expect("reloaded adapter should resolve");
        let next_generation = adapter
            .plugin_generation_id()
            .expect("reloaded adapter should report plugin generation");
        assert_ne!(first_generation, next_generation);
        assert!(adapter.capability_set().contains("build-tag:reload-v2"));
        Ok(())
    }

    fn je_handshake_frame(protocol_version: i32) -> Vec<u8> {
        let mut writer = PacketWriter::default();
        writer.write_varint(0);
        writer.write_varint(protocol_version);
        writer
            .write_string("localhost")
            .expect("handshake host should encode");
        writer.write_u16(25565);
        writer.write_varint(2);
        writer.into_inner()
    }

    fn raknet_unconnected_ping() -> Vec<u8> {
        let mut frame = Vec::with_capacity(33);
        frame.push(0x01);
        frame.extend_from_slice(&123_i64.to_be_bytes());
        frame.extend_from_slice(&[
            0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34,
            0x56, 0x78,
        ]);
        frame.extend_from_slice(&456_i64.to_be_bytes());
        frame
    }

    #[cfg(target_os = "linux")]
    fn package_single_protocol_plugin(
        cargo_package: &str,
        plugin_id: &str,
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<(), crate::RuntimeError> {
        let _guard = crate::packaged_plugin_test_build_lock()
            .lock()
            .expect("packaged plugin build lock should not be poisoned");
        let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
        let status = Command::new(cargo)
            .current_dir(workspace_root())
            .env("CARGO_TARGET_DIR", target_dir)
            .env("REVY_PLUGIN_BUILD_TAG", build_tag)
            .arg("build")
            .arg("-p")
            .arg(cargo_package)
            .status()
            .map_err(|error| crate::RuntimeError::Config(error.to_string()))?;
        if !status.success() {
            return Err(crate::RuntimeError::Config(format!(
                "cargo build failed for `{cargo_package}`"
            )));
        }

        let artifact_name = dynamic_library_filename(cargo_package);
        let source = target_dir.join("debug").join(&artifact_name);
        let plugin_dir = dist_dir.join(plugin_id);
        fs::create_dir_all(&plugin_dir)?;
        let packaged_artifact = packaged_artifact_name(&artifact_name, build_tag);
        let destination = plugin_dir.join(&packaged_artifact);
        let staging = plugin_dir.join(format!(".{packaged_artifact}.tmp"));
        fs::copy(&source, &staging)?;
        if destination.exists() {
            fs::remove_file(&destination)?;
        }
        fs::rename(&staging, &destination)?;
        let manifest = format!(
            "[plugin]\nid = \"{plugin_id}\"\nkind = \"protocol\"\n\n[artifacts]\n\"{}-{}\" = \"{packaged_artifact}\"\n",
            env::consts::OS,
            env::consts::ARCH
        );
        fs::write(plugin_dir.join("plugin.toml"), manifest)?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn dynamic_library_filename(package: &str) -> String {
        let crate_name = package.replace('-', "_");
        match env::consts::OS {
            "windows" => format!("{crate_name}.dll"),
            "macos" => format!("lib{crate_name}.dylib"),
            _ => format!("lib{crate_name}.so"),
        }
    }

    #[cfg(target_os = "linux")]
    fn packaged_artifact_name(base_name: &str, build_tag: &str) -> String {
        if let Some((stem, extension)) = base_name.rsplit_once('.') {
            format!("{stem}-{build_tag}.{extension}")
        } else {
            format!("{base_name}-{build_tag}")
        }
    }

    #[cfg(target_os = "linux")]
    fn workspace_root() -> PathBuf {
        crate::packaged_plugin_test_workspace_root()
    }
}
