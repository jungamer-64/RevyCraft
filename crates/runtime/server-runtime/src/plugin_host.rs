use crate::RuntimeError;
use crate::config::ServerConfig;
use crate::registry::{ProtocolRegistry, RuntimeRegistries};
use crate::runtime::RuntimeReloadContext;
use bytes::BytesMut;
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
    ProtocolRequest, ProtocolResponse, ProtocolSessionSnapshot, StoragePluginApiV1, StorageRequest,
    StorageResponse, WireFrameDecodeResult, decode_auth_response, decode_gameplay_response,
    decode_protocol_response, decode_storage_response, encode_auth_request,
    encode_gameplay_request, encode_protocol_request, encode_storage_request,
};
use mc_proto_common::{
    BedrockListenerDescriptor, ConnectionPhase, Edition, HandshakeIntent, HandshakeProbe,
    LoginRequest, PlayEncodingContext, ProtocolAdapter, ProtocolDescriptor, ProtocolError,
    ServerListStatus, StatusRequest, StorageAdapter, StorageError, TransportKind, WireCodec,
    WireFormatKind,
};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

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
    migrate_protocol_sessions, protocol_reload_compatible, require_manifest_capability,
};

const PLUGIN_RELOAD_POLL_INTERVAL_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginFailureAction {
    Quarantine,
    Skip,
    FailFast,
}

impl PluginFailureAction {
    fn parse_with_allowed(value: &str, key: &str, allowed: &[Self]) -> Result<Self, RuntimeError> {
        let action = if value.eq_ignore_ascii_case("quarantine") {
            Self::Quarantine
        } else if value.eq_ignore_ascii_case("skip") {
            Self::Skip
        } else if value.eq_ignore_ascii_case("fail-fast") {
            Self::FailFast
        } else {
            return Err(RuntimeError::Config(format!("unsupported {key} `{value}`")));
        };
        if allowed.contains(&action) {
            Ok(action)
        } else {
            Err(RuntimeError::Config(format!("unsupported {key} `{value}`")))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginFailureMatrix {
    pub protocol: PluginFailureAction,
    pub gameplay: PluginFailureAction,
    pub storage: PluginFailureAction,
    pub auth: PluginFailureAction,
}

impl Default for PluginFailureMatrix {
    fn default() -> Self {
        Self {
            protocol: PluginFailureAction::Quarantine,
            gameplay: PluginFailureAction::Quarantine,
            storage: PluginFailureAction::FailFast,
            auth: PluginFailureAction::Skip,
        }
    }
}

impl PluginFailureMatrix {
    pub fn parse_protocol(value: &str) -> Result<PluginFailureAction, RuntimeError> {
        PluginFailureAction::parse_with_allowed(
            value,
            "plugin-failure-policy-protocol",
            &[
                PluginFailureAction::Quarantine,
                PluginFailureAction::Skip,
                PluginFailureAction::FailFast,
            ],
        )
    }

    pub fn parse_gameplay(value: &str) -> Result<PluginFailureAction, RuntimeError> {
        PluginFailureAction::parse_with_allowed(
            value,
            "plugin-failure-policy-gameplay",
            &[
                PluginFailureAction::Quarantine,
                PluginFailureAction::Skip,
                PluginFailureAction::FailFast,
            ],
        )
    }

    pub fn parse_storage(value: &str) -> Result<PluginFailureAction, RuntimeError> {
        PluginFailureAction::parse_with_allowed(
            value,
            "plugin-failure-policy-storage",
            &[PluginFailureAction::Skip, PluginFailureAction::FailFast],
        )
    }

    pub fn parse_auth(value: &str) -> Result<PluginFailureAction, RuntimeError> {
        PluginFailureAction::parse_with_allowed(
            value,
            "plugin-failure-policy-auth",
            &[PluginFailureAction::Skip, PluginFailureAction::FailFast],
        )
    }

    const fn action_for_kind(self, kind: PluginKind) -> PluginFailureAction {
        match kind {
            PluginKind::Protocol => self.protocol,
            PluginKind::Gameplay => self.gameplay,
            PluginKind::Storage => self.storage,
            PluginKind::Auth => self.auth,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginArtifactStatusSnapshot {
    pub source: String,
    pub modified_at_ms: u64,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolPluginStatusSnapshot {
    pub plugin_id: String,
    pub adapter_id: String,
    pub generation_id: PluginGenerationId,
    pub loaded_at_ms: u64,
    pub failure_action: PluginFailureAction,
    pub current_artifact: PluginArtifactStatusSnapshot,
    pub active_quarantine_reason: Option<String>,
    pub artifact_quarantine: Option<PluginArtifactStatusSnapshot>,
    pub version_name: String,
    pub transport: TransportKind,
    pub edition: Edition,
    pub protocol_number: i32,
    pub bedrock_listener_descriptor_present: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameplayPluginStatusSnapshot {
    pub plugin_id: String,
    pub profile_id: GameplayProfileId,
    pub generation_id: PluginGenerationId,
    pub loaded_at_ms: u64,
    pub failure_action: PluginFailureAction,
    pub current_artifact: PluginArtifactStatusSnapshot,
    pub active_quarantine_reason: Option<String>,
    pub artifact_quarantine: Option<PluginArtifactStatusSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoragePluginStatusSnapshot {
    pub plugin_id: String,
    pub profile_id: String,
    pub generation_id: PluginGenerationId,
    pub loaded_at_ms: u64,
    pub failure_action: PluginFailureAction,
    pub current_artifact: PluginArtifactStatusSnapshot,
    pub active_quarantine_reason: Option<String>,
    pub artifact_quarantine: Option<PluginArtifactStatusSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthPluginStatusSnapshot {
    pub plugin_id: String,
    pub profile_id: String,
    pub generation_id: PluginGenerationId,
    pub loaded_at_ms: u64,
    pub failure_action: PluginFailureAction,
    pub current_artifact: PluginArtifactStatusSnapshot,
    pub active_quarantine_reason: Option<String>,
    pub artifact_quarantine: Option<PluginArtifactStatusSnapshot>,
    pub mode: AuthMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHostStatusSnapshot {
    pub failure_matrix: PluginFailureMatrix,
    pub pending_fatal_error: Option<String>,
    pub protocols: Vec<ProtocolPluginStatusSnapshot>,
    pub gameplay: Vec<GameplayPluginStatusSnapshot>,
    pub storage: Vec<StoragePluginStatusSnapshot>,
    pub auth: Vec<AuthPluginStatusSnapshot>,
}

impl PluginHostStatusSnapshot {
    #[must_use]
    pub fn active_quarantine_count(&self) -> usize {
        self.protocols
            .iter()
            .filter(|plugin| plugin.active_quarantine_reason.is_some())
            .count()
            + self
                .gameplay
                .iter()
                .filter(|plugin| plugin.active_quarantine_reason.is_some())
                .count()
            + self
                .storage
                .iter()
                .filter(|plugin| plugin.active_quarantine_reason.is_some())
                .count()
            + self
                .auth
                .iter()
                .filter(|plugin| plugin.active_quarantine_reason.is_some())
                .count()
    }

    #[must_use]
    pub fn artifact_quarantine_count(&self) -> usize {
        self.protocols
            .iter()
            .filter(|plugin| plugin.artifact_quarantine.is_some())
            .count()
            + self
                .gameplay
                .iter()
                .filter(|plugin| plugin.artifact_quarantine.is_some())
                .count()
            + self
                .storage
                .iter()
                .filter(|plugin| plugin.artifact_quarantine.is_some())
                .count()
            + self
                .auth
                .iter()
                .filter(|plugin| plugin.artifact_quarantine.is_some())
                .count()
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

#[derive(Clone, Debug)]
struct DynamicCatalogSource {
    root: PathBuf,
    allowlist: Option<HashSet<String>>,
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

    fn artifact_identity(&self, modified_at: SystemTime) -> ArtifactIdentity {
        let source = match &self.source {
            PluginSource::DynamicLibrary {
                manifest_path,
                library_path,
            } => format!("{}|{}", manifest_path.display(), library_path.display()),
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAuth(_) => "in-process".to_string(),
        };
        ArtifactIdentity {
            source,
            modified_at,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PluginCatalog {
    packages: HashMap<String, PluginPackage>,
}

impl PluginCatalog {
    /// Discovers dynamic plugin packages from the configured plugin root.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin root cannot be read or when any discovered manifest is
    /// invalid.
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
                let plugin_id = package.plugin_id.clone();
                match packages.entry(plugin_id.clone()) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(package);
                    }
                    std::collections::hash_map::Entry::Occupied(_) => {
                        return Err(RuntimeError::Config(format!(
                            "duplicate plugin id `{plugin_id}` discovered in {}",
                            root.display()
                        )));
                    }
                }
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

fn system_time_ms(time: SystemTime) -> u64 {
    u64::try_from(
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or_default()
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
pub struct ActiveQuarantineManager {
    reasons: Mutex<HashMap<String, String>>,
}

impl ActiveQuarantineManager {
    fn quarantine(&self, plugin_id: &str, reason: impl Into<String>) -> bool {
        let reason = reason.into();
        let mut reasons = self
            .reasons
            .lock()
            .expect("quarantine mutex should not be poisoned");
        let changed = reasons.get(plugin_id) != Some(&reason);
        reasons.insert(plugin_id.to_string(), reason);
        changed
    }

    fn is_quarantined(&self, plugin_id: &str) -> bool {
        self.reasons
            .lock()
            .expect("quarantine mutex should not be poisoned")
            .contains_key(plugin_id)
    }

    fn clear(&self, plugin_id: &str) {
        self.reasons
            .lock()
            .expect("quarantine mutex should not be poisoned")
            .remove(plugin_id);
    }

    pub fn reason(&self, plugin_id: &str) -> Option<String> {
        self.reasons
            .lock()
            .expect("quarantine mutex should not be poisoned")
            .get(plugin_id)
            .cloned()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArtifactIdentity {
    source: String,
    modified_at: SystemTime,
}

#[derive(Clone, Debug)]
struct ArtifactQuarantineRecord {
    identity: ArtifactIdentity,
    reason: String,
}

#[derive(Default)]
struct ArtifactQuarantineManager {
    records: Mutex<HashMap<String, ArtifactQuarantineRecord>>,
}

impl ArtifactQuarantineManager {
    fn quarantine(
        &self,
        plugin_id: &str,
        identity: ArtifactIdentity,
        reason: impl Into<String>,
    ) -> bool {
        let reason = reason.into();
        let mut records = self
            .records
            .lock()
            .expect("artifact quarantine mutex should not be poisoned");
        let changed = records
            .get(plugin_id)
            .is_none_or(|record| record.identity != identity || record.reason != reason);
        records.insert(
            plugin_id.to_string(),
            ArtifactQuarantineRecord { identity, reason },
        );
        changed
    }

    fn is_quarantined(&self, plugin_id: &str, identity: &ArtifactIdentity) -> bool {
        self.records
            .lock()
            .expect("artifact quarantine mutex should not be poisoned")
            .get(plugin_id)
            .is_some_and(|record| &record.identity == identity)
    }

    fn clear(&self, plugin_id: &str) {
        self.records
            .lock()
            .expect("artifact quarantine mutex should not be poisoned")
            .remove(plugin_id);
    }

    fn reason(&self, plugin_id: &str, identity: &ArtifactIdentity) -> Option<String> {
        self.records
            .lock()
            .expect("artifact quarantine mutex should not be poisoned")
            .get(plugin_id)
            .filter(|record| &record.identity == identity)
            .map(|record| record.reason.clone())
    }

    fn record(&self, plugin_id: &str) -> Option<ArtifactQuarantineRecord> {
        self.records
            .lock()
            .expect("artifact quarantine mutex should not be poisoned")
            .get(plugin_id)
            .cloned()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PluginFailureStage {
    Boot,
    Reload,
    Runtime,
}

impl PluginFailureStage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Reload => "reload",
            Self::Runtime => "runtime",
        }
    }
}

struct PluginFailureDispatch {
    matrix: PluginFailureMatrix,
    active_quarantine: ActiveQuarantineManager,
    artifact_quarantine: ArtifactQuarantineManager,
    pending_fatal: Mutex<Option<String>>,
}

impl PluginFailureDispatch {
    fn new(matrix: PluginFailureMatrix) -> Self {
        Self {
            matrix,
            active_quarantine: ActiveQuarantineManager {
                reasons: Mutex::new(HashMap::new()),
            },
            artifact_quarantine: ArtifactQuarantineManager {
                records: Mutex::new(HashMap::new()),
            },
            pending_fatal: Mutex::new(None),
        }
    }

    const fn action_for_kind(&self, kind: PluginKind) -> PluginFailureAction {
        self.matrix.action_for_kind(kind)
    }

    fn active_reason(&self, plugin_id: &str) -> Option<String> {
        self.active_quarantine.reason(plugin_id)
    }

    fn pending_fatal_message(&self) -> Option<String> {
        self.pending_fatal
            .lock()
            .expect("pending fatal mutex should not be poisoned")
            .clone()
    }

    fn is_active_quarantined(&self, plugin_id: &str) -> bool {
        self.active_quarantine.is_quarantined(plugin_id)
    }

    fn is_artifact_quarantined(&self, plugin_id: &str, identity: &ArtifactIdentity) -> bool {
        self.artifact_quarantine.is_quarantined(plugin_id, identity)
    }

    fn artifact_reason(&self, plugin_id: &str, identity: &ArtifactIdentity) -> Option<String> {
        self.artifact_quarantine.reason(plugin_id, identity)
    }

    fn artifact_record(&self, plugin_id: &str) -> Option<ArtifactQuarantineRecord> {
        self.artifact_quarantine.record(plugin_id)
    }

    fn clear_plugin_state(&self, plugin_id: &str) {
        self.active_quarantine.clear(plugin_id);
        self.artifact_quarantine.clear(plugin_id);
    }

    fn record_fatal_message(&self, message: String) {
        let mut pending = self
            .pending_fatal
            .lock()
            .expect("pending fatal mutex should not be poisoned");
        if pending.is_none() {
            eprintln!("plugin fail-fast scheduled graceful shutdown: {message}");
            *pending = Some(message);
        }
    }

    const fn kind_label(kind: PluginKind) -> &'static str {
        match kind {
            PluginKind::Protocol => "protocol",
            PluginKind::Gameplay => "gameplay",
            PluginKind::Storage => "storage",
            PluginKind::Auth => "auth",
        }
    }

    fn take_pending_fatal_error(&self) -> Option<RuntimeError> {
        self.pending_fatal
            .lock()
            .expect("pending fatal mutex should not be poisoned")
            .take()
            .map(RuntimeError::PluginFatal)
    }

    fn fail_fast_message(
        kind: PluginKind,
        stage: PluginFailureStage,
        plugin_id: &str,
        reason: &str,
    ) -> String {
        format!(
            "{} plugin `{plugin_id}` failed during {}: {reason}",
            match kind {
                PluginKind::Protocol => "protocol",
                PluginKind::Gameplay => "gameplay",
                PluginKind::Storage => "storage",
                PluginKind::Auth => "auth",
            },
            stage.as_str(),
        )
    }

    fn handle_runtime_failure(
        &self,
        kind: PluginKind,
        plugin_id: &str,
        reason: &str,
    ) -> PluginFailureAction {
        let action = self.action_for_kind(kind);
        match action {
            PluginFailureAction::Quarantine => {
                if self
                    .active_quarantine
                    .quarantine(plugin_id, reason.to_string())
                {
                    eprintln!(
                        "{} plugin `{plugin_id}` entered active quarantine: {reason}",
                        Self::kind_label(kind)
                    );
                }
            }
            PluginFailureAction::FailFast => {
                self.record_fatal_message(Self::fail_fast_message(
                    kind,
                    PluginFailureStage::Runtime,
                    plugin_id,
                    reason,
                ));
            }
            PluginFailureAction::Skip => {}
        }
        action
    }

    fn handle_candidate_failure(
        &self,
        kind: PluginKind,
        stage: PluginFailureStage,
        plugin_id: &str,
        identity: ArtifactIdentity,
        reason: &str,
    ) -> Result<(), RuntimeError> {
        match self.action_for_kind(kind) {
            PluginFailureAction::Skip => Ok(()),
            PluginFailureAction::Quarantine => {
                let modified_at_ms = system_time_ms(identity.modified_at);
                let source = identity.source.clone();
                if self
                    .artifact_quarantine
                    .quarantine(plugin_id, identity, reason.to_string())
                {
                    eprintln!(
                        "{} plugin `{plugin_id}` artifact quarantined during {}: source={} modified_at_ms={} reason={reason}",
                        Self::kind_label(kind),
                        stage.as_str(),
                        source,
                        modified_at_ms,
                    );
                }
                Ok(())
            }
            PluginFailureAction::FailFast => {
                let message = Self::fail_fast_message(kind, stage, plugin_id, reason);
                self.record_fatal_message(message.clone());
                Err(RuntimeError::PluginFatal(message))
            }
        }
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
    fn invoke(&self, request: &ProtocolRequest) -> Result<ProtocolResponse, ProtocolError> {
        let request_bytes = encode_protocol_request(request)
            .map_err(|error| ProtocolError::Plugin(error.to_string()))?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            (self.invoke)(
                mc_plugin_api::ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw mut output,
                &raw mut error,
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
        decode_protocol_response(request, &response_bytes)
            .map_err(|error| ProtocolError::Plugin(error.to_string()))
    }
}

#[derive(Clone, Copy)]
struct GameplayQueryScope<'a> {
    query: &'a dyn GameplayQuery,
}

thread_local! {
    static CURRENT_GAMEPLAY_QUERY: Cell<Option<*const ()>> = const { Cell::new(None) };
}

/// Runs plugin gameplay code with the current query temporarily published in thread-local state.
///
/// # Safety invariants
///
/// The stored pointer borrows `query`, so it is only valid for the dynamic extent of `f`.
/// Gameplay host callbacks must therefore remain synchronous, stay on the same thread, and never
/// retain the pointer beyond the callback.
fn with_gameplay_query<T>(
    query: &dyn GameplayQuery,
    f: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    CURRENT_GAMEPLAY_QUERY.with(|slot| {
        // The pointer is only published while `f` runs, and nested invocations restore the
        // previous pointer before returning.
        let scope = GameplayQueryScope { query };
        let previous = slot.replace(Some(std::ptr::from_ref(&scope).cast()));
        let result = f();
        let _ = slot.replace(previous);
        result
    })
}

/// Resolves the gameplay query currently published by [`with_gameplay_query`].
///
/// # Safety invariants
///
/// This function may only be reached while `with_gameplay_query` is active on the current thread;
/// otherwise the stored pointer would be dangling or absent. Nested calls are safe because the
/// thread-local slot restores the previous pointer before returning.
fn with_current_gameplay_query<T>(
    f: impl FnOnce(&dyn GameplayQuery) -> Result<T, String>,
) -> Result<T, String> {
    CURRENT_GAMEPLAY_QUERY.with(|slot| {
        let query_scope_ptr = slot
            .get()
            .ok_or_else(|| "gameplay host callback invoked without an active query".to_string())?;
        // SAFETY: `with_gameplay_query` only publishes a pointer to its stack-local
        // `GameplayQueryScope` for the duration of the callback, and this accessor is only used
        // synchronously from that dynamic extent on the same thread.
        let query = unsafe { (&*query_scope_ptr.cast::<GameplayQueryScope<'_>>()).query };
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
pub struct GameplayGeneration {
    generation_id: PluginGenerationId,
    plugin_id: String,
    profile_id: GameplayProfileId,
    capabilities: CapabilitySet,
    invoke: mc_plugin_api::PluginInvokeFn,
    free_buffer: mc_plugin_api::PluginFreeBufferFn,
    _library_guard: Option<Arc<Mutex<Library>>>,
}

impl GameplayGeneration {
    fn invoke(&self, request: &GameplayRequest) -> Result<GameplayResponse, String> {
        let request_bytes = encode_gameplay_request(request).map_err(|error| error.to_string())?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            (self.invoke)(
                mc_plugin_api::ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw mut output,
                &raw mut error,
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
        decode_gameplay_response(request, &response_bytes).map_err(|error| error.to_string())
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
    fn invoke(&self, request: &StorageRequest) -> Result<StorageResponse, StorageError> {
        let request_bytes = encode_storage_request(request)
            .map_err(|error| StorageError::Plugin(error.to_string()))?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            (self.invoke)(
                mc_plugin_api::ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw mut output,
                &raw mut error,
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
        decode_storage_response(request, &response_bytes)
            .map_err(|error| StorageError::Plugin(error.to_string()))
    }
}

#[derive(Clone)]
pub struct AuthGeneration {
    pub generation_id: PluginGenerationId,
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
    fn invoke(&self, request: &AuthRequest) -> Result<AuthResponse, String> {
        let request_bytes = encode_auth_request(request).map_err(|error| error.to_string())?;
        let mut output = OwnedBuffer::empty();
        let mut error = OwnedBuffer::empty();
        let status = unsafe {
            (self.invoke)(
                mc_plugin_api::ByteSlice {
                    ptr: request_bytes.as_ptr(),
                    len: request_bytes.len(),
                },
                &raw mut output,
                &raw mut error,
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
        decode_auth_response(request, &response_bytes).map_err(|error| error.to_string())
    }

    pub const fn mode(&self) -> AuthMode {
        self.mode
    }

    pub fn authenticate_offline(&self, username: &str) -> Result<PlayerId, RuntimeError> {
        match self
            .invoke(&AuthRequest::AuthenticateOffline {
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

    pub fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, RuntimeError> {
        match self
            .invoke(&AuthRequest::AuthenticateOnline {
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

    pub fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .invoke(&AuthRequest::AuthenticateBedrockOffline {
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

    pub fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .invoke(&AuthRequest::AuthenticateBedrockXbl {
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
    pub const fn new(abi_range: PluginAbiRange) -> Self {
        Self { abi_range }
    }
}

struct HotSwappableProtocolAdapter {
    plugin_id: String,
    generation: RwLock<Arc<ProtocolGeneration>>,
    failures: Arc<PluginFailureDispatch>,
    reload_gate: RwLock<()>,
}

impl HotSwappableProtocolAdapter {
    const fn new(
        plugin_id: String,
        generation: Arc<ProtocolGeneration>,
        failures: Arc<PluginFailureDispatch>,
    ) -> Self {
        Self {
            plugin_id,
            generation: RwLock::new(generation),
            failures,
            reload_gate: RwLock::new(()),
        }
    }

    fn current_generation(&self) -> Result<Arc<ProtocolGeneration>, ProtocolError> {
        if self.failures.is_active_quarantined(&self.plugin_id) {
            return Err(ProtocolError::Plugin(
                self.failures
                    .active_reason(&self.plugin_id)
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
        let _guard = self
            .reload_gate
            .write()
            .expect("protocol reload gate should not be poisoned");
        self.swap_generation_while_reloading(generation);
    }

    fn swap_generation_while_reloading(&self, generation: Arc<ProtocolGeneration>) {
        *self
            .generation
            .write()
            .expect("protocol generation lock should not be poisoned") = generation;
    }

    fn quarantine_on_error<T>(&self, result: Result<T, ProtocolError>) -> Result<T, ProtocolError> {
        if let Err(ProtocolError::Plugin(message)) = &result {
            let _ = self.failures.handle_runtime_failure(
                PluginKind::Protocol,
                &self.plugin_id,
                message,
            );
        }
        result
    }

    fn with_generation<T>(
        &self,
        f: impl FnOnce(&ProtocolGeneration) -> Result<T, ProtocolError>,
    ) -> Result<T, ProtocolError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("protocol reload gate should not be poisoned");
        let generation = self.current_generation()?;
        self.quarantine_on_error(f(&generation))
    }

    #[allow(dead_code)]
    pub fn export_session_state(
        &self,
        session: &ProtocolSessionSnapshot,
    ) -> Result<Vec<u8>, RuntimeError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::ExportSessionState {
                session: session.clone(),
            })? {
                ProtocolResponse::SessionTransferBlob(blob) => Ok(blob),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected protocol export payload: {other:?}"
                ))),
            }
        })
        .map_err(|error| RuntimeError::Config(error.to_string()))
    }

    #[allow(dead_code)]
    pub fn import_session_state(
        &self,
        session: &ProtocolSessionSnapshot,
        blob: &[u8],
    ) -> Result<(), RuntimeError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::ImportSessionState {
                session: session.clone(),
                blob: blob.to_vec(),
            })? {
                ProtocolResponse::Empty => Ok(()),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected protocol import payload: {other:?}"
                ))),
            }
        })
        .map_err(|error| RuntimeError::Config(error.to_string()))
    }
}

impl HandshakeProbe for HotSwappableProtocolAdapter {
    fn transport_kind(&self) -> TransportKind {
        self.with_generation(|generation| Ok(generation.descriptor.transport))
            .unwrap_or(TransportKind::Tcp)
    }

    fn try_route(&self, frame: &[u8]) -> Result<Option<HandshakeIntent>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::TryRoute {
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::HandshakeIntent(intent) => Ok(intent),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected try_route response: {other:?}"
                ))),
            }
        })
    }
}

impl WireCodec for HotSwappableProtocolAdapter {
    fn encode_frame(&self, payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeWireFrame {
                payload: payload.to_vec(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_wire_frame response: {other:?}"
                ))),
            }
        })
    }

    fn try_decode_frame(&self, buffer: &mut BytesMut) -> Result<Option<Vec<u8>>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::TryDecodeWireFrame {
                buffer: buffer.to_vec(),
            })? {
                ProtocolResponse::WireFrameDecodeResult(result) => {
                    let Some(WireFrameDecodeResult {
                        frame,
                        bytes_consumed,
                    }) = result
                    else {
                        return Ok(None);
                    };
                    if bytes_consumed > buffer.len() {
                        return Err(ProtocolError::Plugin(format!(
                            "wire codec consumed {bytes_consumed} buffered bytes but only {} were available",
                            buffer.len()
                        )));
                    }
                    let _ = buffer.split_to(bytes_consumed);
                    Ok(Some(frame))
                }
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected try_decode_wire_frame response: {other:?}"
                ))),
            }
        })
    }
}

impl mc_proto_common::SessionAdapter for HotSwappableProtocolAdapter {
    fn wire_codec(&self) -> &dyn WireCodec {
        self
    }

    fn decode_status(&self, frame: &[u8]) -> Result<StatusRequest, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::DecodeStatus {
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::StatusRequest(request) => Ok(request),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected decode_status response: {other:?}"
                ))),
            }
        })
    }

    fn decode_login(&self, frame: &[u8]) -> Result<LoginRequest, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::DecodeLogin {
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::LoginRequest(request) => Ok(request),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected decode_login response: {other:?}"
                ))),
            }
        })
    }

    fn encode_status_response(&self, status: &ServerListStatus) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeStatusResponse {
                status: status.clone(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_status_response payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_status_pong(&self, payload: i64) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeStatusPong { payload })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_status_pong payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_disconnect(
        &self,
        phase: ConnectionPhase,
        reason: &str,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeDisconnect {
                phase,
                reason: reason.to_string(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_disconnect payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_encryption_request(
        &self,
        server_id: &str,
        public_key_der: &[u8],
        verify_token: &[u8],
    ) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeEncryptionRequest {
                server_id: server_id.to_string(),
                public_key_der: public_key_der.to_vec(),
                verify_token: verify_token.to_vec(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_encryption_request payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_network_settings(
        &self,
        compression_threshold: u16,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeNetworkSettings {
                compression_threshold,
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_network_settings payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_login_success(
        &self,
        player: &mc_core::PlayerSnapshot,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodeLoginSuccess {
                player: player.clone(),
            })? {
                ProtocolResponse::Frame(frame) => Ok(frame),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_login_success payload: {other:?}"
                ))),
            }
        })
    }
}

impl mc_proto_common::PlaySyncAdapter for HotSwappableProtocolAdapter {
    fn decode_play(
        &self,
        player_id: mc_core::PlayerId,
        frame: &[u8],
    ) -> Result<Option<mc_core::CoreCommand>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::DecodePlay {
                player_id,
                frame: frame.to_vec(),
            })? {
                ProtocolResponse::CoreCommand(command) => Ok(command),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected decode_play payload: {other:?}"
                ))),
            }
        })
    }

    fn encode_play_event(
        &self,
        event: &mc_core::CoreEvent,
        context: &PlayEncodingContext,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        self.with_generation(|generation| {
            match generation.invoke(&ProtocolRequest::EncodePlayEvent {
                event: event.clone(),
                context: *context,
            })? {
                ProtocolResponse::Frames(frames) => Ok(frames),
                other => Err(ProtocolError::Plugin(format!(
                    "unexpected encode_play_event payload: {other:?}"
                ))),
            }
        })
    }
}

impl ProtocolAdapter for HotSwappableProtocolAdapter {
    fn descriptor(&self) -> ProtocolDescriptor {
        self.with_generation(|generation| Ok(generation.descriptor.clone()))
            .map_or_else(
                |_| ProtocolDescriptor {
                    adapter_id: self.plugin_id.clone(),
                    transport: TransportKind::Tcp,
                    wire_format: WireFormatKind::MinecraftFramed,
                    edition: mc_proto_common::Edition::Je,
                    version_name: "quarantined".to_string(),
                    protocol_number: -1,
                },
                |descriptor| descriptor,
            )
    }

    fn bedrock_listener_descriptor(&self) -> Option<BedrockListenerDescriptor> {
        self.with_generation(|generation| Ok(generation.bedrock_listener_descriptor.clone()))
            .ok()
            .flatten()
    }

    fn capability_set(&self) -> CapabilitySet {
        self.with_generation(|generation| Ok(generation.capabilities.clone()))
            .unwrap_or_default()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.with_generation(|generation| Ok(generation.generation_id))
            .ok()
    }
}

pub struct HotSwappableGameplayProfile {
    plugin_id: String,
    profile_id: GameplayProfileId,
    generation: RwLock<Arc<GameplayGeneration>>,
    failures: Arc<PluginFailureDispatch>,
    reload_gate: RwLock<()>,
}

impl HotSwappableGameplayProfile {
    const fn new(
        plugin_id: String,
        profile_id: GameplayProfileId,
        generation: Arc<GameplayGeneration>,
        failures: Arc<PluginFailureDispatch>,
    ) -> Self {
        Self {
            plugin_id,
            profile_id,
            generation: RwLock::new(generation),
            failures,
            reload_gate: RwLock::new(()),
        }
    }

    fn current_generation(&self) -> Arc<GameplayGeneration> {
        self.generation
            .read()
            .expect("gameplay generation lock should not be poisoned")
            .clone()
    }

    fn swap_generation(&self, generation: Arc<GameplayGeneration>) {
        *self
            .generation
            .write()
            .expect("gameplay generation lock should not be poisoned") = generation;
    }

    pub fn profile_id(&self) -> GameplayProfileId {
        self.profile_id.clone()
    }

    pub fn capability_set(&self) -> CapabilitySet {
        self.current_generation().capabilities.clone()
    }

    pub fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Some(self.current_generation().generation_id)
    }

    pub fn export_session_state(
        &self,
        session: &GameplaySessionSnapshot,
    ) -> Result<Vec<u8>, RuntimeError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        let generation = self.current_generation();
        match generation
            .invoke(&GameplayRequest::ExportSessionState {
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

    pub fn import_session_state(
        &self,
        session: &GameplaySessionSnapshot,
        blob: &[u8],
    ) -> Result<(), RuntimeError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        let generation = self.current_generation();
        match generation
            .invoke(&GameplayRequest::ImportSessionState {
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

    pub fn session_closed(&self, session: &GameplaySessionSnapshot) -> Result<(), RuntimeError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        let generation = self.current_generation();
        match generation
            .invoke(&GameplayRequest::SessionClosed {
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
            entity_id: session.entity_id,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        if self.failures.is_active_quarantined(&self.plugin_id) {
            return Ok(GameplayJoinEffect::default());
        }
        let generation = self.current_generation();
        with_gameplay_query(query, || {
            match generation.invoke(&GameplayRequest::HandlePlayerJoin {
                session,
                player: player.clone(),
            }) {
                Ok(GameplayResponse::JoinEffect(effect)) => Ok(effect),
                Ok(other) => {
                    let message = format!("unexpected gameplay join payload: {other:?}");
                    match self.failures.handle_runtime_failure(
                        PluginKind::Gameplay,
                        &self.plugin_id,
                        &message,
                    ) {
                        PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                            Ok(GameplayJoinEffect::default())
                        }
                        PluginFailureAction::FailFast => Err(message),
                    }
                }
                Err(error) => match self.failures.handle_runtime_failure(
                    PluginKind::Gameplay,
                    &self.plugin_id,
                    &error,
                ) {
                    PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                        Ok(GameplayJoinEffect::default())
                    }
                    PluginFailureAction::FailFast => Err(error),
                },
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
            entity_id: session.entity_id,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        if self.failures.is_active_quarantined(&self.plugin_id) {
            return Ok(GameplayEffect::default());
        }
        let generation = self.current_generation();
        with_gameplay_query(query, || {
            match generation.invoke(&GameplayRequest::HandleCommand {
                session,
                command: command.clone(),
            }) {
                Ok(GameplayResponse::Effect(effect)) => Ok(effect),
                Ok(other) => {
                    let message = format!("unexpected gameplay command payload: {other:?}");
                    match self.failures.handle_runtime_failure(
                        PluginKind::Gameplay,
                        &self.plugin_id,
                        &message,
                    ) {
                        PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                            Ok(GameplayEffect::default())
                        }
                        PluginFailureAction::FailFast => Err(message),
                    }
                }
                Err(error) => match self.failures.handle_runtime_failure(
                    PluginKind::Gameplay,
                    &self.plugin_id,
                    &error,
                ) {
                    PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                        Ok(GameplayEffect::default())
                    }
                    PluginFailureAction::FailFast => Err(error),
                },
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
            entity_id: session.entity_id,
            gameplay_profile: session.gameplay_profile.clone(),
        };
        let _guard = self
            .reload_gate
            .read()
            .expect("gameplay reload gate should not be poisoned");
        if self.failures.is_active_quarantined(&self.plugin_id) {
            return Ok(GameplayEffect::default());
        }
        let generation = self.current_generation();
        with_gameplay_query(query, || {
            match generation.invoke(&GameplayRequest::HandleTick { session, now_ms }) {
                Ok(GameplayResponse::Effect(effect)) => Ok(effect),
                Ok(other) => {
                    let message = format!("unexpected gameplay tick payload: {other:?}");
                    match self.failures.handle_runtime_failure(
                        PluginKind::Gameplay,
                        &self.plugin_id,
                        &message,
                    ) {
                        PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                            Ok(GameplayEffect::default())
                        }
                        PluginFailureAction::FailFast => Err(message),
                    }
                }
                Err(error) => match self.failures.handle_runtime_failure(
                    PluginKind::Gameplay,
                    &self.plugin_id,
                    &error,
                ) {
                    PluginFailureAction::Skip | PluginFailureAction::Quarantine => {
                        Ok(GameplayEffect::default())
                    }
                    PluginFailureAction::FailFast => Err(error),
                },
            }
        })
    }
}

pub struct HotSwappableStorageProfile {
    plugin_id: String,
    #[allow(dead_code)]
    profile_id: String,
    generation: RwLock<Arc<StorageGeneration>>,
    reload_gate: RwLock<()>,
}

impl HotSwappableStorageProfile {
    const fn new(
        plugin_id: String,
        profile_id: String,
        generation: Arc<StorageGeneration>,
    ) -> Self {
        Self {
            plugin_id,
            profile_id,
            generation: RwLock::new(generation),
            reload_gate: RwLock::new(()),
        }
    }

    fn current_generation(&self) -> Result<Arc<StorageGeneration>, StorageError> {
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

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    #[allow(dead_code)]
    pub fn capability_set(&self) -> CapabilitySet {
        self.current_generation()
            .map(|generation| generation.capabilities.clone())
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.current_generation()
            .ok()
            .map(|generation| generation.generation_id)
    }

    pub fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("storage reload gate should not be poisoned");
        let generation = self.current_generation()?;
        match generation.invoke(&StorageRequest::LoadSnapshot {
            world_dir: world_dir.display().to_string(),
        })? {
            StorageResponse::Snapshot(snapshot) => Ok(snapshot),
            other => Err(StorageError::Plugin(format!(
                "unexpected storage load_snapshot payload: {other:?}"
            ))),
        }
    }

    pub fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("storage reload gate should not be poisoned");
        let generation = self.current_generation()?;
        match generation.invoke(&StorageRequest::SaveSnapshot {
            world_dir: world_dir.display().to_string(),
            snapshot: snapshot.clone(),
        })? {
            StorageResponse::Empty => Ok(()),
            other => Err(StorageError::Plugin(format!(
                "unexpected storage save_snapshot payload: {other:?}"
            ))),
        }
    }

    pub fn import_runtime_state(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        let _guard = self
            .reload_gate
            .read()
            .expect("storage reload gate should not be poisoned");
        let generation = self.current_generation()?;
        match generation.invoke(&StorageRequest::ImportRuntimeState {
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

impl StorageAdapter for HotSwappableStorageProfile {
    fn load_snapshot(&self, world_dir: &Path) -> Result<Option<WorldSnapshot>, StorageError> {
        Self::load_snapshot(self, world_dir)
    }

    fn save_snapshot(
        &self,
        world_dir: &Path,
        snapshot: &WorldSnapshot,
    ) -> Result<(), StorageError> {
        Self::save_snapshot(self, world_dir, snapshot)
    }
}

pub struct HotSwappableAuthProfile {
    plugin_id: String,
    #[allow(dead_code)]
    profile_id: String,
    generation: RwLock<Arc<AuthGeneration>>,
    failures: Arc<PluginFailureDispatch>,
}

impl HotSwappableAuthProfile {
    const fn new(
        plugin_id: String,
        profile_id: String,
        generation: Arc<AuthGeneration>,
        failures: Arc<PluginFailureDispatch>,
    ) -> Self {
        Self {
            plugin_id,
            profile_id,
            generation: RwLock::new(generation),
            failures,
        }
    }

    fn current_generation(&self) -> Result<Arc<AuthGeneration>, String> {
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

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    #[allow(dead_code)]
    pub fn capability_set(&self) -> CapabilitySet {
        self.current_generation()
            .map(|generation| generation.capabilities.clone())
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.current_generation()
            .ok()
            .map(|generation| generation.generation_id)
    }

    pub fn mode(&self) -> Result<AuthMode, RuntimeError> {
        self.current_generation()
            .map(|generation| generation.mode())
            .map_err(RuntimeError::Config)
    }

    pub fn capture_generation(&self) -> Result<Arc<AuthGeneration>, RuntimeError> {
        self.current_generation().map_err(RuntimeError::Config)
    }

    pub fn authenticate_offline(&self, username: &str) -> Result<PlayerId, RuntimeError> {
        match self.capture_generation()?.authenticate_offline(username) {
            Ok(player_id) => Ok(player_id),
            Err(RuntimeError::Config(message)) => {
                let _ = self.failures.handle_runtime_failure(
                    PluginKind::Auth,
                    &self.plugin_id,
                    &message,
                );
                Err(RuntimeError::Config(message))
            }
            Err(error) => Err(error),
        }
    }

    pub fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, RuntimeError> {
        match self
            .capture_generation()?
            .authenticate_online(username, server_hash)
        {
            Ok(player_id) => Ok(player_id),
            Err(RuntimeError::Config(message)) => {
                let _ = self.failures.handle_runtime_failure(
                    PluginKind::Auth,
                    &self.plugin_id,
                    &message,
                );
                Err(RuntimeError::Config(message))
            }
            Err(error) => Err(error),
        }
    }

    pub fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .capture_generation()?
            .authenticate_bedrock_offline(display_name)
        {
            Ok(result) => Ok(result),
            Err(RuntimeError::Config(message)) => {
                let _ = self.failures.handle_runtime_failure(
                    PluginKind::Auth,
                    &self.plugin_id,
                    &message,
                );
                Err(RuntimeError::Config(message))
            }
            Err(error) => Err(error),
        }
    }

    pub fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .capture_generation()?
            .authenticate_bedrock_xbl(chain_jwts, client_data_jwt)
        {
            Ok(result) => Ok(result),
            Err(RuntimeError::Config(message)) => {
                let _ = self.failures.handle_runtime_failure(
                    PluginKind::Auth,
                    &self.plugin_id,
                    &message,
                );
                Err(RuntimeError::Config(message))
            }
            Err(error) => Err(error),
        }
    }
}

struct ManagedProtocolPlugin {
    package: PluginPackage,
    adapter: Arc<HotSwappableProtocolAdapter>,
    loaded_at: SystemTime,
    active_loaded_at: SystemTime,
}

pub struct PreparedProtocolTopology {
    pub registry: ProtocolRegistry,
    pub adapter_ids: Vec<String>,
    managed: HashMap<String, ManagedProtocolPlugin>,
}

struct ManagedGameplayPlugin {
    package: PluginPackage,
    profile_id: GameplayProfileId,
    profile: Arc<HotSwappableGameplayProfile>,
    loaded_at: SystemTime,
    active_loaded_at: SystemTime,
}

struct ManagedStoragePlugin {
    package: PluginPackage,
    profile_id: String,
    profile: Arc<HotSwappableStorageProfile>,
    loaded_at: SystemTime,
    active_loaded_at: SystemTime,
}

struct ManagedAuthPlugin {
    package: PluginPackage,
    profile_id: String,
    profile: Arc<HotSwappableAuthProfile>,
    loaded_at: SystemTime,
    active_loaded_at: SystemTime,
}

pub struct PluginHost {
    catalog: PluginCatalog,
    dynamic_catalog_source: Option<DynamicCatalogSource>,
    loader: PluginLoader,
    generations: Arc<GenerationManager>,
    failures: Arc<PluginFailureDispatch>,
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
        failure_matrix: PluginFailureMatrix,
    ) -> Self {
        Self::new_with_dynamic_catalog_source(catalog, abi_range, failure_matrix, None)
    }

    #[must_use]
    pub fn new_with_dynamic_catalog_source(
        catalog: PluginCatalog,
        abi_range: PluginAbiRange,
        failure_matrix: PluginFailureMatrix,
        dynamic_catalog_source: Option<(PathBuf, Option<HashSet<String>>)>,
    ) -> Self {
        Self {
            catalog,
            dynamic_catalog_source: dynamic_catalog_source
                .map(|(root, allowlist)| DynamicCatalogSource { root, allowlist }),
            loader: PluginLoader::new(abi_range),
            generations: Arc::new(GenerationManager::default()),
            failures: Arc::new(PluginFailureDispatch::new(failure_matrix)),
            protocols: Mutex::new(HashMap::new()),
            gameplay: Mutex::new(HashMap::new()),
            storage: Mutex::new(HashMap::new()),
            auth: Mutex::new(HashMap::new()),
        }
    }

    fn protocol_catalog(&self) -> Result<PluginCatalog, RuntimeError> {
        match &self.dynamic_catalog_source {
            Some(source) => PluginCatalog::discover(&source.root, source.allowlist.as_ref()),
            None => Ok(self.catalog.clone()),
        }
    }

    fn prepare_protocol_topology_with_stage(
        &self,
        stage: PluginFailureStage,
    ) -> Result<PreparedProtocolTopology, RuntimeError> {
        let catalog = self.protocol_catalog()?;
        let mut registry = ProtocolRegistry::new();
        let mut managed = HashMap::new();
        let mut adapter_ids = Vec::new();
        for package in catalog.packages() {
            if package.plugin_kind != PluginKind::Protocol {
                continue;
            }
            let modified_at = package.modified_at()?;
            let identity = package.artifact_identity(modified_at);
            if self
                .failures
                .is_artifact_quarantined(&package.plugin_id, &identity)
            {
                if let Some(reason) = self.failures.artifact_reason(&package.plugin_id, &identity) {
                    eprintln!(
                        "skipping quarantined protocol artifact `{}` during {}: {reason}",
                        package.plugin_id,
                        stage.as_str()
                    );
                }
                continue;
            }
            let generation = match self
                .loader
                .load_protocol_generation(package, self.generations.next_generation_id())
            {
                Ok(generation) => Arc::new(generation),
                Err(error) => {
                    let reason = error.to_string();
                    eprintln!(
                        "protocol {} load failed for `{}`: {reason}",
                        stage.as_str(),
                        package.plugin_id
                    );
                    self.failures.handle_candidate_failure(
                        PluginKind::Protocol,
                        stage,
                        &package.plugin_id,
                        identity,
                        &reason,
                    )?;
                    continue;
                }
            };
            let adapter = Arc::new(HotSwappableProtocolAdapter::new(
                package.plugin_id.clone(),
                generation,
                Arc::clone(&self.failures),
            ));
            registry.register_adapter(adapter.clone());
            registry.register_probe(adapter.clone());
            adapter_ids.push(package.plugin_id.clone());
            managed.insert(
                package.plugin_id.clone(),
                ManagedProtocolPlugin {
                    package: package.clone(),
                    adapter,
                    loaded_at: modified_at,
                    active_loaded_at: modified_at,
                },
            );
        }
        adapter_ids.sort();
        Ok(PreparedProtocolTopology {
            registry,
            adapter_ids,
            managed,
        })
    }

    pub(crate) fn prepare_protocol_topology_for_boot(
        &self,
    ) -> Result<PreparedProtocolTopology, RuntimeError> {
        self.prepare_protocol_topology_with_stage(PluginFailureStage::Boot)
    }

    pub fn prepare_protocol_topology_for_reload(
        &self,
    ) -> Result<PreparedProtocolTopology, RuntimeError> {
        self.prepare_protocol_topology_with_stage(PluginFailureStage::Reload)
    }

    pub fn activate_protocol_topology(&self, candidate: PreparedProtocolTopology) {
        *self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned") = candidate.managed;
    }

    /// Registers protocol adapters and probes from the plugin catalog.
    ///
    /// `load_into_registries()` only registers protocol adapters and probes.
    /// Use `initialize_runtime_registries()` when gameplay, storage, and auth
    /// profiles also need to be activated from a concrete `ServerConfig`.
    ///
    /// # Errors
    ///
    /// Returns an error when a protocol plugin cannot be loaded into the runtime registries.
    ///
    /// # Panics
    ///
    /// Panics if the protocol plugin registry mutex is poisoned.
    pub fn load_into_registries(
        self: &Arc<Self>,
        registries: &mut RuntimeRegistries,
    ) -> Result<(), RuntimeError> {
        let prepared = self.prepare_protocol_topology_for_boot()?;
        registries.replace_protocols(prepared.registry.clone());
        self.activate_protocol_topology(prepared);
        registries.attach_plugin_host(Arc::clone(self));
        Ok(())
    }

    /// Resolves an active gameplay profile by id.
    ///
    /// # Panics
    ///
    /// Panics if the gameplay plugin registry mutex is poisoned.
    pub fn resolve_gameplay_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<HotSwappableGameplayProfile>> {
        self.gameplay
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .get(profile_id)
            .map(|managed| Arc::clone(&managed.profile))
    }

    /// Resolves an active storage profile by id.
    ///
    /// # Panics
    ///
    /// Panics if the storage plugin registry mutex is poisoned.
    pub fn resolve_storage_profile(
        &self,
        profile_id: &str,
    ) -> Option<Arc<HotSwappableStorageProfile>> {
        self.storage
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .get(profile_id)
            .map(|managed| Arc::clone(&managed.profile))
    }

    /// Resolves an active auth profile by id.
    ///
    /// # Panics
    ///
    /// Panics if the auth plugin registry mutex is poisoned.
    pub fn resolve_auth_profile(&self, profile_id: &str) -> Option<Arc<HotSwappableAuthProfile>> {
        self.auth
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .get(profile_id)
            .map(|managed| Arc::clone(&managed.profile))
    }

    #[must_use]
    pub fn status(&self) -> PluginHostStatusSnapshot {
        let mut protocols = self
            .protocols
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .values()
            .map(|managed| {
                let generation = managed
                    .adapter
                    .generation
                    .read()
                    .expect("protocol generation lock should not be poisoned")
                    .clone();
                ProtocolPluginStatusSnapshot {
                    plugin_id: managed.package.plugin_id.clone(),
                    adapter_id: generation.descriptor.adapter_id.clone(),
                    generation_id: generation.generation_id,
                    loaded_at_ms: system_time_ms(managed.active_loaded_at),
                    failure_action: self.failures.action_for_kind(PluginKind::Protocol),
                    current_artifact: artifact_status_snapshot(
                        managed.package.artifact_identity(managed.active_loaded_at),
                        None,
                    ),
                    active_quarantine_reason: self
                        .failures
                        .active_reason(&managed.package.plugin_id),
                    artifact_quarantine: self
                        .failures
                        .artifact_record(&managed.package.plugin_id)
                        .map(artifact_quarantine_status_snapshot),
                    version_name: generation.descriptor.version_name.clone(),
                    transport: generation.descriptor.transport,
                    edition: generation.descriptor.edition,
                    protocol_number: generation.descriptor.protocol_number,
                    bedrock_listener_descriptor_present: generation
                        .bedrock_listener_descriptor
                        .is_some(),
                }
            })
            .collect::<Vec<_>>();
        protocols.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));

        let mut gameplay = self
            .gameplay
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .values()
            .map(|managed| {
                let generation = managed
                    .profile
                    .generation
                    .read()
                    .expect("gameplay generation lock should not be poisoned")
                    .clone();
                GameplayPluginStatusSnapshot {
                    plugin_id: managed.package.plugin_id.clone(),
                    profile_id: managed.profile_id.clone(),
                    generation_id: generation.generation_id,
                    loaded_at_ms: system_time_ms(managed.active_loaded_at),
                    failure_action: self.failures.action_for_kind(PluginKind::Gameplay),
                    current_artifact: artifact_status_snapshot(
                        managed.package.artifact_identity(managed.active_loaded_at),
                        None,
                    ),
                    active_quarantine_reason: self
                        .failures
                        .active_reason(&managed.package.plugin_id),
                    artifact_quarantine: self
                        .failures
                        .artifact_record(&managed.package.plugin_id)
                        .map(artifact_quarantine_status_snapshot),
                }
            })
            .collect::<Vec<_>>();
        gameplay.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));

        let mut storage = self
            .storage
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .values()
            .map(|managed| {
                let generation = managed
                    .profile
                    .generation
                    .read()
                    .expect("storage generation lock should not be poisoned")
                    .clone();
                StoragePluginStatusSnapshot {
                    plugin_id: managed.package.plugin_id.clone(),
                    profile_id: managed.profile_id.clone(),
                    generation_id: generation.generation_id,
                    loaded_at_ms: system_time_ms(managed.active_loaded_at),
                    failure_action: self.failures.action_for_kind(PluginKind::Storage),
                    current_artifact: artifact_status_snapshot(
                        managed.package.artifact_identity(managed.active_loaded_at),
                        None,
                    ),
                    active_quarantine_reason: self
                        .failures
                        .active_reason(&managed.package.plugin_id),
                    artifact_quarantine: self
                        .failures
                        .artifact_record(&managed.package.plugin_id)
                        .map(artifact_quarantine_status_snapshot),
                }
            })
            .collect::<Vec<_>>();
        storage.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));

        let mut auth = self
            .auth
            .lock()
            .expect("plugin host mutex should not be poisoned")
            .values()
            .map(|managed| {
                let generation = managed
                    .profile
                    .generation
                    .read()
                    .expect("auth generation lock should not be poisoned")
                    .clone();
                AuthPluginStatusSnapshot {
                    plugin_id: managed.package.plugin_id.clone(),
                    profile_id: managed.profile_id.clone(),
                    generation_id: generation.generation_id,
                    loaded_at_ms: system_time_ms(managed.active_loaded_at),
                    failure_action: self.failures.action_for_kind(PluginKind::Auth),
                    current_artifact: artifact_status_snapshot(
                        managed.package.artifact_identity(managed.active_loaded_at),
                        None,
                    ),
                    active_quarantine_reason: self
                        .failures
                        .active_reason(&managed.package.plugin_id),
                    artifact_quarantine: self
                        .failures
                        .artifact_record(&managed.package.plugin_id)
                        .map(artifact_quarantine_status_snapshot),
                    mode: generation.mode(),
                }
            })
            .collect::<Vec<_>>();
        auth.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));

        PluginHostStatusSnapshot {
            failure_matrix: self.failures.matrix,
            pending_fatal_error: self.failures.pending_fatal_message(),
            protocols,
            gameplay,
            storage,
            auth,
        }
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
    pub fn replace_in_process_protocol_plugin(
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
        let generation = Arc::new(
            self.loader
                .load_protocol_generation(&managed.package, generation_id)?,
        );
        managed.adapter.swap_generation(generation);
        self.failures.clear_plugin_state(&plugin_id);
        managed.loaded_at = managed.package.modified_at()?;
        managed.active_loaded_at = managed.loaded_at;
        drop(protocols);
        Ok(generation_id)
    }

    pub fn quarantine_reason(&self, plugin_id: &str) -> Option<String> {
        self.failures.active_reason(plugin_id)
    }

    pub fn take_pending_fatal_error(&self) -> Option<RuntimeError> {
        self.failures.take_pending_fatal_error()
    }

    pub fn handle_runtime_failure(
        &self,
        kind: PluginKind,
        plugin_id: &str,
        reason: &str,
    ) -> PluginFailureAction {
        self.failures
            .handle_runtime_failure(kind, plugin_id, reason)
    }

    pub fn managed_protocol_ids(&self) -> Vec<String> {
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
}

fn artifact_status_snapshot(
    identity: ArtifactIdentity,
    reason: Option<String>,
) -> PluginArtifactStatusSnapshot {
    PluginArtifactStatusSnapshot {
        source: identity.source,
        modified_at_ms: system_time_ms(identity.modified_at),
        reason,
    }
}

fn artifact_quarantine_status_snapshot(
    record: ArtifactQuarantineRecord,
) -> PluginArtifactStatusSnapshot {
    artifact_status_snapshot(record.identity, Some(record.reason))
}

/// Builds a plugin host from the current server configuration.
///
/// # Errors
///
/// Returns an error when plugin discovery fails or a configured plugin manifest is invalid.
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
    Ok(Some(Arc::new(PluginHost::new_with_dynamic_catalog_source(
        catalog,
        PluginAbiRange {
            min: config.plugin_abi_min,
            max: config.plugin_abi_max,
        },
        PluginFailureMatrix {
            protocol: config.plugin_failure_policy_protocol,
            gameplay: config.plugin_failure_policy_gameplay,
            storage: config.plugin_failure_policy_storage,
            auth: config.plugin_failure_policy_auth,
        },
        Some((config.plugins_dir.clone(), allowlist)),
    ))))
}

#[must_use]
pub const fn plugin_reload_poll_interval_ms() -> u64 {
    PLUGIN_RELOAD_POLL_INTERVAL_MS
}

#[cfg(test)]
#[path = "plugin_host/tests.rs"]
mod tests;
