use crate::PluginHostError as RuntimeError;
use mc_plugin_contract::plugin::PluginKind;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactIdentity {
    pub(crate) source: String,
    pub(crate) modified_at: SystemTime,
}

#[derive(Clone, Debug)]
pub(crate) struct PluginPackage {
    pub(crate) plugin_id: String,
    pub(crate) plugin_kind: PluginKind,
    pub(crate) manifest_path: PathBuf,
    pub(crate) library_path: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct DynamicCatalogSource {
    pub(crate) root: PathBuf,
}

impl PluginPackage {
    pub(crate) fn artifact_sha256(&self) -> Result<[u8; 32], RuntimeError> {
        let mut digest = Sha256::new();
        digest.update(b"RevyCraft plugin artifact v1\0");
        hash_artifact_file(&mut digest, b"manifest\0", &self.manifest_path)?;
        hash_artifact_file(&mut digest, b"library\0", &self.library_path)?;
        Ok(digest.finalize().into())
    }

    pub(crate) fn modified_at(&self) -> Result<SystemTime, RuntimeError> {
        Ok(fs::metadata(&self.manifest_path)?
            .modified()?
            .max(fs::metadata(&self.library_path)?.modified()?))
    }

    pub(crate) fn refresh_dynamic_manifest(&mut self) -> Result<(), RuntimeError> {
        let document: PluginPackageDocument =
            toml::from_str(&fs::read_to_string(&self.manifest_path)?).map_err(|error| {
                RuntimeError::Config(format!(
                    "failed to parse plugin manifest {}: {error}",
                    self.manifest_path.display()
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
        self.library_path = self
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(relative_library_path);
        Ok(())
    }

    pub(crate) fn artifact_identity(&self, modified_at: SystemTime) -> ArtifactIdentity {
        ArtifactIdentity {
            source: format!(
                "{}|{}",
                self.manifest_path.display(),
                self.library_path.display()
            ),
            modified_at,
        }
    }
}

fn hash_artifact_file(digest: &mut Sha256, role: &[u8], path: &Path) -> Result<(), RuntimeError> {
    let mut file = File::open(path)?;
    let expected_len = file.metadata()?.len();
    digest.update(role);
    digest.update(expected_len.to_le_bytes());
    let mut actual_len = 0_u64;
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        actual_len = actual_len
            .checked_add(u64::try_from(read).map_err(|_| {
                RuntimeError::Config(format!(
                    "plugin artifact {} read length did not fit u64",
                    path.display()
                ))
            })?)
            .ok_or_else(|| {
                RuntimeError::Config(format!(
                    "plugin artifact {} exceeded the supported length",
                    path.display()
                ))
            })?;
        digest.update(&chunk[..read]);
    }
    if actual_len != expected_len {
        return Err(RuntimeError::Config(format!(
            "plugin artifact {} changed length while it was being hashed",
            path.display()
        )));
    }
    Ok(())
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
        manifest_path: manifest_path.clone(),
        library_path: manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(relative_library_path),
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
        "admin-surface" => Ok(PluginKind::AdminSurface),
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
