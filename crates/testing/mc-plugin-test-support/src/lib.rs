use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackagedPluginKind {
    Protocol,
    Gameplay,
    Storage,
    Auth,
}

impl PackagedPluginKind {
    fn manifest_kind(self) -> &'static str {
        match self {
            Self::Protocol => "protocol",
            Self::Gameplay => "gameplay",
            Self::Storage => "storage",
            Self::Auth => "auth",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PackagedPluginSpec<'a> {
    pub cargo_package: &'a str,
    pub plugin_id: &'a str,
    pub kind: PackagedPluginKind,
    pub build_tag: &'a str,
}

#[derive(Debug, thiserror::Error)]
pub enum PackagedPluginTestError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub struct PackagedPluginHarness {
    dist_dir: PathBuf,
    artifact_cache_dir: PathBuf,
    cargo_target_dir: PathBuf,
}

impl PackagedPluginHarness {
    /// # Errors
    ///
    /// Returns an error when the shared packaged-plugin harness cannot be built or loaded.
    pub fn shared() -> Result<&'static Self, PackagedPluginTestError> {
        match PACKAGED_PLUGIN_TEST_HARNESS
            .get_or_init(|| {
                PACKAGED_PLUGIN_TEST_HARNESS_BUILDS
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let root_dir = packaged_plugin_test_workspace_root()
                    .join("target")
                    .join("server-runtime-plugin-test-harness");
                let dist_dir = root_dir.join("runtime").join("plugins");
                let stamp_path = root_dir.join("stamp.txt");
                let stamp = packaged_plugin_test_harness_stamp()?;
                if dist_dir.is_dir()
                    && stamp_path.is_file()
                    && fs::read_to_string(&stamp_path)
                        .map(|current| current == stamp)
                        .unwrap_or(false)
                {
                    return Ok(PackagedPluginHarness {
                        dist_dir,
                        artifact_cache_dir: packaged_plugin_test_workspace_root()
                            .join("target")
                            .join("server-runtime-plugin-artifacts"),
                        cargo_target_dir: packaged_plugin_test_workspace_root()
                            .join("target")
                            .join("server-runtime-packaged-plugin-builds")
                            .join("cargo-target"),
                    });
                }
                if root_dir.exists() {
                    fs::remove_dir_all(&root_dir).map_err(|error| error.to_string())?;
                }
                let harness = PackagedPluginHarness {
                    dist_dir,
                    artifact_cache_dir: packaged_plugin_test_workspace_root()
                        .join("target")
                        .join("server-runtime-plugin-artifacts"),
                    cargo_target_dir: packaged_plugin_test_workspace_root()
                        .join("target")
                        .join("server-runtime-packaged-plugin-builds")
                        .join("cargo-target"),
                };
                fs::create_dir_all(&harness.dist_dir).map_err(|error| error.to_string())?;
                packaged_plugin_test_run_xtask_package_all_plugins(
                    &harness.dist_dir,
                    &harness.cargo_target_dir,
                    PACKAGED_PLUGIN_TEST_HARNESS_TAG,
                )?;
                fs::write(&stamp_path, stamp).map_err(|error| error.to_string())?;
                Ok(harness)
            })
            .as_ref()
        {
            Ok(harness) => Ok(harness),
            Err(error) => Err(PackagedPluginTestError::Message(error.clone())),
        }
    }

    #[must_use]
    pub fn dist_dir(&self) -> &Path {
        &self.dist_dir
    }

    #[must_use]
    pub fn scoped_target_dir(&self, _scope: &str) -> PathBuf {
        self.cargo_target_dir.clone()
    }

    #[cfg(test)]
    #[must_use]
    fn artifact_cache_dir(&self) -> &Path {
        &self.artifact_cache_dir
    }

    /// # Errors
    ///
    /// Returns an error when the packaged harness cannot seed the requested subset.
    pub fn seed_subset(
        &self,
        dist_dir: &Path,
        plugin_ids: &[&str],
    ) -> Result<(), PackagedPluginTestError> {
        if dist_dir.exists() {
            fs::remove_dir_all(dist_dir)?;
        }
        fs::create_dir_all(dist_dir)?;
        let requested = plugin_ids
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        for plugin_id in requested {
            let source = self.dist_dir.join(plugin_id);
            if !source.is_dir() {
                return Err(PackagedPluginTestError::Message(format!(
                    "packaged plugin test harness is missing `{plugin_id}`"
                )));
            }
            copy_packaged_plugin_tree(&source, &dist_dir.join(plugin_id))?;
        }
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error when the packaged plugin cannot be built, cached, or installed.
    pub fn install_protocol_plugin(
        &self,
        cargo_package: &str,
        plugin_id: &str,
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<(), PackagedPluginTestError> {
        self.install_cached_plugin(
            PackagedPluginSpec {
                cargo_package,
                plugin_id,
                kind: PackagedPluginKind::Protocol,
                build_tag,
            },
            dist_dir,
            target_dir,
        )
    }

    /// # Errors
    ///
    /// Returns an error when the packaged plugin cannot be built, cached, or installed.
    pub fn install_gameplay_plugin(
        &self,
        cargo_package: &str,
        plugin_id: &str,
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<(), PackagedPluginTestError> {
        self.install_cached_plugin(
            PackagedPluginSpec {
                cargo_package,
                plugin_id,
                kind: PackagedPluginKind::Gameplay,
                build_tag,
            },
            dist_dir,
            target_dir,
        )
    }

    /// # Errors
    ///
    /// Returns an error when the packaged plugin cannot be built, cached, or installed.
    pub fn install_storage_plugin(
        &self,
        cargo_package: &str,
        plugin_id: &str,
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<(), PackagedPluginTestError> {
        self.install_cached_plugin(
            PackagedPluginSpec {
                cargo_package,
                plugin_id,
                kind: PackagedPluginKind::Storage,
                build_tag,
            },
            dist_dir,
            target_dir,
        )
    }

    /// # Errors
    ///
    /// Returns an error when the packaged plugin cannot be built, cached, or installed.
    pub fn install_auth_plugin(
        &self,
        cargo_package: &str,
        plugin_id: &str,
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<(), PackagedPluginTestError> {
        self.install_cached_plugin(
            PackagedPluginSpec {
                cargo_package,
                plugin_id,
                kind: PackagedPluginKind::Auth,
                build_tag,
            },
            dist_dir,
            target_dir,
        )
    }

    fn install_cached_plugin(
        &self,
        spec: PackagedPluginSpec<'_>,
        dist_dir: &Path,
        target_dir: &Path,
    ) -> Result<(), PackagedPluginTestError> {
        let source = self.build_cached_packaged_plugin_artifact(
            spec.cargo_package,
            target_dir,
            spec.build_tag,
        )?;
        let plugin_dir = dist_dir.join(spec.plugin_id);
        fs::create_dir_all(&plugin_dir)?;
        let file_name = source
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| {
                PackagedPluginTestError::Message(
                    "packaged plugin artifact name missing".to_string(),
                )
            })?;
        let packaged_artifact = packaged_artifact_name(file_name, spec.build_tag);
        let destination = plugin_dir.join(&packaged_artifact);
        link_or_copy_file(&source, &destination)?;
        fs::write(
            plugin_dir.join("plugin.toml"),
            plugin_manifest_contents(spec.plugin_id, spec.kind, &packaged_artifact),
        )?;
        Ok(())
    }

    fn build_cached_packaged_plugin_artifact(
        &self,
        cargo_package: &str,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<PathBuf, PackagedPluginTestError> {
        let _guard = packaged_plugin_test_build_lock()
            .lock()
            .expect("packaged plugin build lock should not be poisoned");
        let artifact_name = dynamic_library_filename(cargo_package);
        let cached_artifact = self
            .artifact_cache_dir
            .join(cargo_package)
            .join(build_tag)
            .join(&artifact_name);
        if cached_artifact.is_file() {
            return Ok(cached_artifact);
        }

        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| std::ffi::OsString::from("cargo"));
        let status = std::process::Command::new(cargo)
            .current_dir(packaged_plugin_test_workspace_root())
            .env("CARGO_TARGET_DIR", target_dir)
            .env("REVY_PLUGIN_BUILD_TAG", build_tag)
            .arg("build")
            .arg("-p")
            .arg(cargo_package)
            .status()
            .map_err(|error| PackagedPluginTestError::Message(error.to_string()))?;
        if !status.success() {
            return Err(PackagedPluginTestError::Message(format!(
                "cargo build failed for `{cargo_package}`"
            )));
        }

        let source = target_dir.join("debug").join(&artifact_name);
        link_or_copy_file(&source, &cached_artifact)?;
        PACKAGED_PLUGIN_TEST_VARIANT_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(cached_artifact)
    }
}

const PACKAGED_PLUGIN_TEST_HARNESS_TAG: &str = "runtime-test-harness";

static PACKAGED_PLUGIN_TEST_HARNESS_BUILDS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static PACKAGED_PLUGIN_TEST_VARIANT_BUILDS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static PACKAGED_PLUGIN_TEST_HARNESS: std::sync::OnceLock<Result<PackagedPluginHarness, String>> =
    std::sync::OnceLock::new();

fn packaged_plugin_test_build_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

fn packaged_plugin_test_workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for ancestor in manifest_dir.ancestors() {
        let manifest = ancestor.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&manifest) else {
            continue;
        };
        if contents.contains("[workspace]") {
            return ancestor.to_path_buf();
        }
    }
    panic!(
        "mc-plugin-test-support crate should live under the workspace root: {}",
        manifest_dir.display()
    );
}

fn packaged_plugin_test_run_xtask_package_all_plugins(
    dist_dir: &Path,
    target_dir: &Path,
    build_tag: &str,
) -> Result<(), String> {
    let _guard = packaged_plugin_test_build_lock()
        .lock()
        .expect("packaged plugin build lock should not be poisoned");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| std::ffi::OsString::from("cargo"));
    let status = std::process::Command::new(cargo)
        .current_dir(packaged_plugin_test_workspace_root())
        .env("CARGO_TARGET_DIR", target_dir)
        .env("REVY_PLUGIN_BUILD_TAG", build_tag)
        .arg("run")
        .arg("-p")
        .arg("xtask")
        .arg("--")
        .arg("package-all-plugins")
        .arg("--dist-dir")
        .arg(dist_dir)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("xtask package-all-plugins failed".to_string())
    }
}

fn packaged_plugin_test_harness_stamp() -> Result<String, String> {
    let mut newest_ms = 0_u128;
    let mut file_count = 0_u64;
    for relative in [
        "Cargo.toml",
        "Cargo.lock",
        "tools/xtask",
        "plugins",
        "crates/core",
        "crates/plugin",
        "crates/protocol",
    ] {
        accumulate_packaged_plugin_test_stamp(
            &packaged_plugin_test_workspace_root().join(relative),
            &mut newest_ms,
            &mut file_count,
        )?;
    }
    Ok(format!("{newest_ms}-{file_count}"))
}

fn accumulate_packaged_plugin_test_stamp(
    path: &Path,
    newest_ms: &mut u128,
    file_count: &mut u64,
) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "failed to inspect packaged plugin test input {}: {error}",
            path.display()
        )
    })?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| {
            format!(
                "failed to read packaged plugin test input directory {}: {error}",
                path.display()
            )
        })? {
            let entry = entry.map_err(|error| error.to_string())?;
            accumulate_packaged_plugin_test_stamp(&entry.path(), newest_ms, file_count)?;
        }
        return Ok(());
    }

    *file_count += 1;
    let modified_ms = metadata
        .modified()
        .map_err(|error| {
            format!(
                "failed to read modification time for {}: {error}",
                path.display()
            )
        })?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            format!(
                "modification time for {} predates unix epoch: {error}",
                path.display()
            )
        })?
        .as_millis();
    *newest_ms = (*newest_ms).max(modified_ms);
    Ok(())
}

fn copy_packaged_plugin_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let entry_type = entry.file_type()?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry_type.is_dir() {
            copy_packaged_plugin_tree(&source_path, &destination_path)?;
            continue;
        }
        if destination_path.exists() {
            fs::remove_file(&destination_path)?;
        }
        if fs::hard_link(&source_path, &destination_path).is_err() {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn link_or_copy_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    if fs::hard_link(source, destination).is_err() {
        fs::copy(source, destination)?;
    }
    Ok(())
}

fn dynamic_library_filename(package: &str) -> String {
    let crate_name = package.replace('-', "_");
    match std::env::consts::OS {
        "windows" => format!("{crate_name}.dll"),
        "macos" => format!("lib{crate_name}.dylib"),
        _ => format!("lib{crate_name}.so"),
    }
}

fn packaged_artifact_name(base_name: &str, build_tag: &str) -> String {
    if let Some((stem, extension)) = base_name.rsplit_once('.') {
        format!("{stem}-{build_tag}.{extension}")
    } else {
        format!("{base_name}-{build_tag}")
    }
}

fn plugin_manifest_contents(
    plugin_id: &str,
    plugin_kind: PackagedPluginKind,
    packaged_artifact: &str,
) -> String {
    format!(
        "[plugin]\nid = \"{plugin_id}\"\nkind = \"{}\"\n\n[artifacts]\n\"{}-{}\" = \"{packaged_artifact}\"\n",
        plugin_kind.manifest_kind(),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

#[cfg(test)]
mod tests {
    use super::{
        PACKAGED_PLUGIN_TEST_HARNESS_BUILDS, PACKAGED_PLUGIN_TEST_VARIANT_BUILDS,
        PackagedPluginHarness, dynamic_library_filename,
    };
    use std::fs;
    use tempfile::tempdir;

    fn tests_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[test]
    fn packaged_plugin_harness_is_built_at_most_once_per_process() {
        let _guard = tests_lock()
            .lock()
            .expect("packaged plugin test lock should not be poisoned");
        let before = PACKAGED_PLUGIN_TEST_HARNESS_BUILDS.load(std::sync::atomic::Ordering::SeqCst);
        let _ = PackagedPluginHarness::shared().expect("first harness load should succeed");
        let _ = PackagedPluginHarness::shared().expect("second harness load should succeed");
        let after = PACKAGED_PLUGIN_TEST_HARNESS_BUILDS.load(std::sync::atomic::Ordering::SeqCst);
        assert!(after == before || after == before + 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn packaged_plugin_seed_subset_copies_only_requested_plugins() {
        let _guard = tests_lock()
            .lock()
            .expect("packaged plugin test lock should not be poisoned");
        let harness = PackagedPluginHarness::shared().expect("harness should load");
        let temp_dir = tempdir().expect("temp dir should be created");
        let dist_dir = temp_dir.path().join("runtime").join("plugins");
        harness
            .seed_subset(&dist_dir, &["je-1_7_10", "auth-offline"])
            .expect("subset seed should succeed");

        assert!(dist_dir.join("je-1_7_10").is_dir());
        assert!(dist_dir.join("auth-offline").is_dir());
        assert!(!dist_dir.join("je-1_8_x").exists());
        assert!(!dist_dir.join("gameplay-canonical").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn packaged_plugin_variant_cache_reuses_same_package_and_tag() {
        let _guard = tests_lock()
            .lock()
            .expect("packaged plugin test lock should not be poisoned");
        let harness = PackagedPluginHarness::shared().expect("harness should load");
        let cache_dir = harness
            .artifact_cache_dir()
            .join("mc-plugin-proto-je-1_7_10")
            .join("cache-hit-v1");
        if cache_dir.exists() {
            fs::remove_dir_all(&cache_dir).expect("cache dir should be removable");
        }

        let target_dir = harness.scoped_target_dir("variant-cache");
        let first_temp_dir = tempdir().expect("first temp dir should be created");
        let second_temp_dir = tempdir().expect("second temp dir should be created");
        let before = PACKAGED_PLUGIN_TEST_VARIANT_BUILDS.load(std::sync::atomic::Ordering::SeqCst);

        harness
            .install_protocol_plugin(
                "mc-plugin-proto-je-1_7_10",
                "je-1_7_10",
                &first_temp_dir.path().join("runtime").join("plugins"),
                &target_dir,
                "cache-hit-v1",
            )
            .expect("first cached install should succeed");
        let after_first =
            PACKAGED_PLUGIN_TEST_VARIANT_BUILDS.load(std::sync::atomic::Ordering::SeqCst);

        harness
            .install_protocol_plugin(
                "mc-plugin-proto-je-1_7_10",
                "je-1_7_10",
                &second_temp_dir.path().join("runtime").join("plugins"),
                &target_dir,
                "cache-hit-v1",
            )
            .expect("second cached install should succeed");
        let after_second =
            PACKAGED_PLUGIN_TEST_VARIANT_BUILDS.load(std::sync::atomic::Ordering::SeqCst);

        let cached_artifact = cache_dir.join(dynamic_library_filename("mc-plugin-proto-je-1_7_10"));
        assert!(cached_artifact.is_file());
        assert_eq!(after_first, before + 1);
        assert_eq!(after_second, after_first);
    }
}
