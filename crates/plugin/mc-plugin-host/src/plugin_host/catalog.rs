use crate::PluginHostError as RuntimeError;
use mc_plugin_api::abi::PluginKind;
#[cfg(any(test, feature = "in-process-testing"))]
use mc_plugin_api::host_api::{
    AdminUiPluginApiV1, AuthPluginApiV1, GameplayPluginApiV2, ProtocolPluginApiV1,
    StoragePluginApiV1,
};
#[cfg(any(test, feature = "in-process-testing"))]
use mc_plugin_api::manifest::PluginManifestV1;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactIdentity {
    pub(crate) source: String,
    pub(crate) modified_at: SystemTime,
}

#[cfg(any(test, feature = "in-process-testing"))]
#[derive(Clone, Debug)]
pub struct InProcessProtocolPlugin {
    pub plugin_id: String,
    pub manifest: &'static PluginManifestV1,
    pub api: &'static ProtocolPluginApiV1,
}

#[cfg(any(test, feature = "in-process-testing"))]
#[derive(Clone, Debug)]
pub struct InProcessGameplayPlugin {
    pub plugin_id: String,
    pub manifest: &'static PluginManifestV1,
    pub api: &'static GameplayPluginApiV2,
}

#[cfg(any(test, feature = "in-process-testing"))]
#[derive(Clone, Debug)]
pub struct InProcessStoragePlugin {
    pub plugin_id: String,
    pub manifest: &'static PluginManifestV1,
    pub api: &'static StoragePluginApiV1,
}

#[cfg(any(test, feature = "in-process-testing"))]
#[derive(Clone, Debug)]
pub struct InProcessAuthPlugin {
    pub plugin_id: String,
    pub manifest: &'static PluginManifestV1,
    pub api: &'static AuthPluginApiV1,
}

#[cfg(any(test, feature = "in-process-testing"))]
#[derive(Clone, Debug)]
pub struct InProcessAdminUiPlugin {
    pub plugin_id: String,
    pub manifest: &'static PluginManifestV1,
    pub api: &'static AdminUiPluginApiV1,
}

#[derive(Clone, Debug)]
pub(crate) enum PluginSource {
    DynamicLibrary {
        manifest_path: PathBuf,
        library_path: PathBuf,
    },
    #[cfg(any(test, feature = "in-process-testing"))]
    InProcessProtocol(InProcessProtocolPlugin),
    #[cfg(any(test, feature = "in-process-testing"))]
    InProcessGameplay(InProcessGameplayPlugin),
    #[cfg(any(test, feature = "in-process-testing"))]
    InProcessStorage(InProcessStoragePlugin),
    #[cfg(any(test, feature = "in-process-testing"))]
    InProcessAuth(InProcessAuthPlugin),
    #[cfg(any(test, feature = "in-process-testing"))]
    InProcessAdminUi(InProcessAdminUiPlugin),
}

#[derive(Clone, Debug)]
pub(crate) struct PluginPackage {
    pub(crate) plugin_id: String,
    pub(crate) plugin_kind: PluginKind,
    pub(crate) source: PluginSource,
}

#[derive(Clone, Debug)]
pub(crate) struct DynamicCatalogSource {
    pub(crate) root: PathBuf,
}

impl PluginPackage {
    pub(crate) fn modified_at(&self) -> Result<SystemTime, RuntimeError> {
        match &self.source {
            PluginSource::DynamicLibrary {
                manifest_path,
                library_path,
            } => Ok(fs::metadata(manifest_path)?
                .modified()?
                .max(fs::metadata(library_path)?.modified()?)),
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAuth(_)
            | PluginSource::InProcessAdminUi(_) => Ok(SystemTime::UNIX_EPOCH),
        }
    }

    pub(crate) fn refresh_dynamic_manifest(&mut self) -> Result<(), RuntimeError> {
        let (manifest_path, library_path) = match &mut self.source {
            PluginSource::DynamicLibrary {
                manifest_path,
                library_path,
            } => (manifest_path, library_path),
            #[cfg(any(test, feature = "in-process-testing"))]
            _ => return Ok(()),
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

    pub(crate) fn artifact_identity(&self, modified_at: SystemTime) -> ArtifactIdentity {
        let source = match &self.source {
            PluginSource::DynamicLibrary {
                manifest_path,
                library_path,
            } => format!("{}|{}", manifest_path.display(), library_path.display()),
            #[cfg(any(test, feature = "in-process-testing"))]
            PluginSource::InProcessProtocol(_)
            | PluginSource::InProcessGameplay(_)
            | PluginSource::InProcessStorage(_)
            | PluginSource::InProcessAuth(_)
            | PluginSource::InProcessAdminUi(_) => "in-process".to_string(),
        };
        ArtifactIdentity {
            source,
            modified_at,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PluginCatalog {
    packages: HashMap<String, PluginPackage>,
}

impl PluginCatalog {
    pub(crate) fn discover(
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

    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn register_in_process_protocol_plugin(&mut self, plugin: InProcessProtocolPlugin) {
        self.packages.insert(
            plugin.plugin_id.clone(),
            PluginPackage {
                plugin_id: plugin.plugin_id.clone(),
                plugin_kind: PluginKind::Protocol,
                source: PluginSource::InProcessProtocol(plugin),
            },
        );
    }

    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn register_in_process_gameplay_plugin(&mut self, plugin: InProcessGameplayPlugin) {
        self.packages.insert(
            plugin.plugin_id.clone(),
            PluginPackage {
                plugin_id: plugin.plugin_id.clone(),
                plugin_kind: PluginKind::Gameplay,
                source: PluginSource::InProcessGameplay(plugin),
            },
        );
    }

    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn register_in_process_storage_plugin(&mut self, plugin: InProcessStoragePlugin) {
        self.packages.insert(
            plugin.plugin_id.clone(),
            PluginPackage {
                plugin_id: plugin.plugin_id.clone(),
                plugin_kind: PluginKind::Storage,
                source: PluginSource::InProcessStorage(plugin),
            },
        );
    }

    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn register_in_process_auth_plugin(&mut self, plugin: InProcessAuthPlugin) {
        self.packages.insert(
            plugin.plugin_id.clone(),
            PluginPackage {
                plugin_id: plugin.plugin_id.clone(),
                plugin_kind: PluginKind::Auth,
                source: PluginSource::InProcessAuth(plugin),
            },
        );
    }

    #[cfg(any(test, feature = "in-process-testing"))]
    pub(crate) fn register_in_process_admin_ui_plugin(&mut self, plugin: InProcessAdminUiPlugin) {
        self.packages.insert(
            plugin.plugin_id.clone(),
            PluginPackage {
                plugin_id: plugin.plugin_id.clone(),
                plugin_kind: PluginKind::AdminUi,
                source: PluginSource::InProcessAdminUi(plugin),
            },
        );
    }

    pub(crate) fn packages(&self) -> impl Iterator<Item = &PluginPackage> {
        self.packages.values()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.packages.is_empty()
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
        "admin-ui" => Ok(PluginKind::AdminUi),
        _ => Err(RuntimeError::Config(format!(
            "unsupported plugin kind `{value}`"
        ))),
    }
}

pub(crate) fn current_artifact_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

pub(crate) fn system_time_ms(time: SystemTime) -> u64 {
    u64::try_from(
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or_default()
}
