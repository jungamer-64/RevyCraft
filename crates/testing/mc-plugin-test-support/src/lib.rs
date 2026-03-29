use fs2::FileExt;
use std::fs;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackagedPluginKind {
    Protocol,
    Gameplay,
    Storage,
    Auth,
    AdminSurface,
}

impl PackagedPluginKind {
    const fn manifest_kind(self) -> &'static str {
        match self {
            Self::Protocol => "protocol",
            Self::Gameplay => "gameplay",
            Self::Storage => "storage",
            Self::Auth => "auth",
            Self::AdminSurface => "admin-surface",
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

#[derive(Clone, Debug)]
pub struct PackagedPluginHarness {
    dist: PathBuf,
    artifact_cache: PathBuf,
    cargo_target: PathBuf,
}

#[derive(Clone, Debug)]
struct PackagedPluginOperationError {
    message: String,
    quota_like: bool,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug)]
enum PackagedPluginRecoveryMode {
    Generic,
    SharedHarnessBuild,
    ScopedVariantBuild { target_dir: PathBuf },
}

#[derive(Clone, Debug)]
struct PackagedPluginRecoveryContext {
    cache_root: PathBuf,
    current_stamp: Option<String>,
    mode: PackagedPluginRecoveryMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackagedPluginFailureInjectionPoint {
    SharedHarnessPackageAllPlugins,
    VariantCargoBuild,
    CopyPackagedPluginTree,
    LinkOrCopyFile,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackagedPluginInjectedFailureKind {
    QuotaLike,
    Other,
}

impl PackagedPluginOperationError {
    fn new(message: impl Into<String>, quota_like: bool) -> Self {
        Self {
            message: message.into(),
            quota_like,
        }
    }
}

impl From<PackagedPluginOperationError> for PackagedPluginTestError {
    fn from(value: PackagedPluginOperationError) -> Self {
        Self::Message(value.message)
    }
}

struct PackagedPluginFileLock(std::fs::File);

impl Drop for PackagedPluginFileLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl PackagedPluginHarness {
    /// # Errors
    ///
    /// Returns an error when the shared packaged-plugin harness cannot be built or loaded.
    ///
    /// # Panics
    ///
    /// Panics if the packaged plugin build lock is poisoned.
    pub fn shared() -> Result<&'static Self, PackagedPluginTestError> {
        match PACKAGED_PLUGIN_TEST_HARNESS
            .get_or_init(|| {
                let stamp = packaged_plugin_test_harness_stamp()?;
                Self::build_shared_with_packager(&stamp, |dist_dir, target_dir, build_tag| {
                    packaged_plugin_test_run_xtask_package_all_plugins(
                        dist_dir, target_dir, build_tag,
                    )
                })
                .map_err(|error| error.message)
            })
            .as_ref()
        {
            Ok(harness) => Ok(harness),
            Err(error) => Err(PackagedPluginTestError::Message(error.clone())),
        }
    }

    #[must_use]
    pub fn dist_dir(&self) -> &Path {
        &self.dist
    }

    #[must_use]
    pub fn scoped_target_dir(&self, scope: &str) -> PathBuf {
        self.cargo_target
            .join(sanitize_packaged_plugin_test_scope(scope))
    }

    fn shared_harness_target_dir(&self) -> PathBuf {
        self.cargo_target.join("shared")
    }

    fn current_stamp(&self) -> Option<String> {
        self.artifact_cache
            .parent()
            .and_then(Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .map(ToOwned::to_owned)
    }

    fn cache_root(&self) -> PathBuf {
        self.cargo_target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(packaged_plugin_test_cache_root)
    }

    fn recovery_context(&self, mode: PackagedPluginRecoveryMode) -> PackagedPluginRecoveryContext {
        PackagedPluginRecoveryContext {
            cache_root: self.cache_root(),
            current_stamp: self.current_stamp(),
            mode,
        }
    }

    fn build_shared_with_packager<F>(
        stamp: &str,
        mut package_plugins: F,
    ) -> Result<Self, PackagedPluginOperationError>
    where
        F: FnMut(&Path, &Path, &str) -> Result<(), PackagedPluginOperationError>,
    {
        Self::build_shared_with_packager_in_cache(
            &packaged_plugin_test_cache_root(),
            stamp,
            &mut package_plugins,
        )
    }

    fn build_shared_with_packager_in_cache<F>(
        cache_root: &Path,
        stamp: &str,
        mut package_plugins: F,
    ) -> Result<Self, PackagedPluginOperationError>
    where
        F: FnMut(&Path, &Path, &str) -> Result<(), PackagedPluginOperationError>,
    {
        let root_dir = cache_root.join(stamp);
        let dist_dir = root_dir.join("runtime").join("plugins");
        let artifact_cache_dir = root_dir.join("artifacts");
        let cargo_target_dir = cache_root.join("cargo-targets");
        let stamp_path = root_dir.join("stamp.txt");
        let harness = Self {
            dist: dist_dir,
            artifact_cache: artifact_cache_dir,
            cargo_target: cargo_target_dir,
        };
        if packaged_plugin_harness_is_ready(&harness, &stamp_path, stamp)
            .map_err(PackagedPluginOperationError::from_message)?
        {
            ensure_packaged_plugin_harness_dirs(&harness)?;
            return Ok(harness);
        }

        let recovery_context = PackagedPluginRecoveryContext {
            cache_root: cache_root.to_path_buf(),
            current_stamp: Some(stamp.to_string()),
            mode: PackagedPluginRecoveryMode::SharedHarnessBuild,
        };
        run_packaged_plugin_operation_with_quota_recovery(recovery_context, || {
            let _guard = packaged_plugin_test_build_lock()
                .lock()
                .expect("packaged plugin build lock should not be poisoned");
            let _file_lock = packaged_plugin_test_file_lock(
                &packaged_plugin_test_shared_lock_path_in(cache_root),
            )?;
            if packaged_plugin_harness_is_ready(&harness, &stamp_path, stamp)
                .map_err(PackagedPluginOperationError::from_message)?
            {
                ensure_packaged_plugin_harness_dirs(&harness)?;
                return Ok(harness.clone());
            }
            if root_dir.exists() {
                fs::remove_dir_all(&root_dir).map_err(|error| {
                    PackagedPluginOperationError::from_io(
                        format!(
                            "failed to remove packaged plugin stamp root {}",
                            root_dir.display()
                        ),
                        error,
                    )
                })?;
            }
            ensure_packaged_plugin_harness_dirs(&harness)?;
            package_plugins(
                &harness.dist,
                &harness.shared_harness_target_dir(),
                PACKAGED_PLUGIN_TEST_HARNESS_TAG,
            )?;
            PACKAGED_PLUGIN_TEST_HARNESS_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            fs::write(&stamp_path, stamp).map_err(|error| {
                PackagedPluginOperationError::from_io(
                    format!(
                        "failed to write packaged plugin harness stamp {}",
                        stamp_path.display()
                    ),
                    error,
                )
            })?;
            Ok(harness.clone())
        })
    }

    #[cfg(test)]
    #[must_use]
    fn artifact_cache_dir(&self) -> &Path {
        &self.artifact_cache
    }

    #[cfg(test)]
    #[must_use]
    fn cargo_target_dir(&self) -> &Path {
        &self.cargo_target
    }

    /// # Errors
    ///
    /// Returns an error when the packaged harness cannot seed the requested subset.
    pub fn seed_subset(
        &self,
        dist_dir: &Path,
        plugin_ids: &[&str],
    ) -> Result<(), PackagedPluginTestError> {
        let requested = plugin_ids
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let recovery_context =
            self.recovery_context(PackagedPluginRecoveryMode::SharedHarnessBuild);
        run_packaged_plugin_operation_with_quota_recovery(recovery_context, || {
            if dist_dir.exists() {
                fs::remove_dir_all(dist_dir).map_err(|error| {
                    PackagedPluginOperationError::from_io(
                        format!(
                            "failed to clear seeded packaged plugin dir {}",
                            dist_dir.display()
                        ),
                        error,
                    )
                })?;
            }
            fs::create_dir_all(dist_dir).map_err(|error| {
                PackagedPluginOperationError::from_io(
                    format!(
                        "failed to create seeded packaged plugin dir {}",
                        dist_dir.display()
                    ),
                    error,
                )
            })?;
            for plugin_id in &requested {
                let source = self.dist.join(plugin_id);
                if !source.is_dir() {
                    return Err(PackagedPluginOperationError::from_message(format!(
                        "packaged plugin test harness is missing `{plugin_id}`"
                    )));
                }
                copy_packaged_plugin_tree(&source, &dist_dir.join(plugin_id)).map_err(|error| {
                    PackagedPluginOperationError::from_io(
                        format!(
                            "failed to seed packaged plugin {} into {}",
                            source.display(),
                            dist_dir.display()
                        ),
                        error,
                    )
                })?;
            }
            Ok(())
        })
        .map_err(PackagedPluginTestError::from)
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
    pub fn install_protocol_plugin_for_reload(
        &self,
        cargo_package: &str,
        plugin_id: &str,
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<(), PackagedPluginTestError> {
        self.install_cached_plugin_for_reload(
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
    pub fn install_gameplay_plugin_for_reload(
        &self,
        cargo_package: &str,
        plugin_id: &str,
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<(), PackagedPluginTestError> {
        self.install_cached_plugin_for_reload(
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
    pub fn install_storage_plugin_for_reload(
        &self,
        cargo_package: &str,
        plugin_id: &str,
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<(), PackagedPluginTestError> {
        self.install_cached_plugin_for_reload(
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

    /// # Errors
    ///
    /// Returns an error when the packaged plugin cannot be built, cached, or installed.
    pub fn install_auth_plugin_for_reload(
        &self,
        cargo_package: &str,
        plugin_id: &str,
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<(), PackagedPluginTestError> {
        self.install_cached_plugin_for_reload(
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

    /// # Errors
    ///
    /// Returns an error when the packaged plugin cannot be built, cached, or installed.
    pub fn install_admin_surface_plugin(
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
                kind: PackagedPluginKind::AdminSurface,
                build_tag,
            },
            dist_dir,
            target_dir,
        )
    }

    /// # Errors
    ///
    /// Returns an error when the packaged plugin cannot be built, cached, or installed.
    pub fn install_admin_surface_plugin_for_reload(
        &self,
        cargo_package: &str,
        plugin_id: &str,
        dist_dir: &Path,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<(), PackagedPluginTestError> {
        self.install_cached_plugin_for_reload(
            PackagedPluginSpec {
                cargo_package,
                plugin_id,
                kind: PackagedPluginKind::AdminSurface,
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
        let recovery_context =
            self.recovery_context(PackagedPluginRecoveryMode::ScopedVariantBuild {
                target_dir: target_dir.to_path_buf(),
            });
        run_packaged_plugin_operation_with_quota_recovery(recovery_context, || {
            let plugin_dir = dist_dir.join(spec.plugin_id);
            fs::create_dir_all(&plugin_dir).map_err(|error| {
                PackagedPluginOperationError::from_io(
                    format!(
                        "failed to create packaged plugin dir {}",
                        plugin_dir.display()
                    ),
                    error,
                )
            })?;
            let file_name = source
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .ok_or_else(|| {
                    PackagedPluginOperationError::from_message(
                        "packaged plugin artifact name missing",
                    )
                })?;
            let packaged_artifact = packaged_artifact_name(file_name, spec.build_tag);
            let destination = plugin_dir.join(&packaged_artifact);
            link_or_copy_file(&source, &destination).map_err(|error| {
                PackagedPluginOperationError::from_io(
                    format!(
                        "failed to place packaged artifact {} into {}",
                        source.display(),
                        destination.display()
                    ),
                    error,
                )
            })?;
            fs::write(
                plugin_dir.join("plugin.toml"),
                plugin_manifest_contents(spec.plugin_id, spec.kind, &packaged_artifact),
            )
            .map_err(|error| {
                PackagedPluginOperationError::from_io(
                    format!(
                        "failed to write packaged plugin manifest {}",
                        plugin_dir.join("plugin.toml").display()
                    ),
                    error,
                )
            })?;
            Ok(())
        })
        .map_err(PackagedPluginTestError::from)
    }

    fn install_cached_plugin_for_reload(
        &self,
        spec: PackagedPluginSpec<'_>,
        dist_dir: &Path,
        target_dir: &Path,
    ) -> Result<(), PackagedPluginTestError> {
        let plugin_dir = dist_dir.join(spec.plugin_id);
        let previous_modified_at = latest_modified_at_in_dir(&plugin_dir)?;
        self.install_cached_plugin(spec, dist_dir, target_dir)?;
        ensure_reload_visible_plugin_dir(&plugin_dir, previous_modified_at)?;
        Ok(())
    }

    fn build_cached_packaged_plugin_artifact(
        &self,
        cargo_package: &str,
        target_dir: &Path,
        build_tag: &str,
    ) -> Result<PathBuf, PackagedPluginTestError> {
        self.build_cached_packaged_plugin_artifact_with_builder(
            cargo_package,
            target_dir,
            build_tag,
            |cargo_package, target_dir, build_tag| {
                packaged_plugin_test_run_cargo_build(cargo_package, target_dir, build_tag)?;
                Ok(target_dir
                    .join("debug")
                    .join(dynamic_library_filename(cargo_package)))
            },
        )
    }

    fn build_cached_packaged_plugin_artifact_with_builder<F>(
        &self,
        cargo_package: &str,
        target_dir: &Path,
        build_tag: &str,
        mut build_artifact: F,
    ) -> Result<PathBuf, PackagedPluginTestError>
    where
        F: FnMut(&str, &Path, &str) -> Result<PathBuf, PackagedPluginOperationError>,
    {
        let artifact_name = dynamic_library_filename(cargo_package);
        let cached_artifact = self
            .artifact_cache
            .join(cargo_package)
            .join(build_tag)
            .join(&artifact_name);
        let recovery_context =
            self.recovery_context(PackagedPluginRecoveryMode::ScopedVariantBuild {
                target_dir: target_dir.to_path_buf(),
            });
        run_packaged_plugin_operation_with_quota_recovery(recovery_context, || {
            if cached_artifact.is_file() {
                return Ok(cached_artifact.clone());
            }

            let _guard = packaged_plugin_test_build_lock()
                .lock()
                .expect("packaged plugin build lock should not be poisoned");
            let _file_lock =
                packaged_plugin_test_file_lock(&packaged_plugin_test_shared_lock_path())?;
            if cached_artifact.is_file() {
                return Ok(cached_artifact.clone());
            }
            fs::create_dir_all(target_dir).map_err(|error| {
                PackagedPluginOperationError::from_io(
                    format!(
                        "failed to create packaged plugin target dir {}",
                        target_dir.display()
                    ),
                    error,
                )
            })?;

            let source = build_artifact(cargo_package, target_dir, build_tag)?;
            link_or_copy_file(&source, &cached_artifact).map_err(|error| {
                PackagedPluginOperationError::from_io(
                    format!(
                        "failed to cache packaged plugin artifact {} into {}",
                        source.display(),
                        cached_artifact.display()
                    ),
                    error,
                )
            })?;
            PACKAGED_PLUGIN_TEST_VARIANT_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(cached_artifact.clone())
        })
        .map_err(PackagedPluginTestError::from)
    }
}

const PACKAGED_PLUGIN_TEST_HARNESS_TAG: &str = "runtime-test-harness";
const RELOAD_VISIBILITY_TIMEOUT: Duration = Duration::from_secs(3);
const RELOAD_VISIBILITY_POLL: Duration = Duration::from_millis(25);
const LINUX_ENOSPC: i32 = 28;
const LINUX_EDQUOT: i32 = 122;

static PACKAGED_PLUGIN_TEST_HARNESS_BUILDS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static PACKAGED_PLUGIN_TEST_VARIANT_BUILDS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static PACKAGED_PLUGIN_TEST_HARNESS: std::sync::OnceLock<Result<PackagedPluginHarness, String>> =
    std::sync::OnceLock::new();

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct PackagedPluginInjectedFailure {
    operation: PackagedPluginFailureInjectionPoint,
    kind: PackagedPluginInjectedFailureKind,
}

#[cfg(test)]
#[derive(Default)]
struct PackagedPluginInjectedFailureState {
    next_failure: Option<PackagedPluginInjectedFailure>,
}

impl PackagedPluginOperationError {
    fn from_message(message: impl Into<String>) -> Self {
        Self::new(message, false)
    }

    fn from_io(action: impl Into<String>, error: std::io::Error) -> Self {
        let action = action.into();
        let quota_like = packaged_plugin_test_error_is_quota_like(&error);
        Self::new(format!("{action}: {error}"), quota_like)
    }

    fn from_command_output(action: impl Into<String>, output: &Output) -> Self {
        let action = action.into();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut message = format!("{action} failed with status {}", output.status);
        if !stderr.trim().is_empty() {
            message.push_str("\nstderr:\n");
            message.push_str(stderr.trim());
        }
        if !stdout.trim().is_empty() {
            message.push_str("\nstdout:\n");
            message.push_str(stdout.trim());
        }
        Self::new(message, packaged_plugin_test_output_is_quota_like(output))
    }
}

fn run_packaged_plugin_operation_with_quota_recovery<T, F>(
    recovery_context: PackagedPluginRecoveryContext,
    mut operation: F,
) -> Result<T, PackagedPluginOperationError>
where
    F: FnMut() -> Result<T, PackagedPluginOperationError>,
{
    match operation() {
        Ok(value) => Ok(value),
        Err(initial_error) if initial_error.quota_like => {
            let cleanup_result = (|| -> Result<(), PackagedPluginOperationError> {
                let _guard = packaged_plugin_test_build_lock()
                    .lock()
                    .expect("packaged plugin build lock should not be poisoned");
                let _file_lock = packaged_plugin_test_file_lock(
                    &packaged_plugin_test_shared_lock_path_in(&recovery_context.cache_root),
                )?;
                packaged_plugin_test_recover_from_quota_failure(&recovery_context)
                    .map_err(PackagedPluginOperationError::from_message)
            })();
            if let Err(cleanup_error) = cleanup_result {
                return Err(PackagedPluginOperationError::new(
                    format!(
                        "{}\nquota recovery failed: {}",
                        initial_error.message, cleanup_error.message
                    ),
                    true,
                ));
            }
            match operation() {
                Ok(value) => Ok(value),
                Err(retry_error) => Err(PackagedPluginOperationError::new(
                    format!(
                        "{}\nquota recovery retry failed: {}",
                        initial_error.message, retry_error.message
                    ),
                    true,
                )),
            }
        }
        Err(error) => Err(error),
    }
}

fn packaged_plugin_test_recover_from_quota_failure(
    recovery_context: &PackagedPluginRecoveryContext,
) -> Result<(), String> {
    packaged_plugin_test_remove_stale_stamp_roots(
        &recovery_context.cache_root,
        recovery_context.current_stamp.as_deref(),
    )?;
    let cargo_target_root = recovery_context.cache_root.join("cargo-targets");
    packaged_plugin_test_remove_non_shared_scoped_targets(&cargo_target_root)?;
    match &recovery_context.mode {
        PackagedPluginRecoveryMode::Generic => {}
        PackagedPluginRecoveryMode::SharedHarnessBuild => {
            packaged_plugin_test_prune_shared_debug_dirs(&cargo_target_root.join("shared"))?;
        }
        PackagedPluginRecoveryMode::ScopedVariantBuild { target_dir } => {
            if target_dir.exists() {
                fs::remove_dir_all(target_dir).map_err(|error| {
                    format!(
                        "failed to remove scoped packaged plugin target dir {}: {error}",
                        target_dir.display()
                    )
                })?;
            }
        }
    }
    Ok(())
}

fn packaged_plugin_test_remove_stale_stamp_roots(
    cache_root: &Path,
    current_stamp: Option<&str>,
) -> Result<(), String> {
    if !cache_root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(cache_root).map_err(|error| {
        format!(
            "failed to inspect packaged plugin cache root {}: {error}",
            cache_root.display()
        )
    })? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if file_name == "cargo-targets" || current_stamp.is_some_and(|stamp| stamp == file_name) {
            continue;
        }
        fs::remove_dir_all(entry.path()).map_err(|error| {
            format!(
                "failed to remove stale packaged plugin stamp root {}: {error}",
                entry.path().display()
            )
        })?;
    }
    Ok(())
}

fn packaged_plugin_test_remove_non_shared_scoped_targets(
    cargo_target_root: &Path,
) -> Result<(), String> {
    if !cargo_target_root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(cargo_target_root).map_err(|error| {
        format!(
            "failed to inspect packaged plugin cargo target root {}: {error}",
            cargo_target_root.display()
        )
    })? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        if entry.file_name().to_str() == Some("shared") {
            continue;
        }
        fs::remove_dir_all(entry.path()).map_err(|error| {
            format!(
                "failed to remove scoped packaged plugin target dir {}: {error}",
                entry.path().display()
            )
        })?;
    }
    Ok(())
}

fn packaged_plugin_test_prune_shared_debug_dirs(shared_target_dir: &Path) -> Result<(), String> {
    let debug_dir = shared_target_dir.join("debug");
    for relative in ["deps", "incremental", "build", ".fingerprint", "examples"] {
        let path = debug_dir.join(relative);
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| {
                format!(
                    "failed to prune packaged plugin shared debug dir {}: {error}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn packaged_plugin_test_error_is_quota_like(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(LINUX_ENOSPC) | Some(LINUX_EDQUOT)
    ) || packaged_plugin_test_text_is_quota_like(&error.to_string())
}

fn packaged_plugin_test_output_is_quota_like(output: &Output) -> bool {
    packaged_plugin_test_text_is_quota_like(&String::from_utf8_lossy(&output.stderr))
        || packaged_plugin_test_text_is_quota_like(&String::from_utf8_lossy(&output.stdout))
}

fn packaged_plugin_test_text_is_quota_like(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("disk quota exceeded") || text.contains("no space left on device")
}

fn ensure_packaged_plugin_harness_dirs(
    harness: &PackagedPluginHarness,
) -> Result<(), PackagedPluginOperationError> {
    fs::create_dir_all(&harness.dist).map_err(|error| {
        PackagedPluginOperationError::from_io(
            format!(
                "failed to create packaged plugin dist dir {}",
                harness.dist.display()
            ),
            error,
        )
    })?;
    fs::create_dir_all(&harness.artifact_cache).map_err(|error| {
        PackagedPluginOperationError::from_io(
            format!(
                "failed to create packaged plugin artifact cache {}",
                harness.artifact_cache.display()
            ),
            error,
        )
    })?;
    fs::create_dir_all(&harness.cargo_target).map_err(|error| {
        PackagedPluginOperationError::from_io(
            format!(
                "failed to create packaged plugin cargo target root {}",
                harness.cargo_target.display()
            ),
            error,
        )
    })?;
    fs::create_dir_all(harness.shared_harness_target_dir()).map_err(|error| {
        PackagedPluginOperationError::from_io(
            format!(
                "failed to create shared packaged plugin target dir {}",
                harness.shared_harness_target_dir().display()
            ),
            error,
        )
    })?;
    Ok(())
}

#[cfg(test)]
fn packaged_plugin_injected_failure_state()
-> &'static std::sync::Mutex<PackagedPluginInjectedFailureState> {
    static STATE: std::sync::OnceLock<std::sync::Mutex<PackagedPluginInjectedFailureState>> =
        std::sync::OnceLock::new();
    STATE.get_or_init(|| std::sync::Mutex::new(PackagedPluginInjectedFailureState::default()))
}

#[cfg(test)]
fn set_packaged_plugin_injected_failure(
    operation: PackagedPluginFailureInjectionPoint,
    kind: PackagedPluginInjectedFailureKind,
) {
    let mut state = packaged_plugin_injected_failure_state()
        .lock()
        .expect("packaged plugin test failure state should not be poisoned");
    state.next_failure = Some(PackagedPluginInjectedFailure { operation, kind });
}

#[cfg(test)]
fn clear_packaged_plugin_injected_failure() {
    let mut state = packaged_plugin_injected_failure_state()
        .lock()
        .expect("packaged plugin test failure state should not be poisoned");
    state.next_failure = None;
}

#[cfg(test)]
fn take_packaged_plugin_injected_failure(
    operation: PackagedPluginFailureInjectionPoint,
) -> Option<PackagedPluginInjectedFailureKind> {
    let mut state = packaged_plugin_injected_failure_state()
        .lock()
        .expect("packaged plugin test failure state should not be poisoned");
    let failure = state.next_failure?;
    if failure.operation == operation {
        state.next_failure = None;
        Some(failure.kind)
    } else {
        None
    }
}

#[cfg(not(test))]
fn take_packaged_plugin_injected_failure(
    _operation: PackagedPluginFailureInjectionPoint,
) -> Option<PackagedPluginInjectedFailureKind> {
    None
}

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

fn packaged_plugin_test_cache_root() -> PathBuf {
    packaged_plugin_test_workspace_root()
        .join("target")
        .join("revy-server-runtime-plugin-test-cache")
}

#[cfg(test)]
fn packaged_plugin_test_cargo_target_root() -> PathBuf {
    packaged_plugin_test_cache_root().join("cargo-targets")
}

fn packaged_plugin_test_shared_lock_path() -> PathBuf {
    packaged_plugin_test_shared_lock_path_in(&packaged_plugin_test_cache_root())
}

fn packaged_plugin_test_shared_lock_path_in(cache_root: &Path) -> PathBuf {
    cache_root.join(".build.lock")
}

fn packaged_plugin_test_file_lock(
    lock_path: &Path,
) -> Result<PackagedPluginFileLock, PackagedPluginOperationError> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            PackagedPluginOperationError::from_io(
                format!(
                    "failed to create packaged plugin lock dir {}",
                    parent.display()
                ),
                error,
            )
        })?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|error| {
            PackagedPluginOperationError::from_io(
                format!(
                    "failed to open packaged plugin lock file {}",
                    lock_path.display()
                ),
                error,
            )
        })?;
    file.lock_exclusive().map_err(|error| {
        PackagedPluginOperationError::from_io(
            format!(
                "failed to lock packaged plugin build cache {}",
                lock_path.display()
            ),
            std::io::Error::other(format!("exclusive file lock failed: {error}")),
        )
    })?;
    Ok(PackagedPluginFileLock(file))
}

fn packaged_plugin_harness_is_ready(
    harness: &PackagedPluginHarness,
    stamp_path: &Path,
    stamp: &str,
) -> Result<bool, String> {
    Ok(harness.dist.is_dir()
        && stamp_path.is_file()
        && fs::read_to_string(stamp_path)
            .map(|current| current == stamp)
            .unwrap_or(false)
        && packaged_plugin_harness_is_consistent(&harness.dist)?)
}

fn sanitize_packaged_plugin_test_scope(scope: &str) -> String {
    scope
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch,
            _ => '_',
        })
        .collect()
}

fn packaged_plugin_test_run_xtask_package_all_plugins(
    dist_dir: &Path,
    target_dir: &Path,
    build_tag: &str,
) -> Result<(), PackagedPluginOperationError> {
    maybe_inject_packaged_plugin_operation_failure(
        PackagedPluginFailureInjectionPoint::SharedHarnessPackageAllPlugins,
        "injected packaged plugin harness packaging failure",
    )?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| std::ffi::OsString::from("cargo"));
    let mut command = Command::new(&cargo);
    command
        .current_dir(packaged_plugin_test_workspace_root())
        .env("CARGO_TARGET_DIR", target_dir)
        .env("REVY_PLUGIN_BUILD_TAG", build_tag)
        .arg("run")
        .arg("-p")
        .arg("xtask")
        .arg("--")
        .arg("package-all-plugins")
        .arg("--dist-dir")
        .arg(dist_dir);
    packaged_plugin_test_run_command(
        command,
        format!("xtask package-all-plugins into {}", dist_dir.display()),
    )
}

fn packaged_plugin_test_run_cargo_build(
    cargo_package: &str,
    target_dir: &Path,
    build_tag: &str,
) -> Result<(), PackagedPluginOperationError> {
    maybe_inject_packaged_plugin_operation_failure(
        PackagedPluginFailureInjectionPoint::VariantCargoBuild,
        format!("injected cargo build failure for `{cargo_package}`"),
    )?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| std::ffi::OsString::from("cargo"));
    let mut command = Command::new(&cargo);
    command
        .current_dir(packaged_plugin_test_workspace_root())
        .env("CARGO_TARGET_DIR", target_dir)
        .env("REVY_PLUGIN_BUILD_TAG", build_tag)
        .arg("build")
        .arg("-p")
        .arg(cargo_package);
    packaged_plugin_test_run_command(
        command,
        format!("cargo build for packaged plugin `{cargo_package}`"),
    )
}

fn packaged_plugin_test_run_command(
    mut command: Command,
    action: impl Into<String>,
) -> Result<(), PackagedPluginOperationError> {
    let action = action.into();
    let output = command
        .output()
        .map_err(|error| PackagedPluginOperationError::from_io(&action, error))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(PackagedPluginOperationError::from_command_output(
            action, &output,
        ))
    }
}

fn packaged_plugin_test_harness_stamp() -> Result<String, String> {
    let mut newest_ms = 0_u128;
    let mut file_count = 0_u64;
    for relative in [
        "Cargo.toml",
        "Cargo.lock",
        "tools/xtask",
        "crates/testing",
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

fn maybe_inject_packaged_plugin_io_failure(
    operation: PackagedPluginFailureInjectionPoint,
) -> std::io::Result<()> {
    match take_packaged_plugin_injected_failure(operation) {
        Some(PackagedPluginInjectedFailureKind::QuotaLike) => {
            Err(std::io::Error::from_raw_os_error(LINUX_EDQUOT))
        }
        Some(PackagedPluginInjectedFailureKind::Other) => {
            Err(std::io::Error::other("injected packaged plugin failure"))
        }
        None => Ok(()),
    }
}

fn maybe_inject_packaged_plugin_operation_failure(
    operation: PackagedPluginFailureInjectionPoint,
    action: impl Into<String>,
) -> Result<(), PackagedPluginOperationError> {
    maybe_inject_packaged_plugin_io_failure(operation)
        .map_err(|error| PackagedPluginOperationError::from_io(action, error))
}

fn copy_packaged_plugin_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    maybe_inject_packaged_plugin_io_failure(
        PackagedPluginFailureInjectionPoint::CopyPackagedPluginTree,
    )?;
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
        if source_path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name == "plugin.toml")
        {
            fs::copy(&source_path, &destination_path)?;
            continue;
        }
        if fs::hard_link(&source_path, &destination_path).is_err() {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn link_or_copy_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    maybe_inject_packaged_plugin_io_failure(PackagedPluginFailureInjectionPoint::LinkOrCopyFile)?;
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

fn latest_modified_at_in_dir(path: &Path) -> Result<Option<SystemTime>, PackagedPluginTestError> {
    if !path.is_dir() {
        return Ok(None);
    }

    let mut latest: Option<SystemTime> = None;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let modified_at = entry.metadata()?.modified()?;
        latest = Some(match latest {
            Some(current) => current.max(modified_at),
            None => modified_at,
        });
    }
    Ok(latest)
}

fn ensure_reload_visible_plugin_dir(
    plugin_dir: &Path,
    previous_modified_at: Option<SystemTime>,
) -> Result<(), PackagedPluginTestError> {
    let Some(previous_modified_at) = previous_modified_at else {
        return Ok(());
    };
    let manifest_path = plugin_dir.join("plugin.toml");
    let manifest_contents = fs::read(&manifest_path)?;
    let deadline = Instant::now() + RELOAD_VISIBILITY_TIMEOUT;

    loop {
        if latest_modified_at_in_dir(plugin_dir)?
            .is_some_and(|current_modified_at| current_modified_at > previous_modified_at)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(PackagedPluginTestError::Message(format!(
                "timed out waiting for reload-visible artifact update in {}",
                plugin_dir.display()
            )));
        }
        std::thread::sleep(RELOAD_VISIBILITY_POLL);
        fs::write(&manifest_path, &manifest_contents)?;
    }
}

fn packaged_artifact_name(base_name: &str, build_tag: &str) -> String {
    if let Some((stem, extension)) = base_name.rsplit_once('.') {
        format!("{stem}-{build_tag}.{extension}")
    } else {
        format!("{base_name}-{build_tag}")
    }
}

fn packaged_plugin_harness_is_consistent(dist_dir: &Path) -> Result<bool, String> {
    if !dist_dir.is_dir() {
        return Ok(false);
    }
    for entry in fs::read_dir(dist_dir).map_err(|error| {
        format!(
            "failed to inspect packaged plugin harness directory {}: {error}",
            dist_dir.display()
        )
    })? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        if !packaged_plugin_dir_is_consistent(&entry.path())? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn packaged_plugin_dir_is_consistent(plugin_dir: &Path) -> Result<bool, String> {
    let manifest_path = plugin_dir.join("plugin.toml");
    if !manifest_path.is_file() {
        return Ok(false);
    }
    let manifest = fs::read_to_string(&manifest_path).map_err(|error| {
        format!(
            "failed to read packaged plugin manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    let mut in_artifacts = false;
    let mut saw_artifact = false;
    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_artifacts = line == "[artifacts]";
            continue;
        }
        if !in_artifacts {
            continue;
        }
        let Some((_, value)) = line.split_once('=') else {
            continue;
        };
        let artifact = value.trim().trim_matches('"');
        if artifact.is_empty() {
            return Ok(false);
        }
        saw_artifact = true;
        if !plugin_dir.join(artifact).is_file() {
            return Ok(false);
        }
    }
    Ok(saw_artifact)
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
        LINUX_EDQUOT, LINUX_ENOSPC, PACKAGED_PLUGIN_TEST_HARNESS_BUILDS,
        PACKAGED_PLUGIN_TEST_VARIANT_BUILDS, PackagedPluginFailureInjectionPoint,
        PackagedPluginHarness, PackagedPluginInjectedFailureKind, PackagedPluginKind,
        PackagedPluginOperationError, PackagedPluginRecoveryContext, PackagedPluginRecoveryMode,
        clear_packaged_plugin_injected_failure, copy_packaged_plugin_tree,
        dynamic_library_filename, packaged_plugin_test_error_is_quota_like,
        packaged_plugin_test_output_is_quota_like, packaged_plugin_test_recover_from_quota_failure,
        plugin_manifest_contents, run_packaged_plugin_operation_with_quota_recovery,
        set_packaged_plugin_injected_failure,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;

    struct InjectedFailureGuard;

    impl Drop for InjectedFailureGuard {
        fn drop(&mut self) {
            clear_packaged_plugin_injected_failure();
        }
    }

    fn tests_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    fn inject_failure(
        operation: PackagedPluginFailureInjectionPoint,
        kind: PackagedPluginInjectedFailureKind,
    ) -> InjectedFailureGuard {
        set_packaged_plugin_injected_failure(operation, kind);
        InjectedFailureGuard
    }

    fn write_fake_packaged_plugin(
        dist_dir: &Path,
        plugin_id: &str,
        build_tag: &str,
    ) -> Result<(), PackagedPluginOperationError> {
        let plugin_dir = dist_dir.join(plugin_id);
        fs::create_dir_all(&plugin_dir).map_err(|error| {
            PackagedPluginOperationError::from_io(
                format!(
                    "failed to create fake packaged plugin dir {}",
                    plugin_dir.display()
                ),
                error,
            )
        })?;
        let artifact = format!("lib{plugin_id}-{build_tag}.so");
        fs::write(plugin_dir.join(&artifact), "fake-artifact").map_err(|error| {
            PackagedPluginOperationError::from_io(
                format!(
                    "failed to write fake packaged plugin artifact {}",
                    plugin_dir.join(&artifact).display()
                ),
                error,
            )
        })?;
        fs::write(
            plugin_dir.join("plugin.toml"),
            plugin_manifest_contents(plugin_id, PackagedPluginKind::Protocol, &artifact),
        )
        .map_err(|error| {
            PackagedPluginOperationError::from_io(
                format!(
                    "failed to write fake packaged plugin manifest {}",
                    plugin_dir.join("plugin.toml").display()
                ),
                error,
            )
        })?;
        Ok(())
    }

    fn write_fake_variant_artifact(
        cargo_package: &str,
        target_dir: &Path,
    ) -> Result<PathBuf, PackagedPluginOperationError> {
        let source = target_dir
            .join("debug")
            .join(dynamic_library_filename(cargo_package));
        fs::create_dir_all(
            source
                .parent()
                .expect("fake variant artifact should have parent"),
        )
        .map_err(|error| {
            PackagedPluginOperationError::from_io(
                format!(
                    "failed to create fake variant artifact dir {}",
                    source.parent().expect("parent should exist").display()
                ),
                error,
            )
        })?;
        fs::write(&source, "fake-variant-artifact").map_err(|error| {
            PackagedPluginOperationError::from_io(
                format!("failed to write fake variant artifact {}", source.display()),
                error,
            )
        })?;
        Ok(source)
    }

    #[test]
    fn quota_classifier_matches_linux_io_errors() {
        assert!(packaged_plugin_test_error_is_quota_like(
            &std::io::Error::from_raw_os_error(LINUX_ENOSPC)
        ));
        assert!(packaged_plugin_test_error_is_quota_like(
            &std::io::Error::from_raw_os_error(LINUX_EDQUOT)
        ));
        assert!(!packaged_plugin_test_error_is_quota_like(
            &std::io::Error::other("permission denied")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn quota_classifier_matches_command_output_text() {
        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(1),
            stdout: Vec::new(),
            stderr: b"Disk quota exceeded".to_vec(),
        };
        assert!(packaged_plugin_test_output_is_quota_like(&output));
    }

    #[test]
    fn quota_recovery_cleanup_preserves_current_stamp_outputs() {
        let temp = tempdir().expect("temp dir should be created");
        let cache_root = temp.path().join("cache-root");
        let current_stamp = "current-stamp";
        let current_plugin = cache_root
            .join(current_stamp)
            .join("runtime")
            .join("plugins")
            .join("je-5");
        let stale_root = cache_root.join("stale-stamp");
        let scoped_target = cache_root.join("cargo-targets").join("scoped-reload");
        let shared_debug = cache_root
            .join("cargo-targets")
            .join("shared")
            .join("debug");
        fs::create_dir_all(&current_plugin).expect("current plugin dir should be created");
        fs::write(current_plugin.join("plugin.toml"), "current")
            .expect("current plugin manifest should be written");
        fs::create_dir_all(stale_root.join("runtime").join("plugins"))
            .expect("stale stamp root should be created");
        fs::create_dir_all(scoped_target.join("debug"))
            .expect("scoped target dir should be created");
        fs::create_dir_all(shared_debug.join("deps")).expect("shared deps dir should be created");
        fs::write(shared_debug.join("deps").join("artifact"), "deps")
            .expect("shared deps artifact should be written");

        packaged_plugin_test_recover_from_quota_failure(&PackagedPluginRecoveryContext {
            cache_root: cache_root.clone(),
            current_stamp: Some(current_stamp.to_string()),
            mode: PackagedPluginRecoveryMode::SharedHarnessBuild,
        })
        .expect("quota recovery cleanup should succeed");

        assert!(current_plugin.join("plugin.toml").is_file());
        assert!(!stale_root.exists());
        assert!(!scoped_target.exists());
        assert!(!shared_debug.join("deps").exists());
        assert!(cache_root.join("cargo-targets").join("shared").is_dir());
    }

    #[test]
    fn shared_harness_bootstrap_recovers_from_quota_failure() {
        let _guard = tests_lock()
            .lock()
            .expect("packaged plugin test lock should not be poisoned");
        let _failure = inject_failure(
            PackagedPluginFailureInjectionPoint::SharedHarnessPackageAllPlugins,
            PackagedPluginInjectedFailureKind::QuotaLike,
        );
        let temp = tempdir().expect("temp dir should be created");
        let cache_root = temp.path().join("cache-root");
        let attempts = AtomicUsize::new(0);

        let harness = PackagedPluginHarness::build_shared_with_packager_in_cache(
            &cache_root,
            "shared-recovery-stamp",
            |dist_dir, _target_dir, build_tag| {
                attempts.fetch_add(1, Ordering::SeqCst);
                super::maybe_inject_packaged_plugin_operation_failure(
                    PackagedPluginFailureInjectionPoint::SharedHarnessPackageAllPlugins,
                    "injected fake shared harness packaging failure",
                )?;
                write_fake_packaged_plugin(dist_dir, "je-5", build_tag)
            },
        )
        .expect("shared harness build should recover and succeed");

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(
            harness
                .dist_dir()
                .join("je-5")
                .join("plugin.toml")
                .is_file()
        );
        assert_eq!(harness.cargo_target_dir(), cache_root.join("cargo-targets"));
    }

    #[test]
    fn cached_variant_build_recovers_from_quota_failure() {
        let _guard = tests_lock()
            .lock()
            .expect("packaged plugin test lock should not be poisoned");
        let _failure = inject_failure(
            PackagedPluginFailureInjectionPoint::VariantCargoBuild,
            PackagedPluginInjectedFailureKind::QuotaLike,
        );
        let temp = tempdir().expect("temp dir should be created");
        let harness = PackagedPluginHarness {
            dist: temp.path().join("runtime").join("plugins"),
            artifact_cache: temp
                .path()
                .join("cache-root")
                .join("stamp")
                .join("artifacts"),
            cargo_target: temp.path().join("cache-root").join("cargo-targets"),
        };
        fs::create_dir_all(&harness.artifact_cache).expect("artifact cache dir should be created");
        let target_dir = harness.scoped_target_dir("variant-recovery");
        let attempts = AtomicUsize::new(0);

        let artifact = harness
            .build_cached_packaged_plugin_artifact_with_builder(
                "mc-plugin-proto-je-5",
                &target_dir,
                "variant-recovery-v1",
                |cargo_package, target_dir, _build_tag| {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    super::maybe_inject_packaged_plugin_operation_failure(
                        PackagedPluginFailureInjectionPoint::VariantCargoBuild,
                        format!("injected fake cargo build failure for `{cargo_package}`"),
                    )?;
                    write_fake_variant_artifact(cargo_package, target_dir)
                },
            )
            .expect("cached variant build should recover and succeed");

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(artifact.is_file());
        assert!(target_dir.join("debug").is_dir());
    }

    #[test]
    fn non_quota_failures_do_not_trigger_cleanup_or_retry() {
        let _guard = tests_lock()
            .lock()
            .expect("packaged plugin test lock should not be poisoned");
        let _failure = inject_failure(
            PackagedPluginFailureInjectionPoint::SharedHarnessPackageAllPlugins,
            PackagedPluginInjectedFailureKind::Other,
        );
        let temp = tempdir().expect("temp dir should be created");
        let cache_root = temp.path().join("cache-root");
        let stale_root = cache_root.join("stale-stamp");
        fs::create_dir_all(stale_root.join("runtime").join("plugins"))
            .expect("stale root should be created");
        let attempts = AtomicUsize::new(0);

        let result = PackagedPluginHarness::build_shared_with_packager_in_cache(
            &cache_root,
            "current-stamp",
            |dist_dir, _target_dir, build_tag| {
                attempts.fetch_add(1, Ordering::SeqCst);
                super::maybe_inject_packaged_plugin_operation_failure(
                    PackagedPluginFailureInjectionPoint::SharedHarnessPackageAllPlugins,
                    "injected non-quota shared bootstrap failure",
                )?;
                write_fake_packaged_plugin(dist_dir, "je-5", build_tag)
            },
        );

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(stale_root.exists());
    }

    #[test]
    fn direct_non_quota_failures_do_not_trigger_wrapper_retry() {
        let _guard = tests_lock()
            .lock()
            .expect("packaged plugin test lock should not be poisoned");
        let temp = tempdir().expect("temp dir should be created");
        let cache_root = temp.path().join("cache-root");
        let stale_root = cache_root.join("stale-stamp");
        fs::create_dir_all(&stale_root).expect("stale root should be created");
        let attempts = AtomicUsize::new(0);

        let result: Result<(), PackagedPluginOperationError> =
            run_packaged_plugin_operation_with_quota_recovery(
                PackagedPluginRecoveryContext {
                    cache_root: cache_root.clone(),
                    current_stamp: Some("current-stamp".to_string()),
                    mode: PackagedPluginRecoveryMode::Generic,
                },
                || {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err(PackagedPluginOperationError::from_message(
                        "non quota failure",
                    ))
                },
            );

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(stale_root.exists());
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
            .seed_subset(&dist_dir, &["je-5", "auth-offline"])
            .expect("subset seed should succeed");

        assert!(dist_dir.join("je-5").is_dir());
        assert!(dist_dir.join("auth-offline").is_dir());
        assert!(!dist_dir.join("je-47").exists());
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
            .join("mc-plugin-proto-je-5")
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
                "mc-plugin-proto-je-5",
                "je-5",
                &first_temp_dir.path().join("runtime").join("plugins"),
                &target_dir,
                "cache-hit-v1",
            )
            .expect("first cached install should succeed");
        let after_first =
            PACKAGED_PLUGIN_TEST_VARIANT_BUILDS.load(std::sync::atomic::Ordering::SeqCst);

        harness
            .install_protocol_plugin(
                "mc-plugin-proto-je-5",
                "je-5",
                &second_temp_dir.path().join("runtime").join("plugins"),
                &target_dir,
                "cache-hit-v1",
            )
            .expect("second cached install should succeed");
        let after_second =
            PACKAGED_PLUGIN_TEST_VARIANT_BUILDS.load(std::sync::atomic::Ordering::SeqCst);

        let cached_artifact = cache_dir.join(dynamic_library_filename("mc-plugin-proto-je-5"));
        assert!(cached_artifact.is_file());
        assert_eq!(after_first, before + 1);
        assert_eq!(after_second, after_first);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn packaged_plugin_harness_keeps_shared_cargo_target_dir_after_build() {
        let _guard = tests_lock()
            .lock()
            .expect("packaged plugin test lock should not be poisoned");
        let harness = PackagedPluginHarness::shared().expect("harness should load");
        assert_eq!(
            harness.cargo_target_dir(),
            super::packaged_plugin_test_cargo_target_root()
        );
        assert!(
            harness.cargo_target_dir().join("shared").is_dir(),
            "shared packaged-plugin target dir should persist for incremental reuse"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn packaged_plugin_direct_cached_lookup_skips_variant_rebuild_counter() {
        let _guard = tests_lock()
            .lock()
            .expect("packaged plugin test lock should not be poisoned");
        let harness = PackagedPluginHarness::shared().expect("harness should load");
        let cache_dir = harness
            .artifact_cache_dir()
            .join("mc-plugin-proto-je-5")
            .join("direct-cache-v1");
        if cache_dir.exists() {
            fs::remove_dir_all(&cache_dir).expect("cache dir should be removable");
        }

        let before = PACKAGED_PLUGIN_TEST_VARIANT_BUILDS.load(std::sync::atomic::Ordering::SeqCst);
        let target_dir = harness.scoped_target_dir("direct-cache");
        let first = harness
            .build_cached_packaged_plugin_artifact(
                "mc-plugin-proto-je-5",
                &target_dir,
                "direct-cache-v1",
            )
            .expect("first direct build should succeed");
        let after_first =
            PACKAGED_PLUGIN_TEST_VARIANT_BUILDS.load(std::sync::atomic::Ordering::SeqCst);
        let second = harness
            .build_cached_packaged_plugin_artifact(
                "mc-plugin-proto-je-5",
                &target_dir,
                "direct-cache-v1",
            )
            .expect("second direct build should hit the cache");
        let after_second =
            PACKAGED_PLUGIN_TEST_VARIANT_BUILDS.load(std::sync::atomic::Ordering::SeqCst);

        assert!(first.is_file());
        assert!(
            target_dir
                .join("debug")
                .join(dynamic_library_filename("mc-plugin-proto-je-5"))
                .is_file(),
            "direct builds should place intermediates under the requested target dir"
        );
        assert_eq!(second, first);
        assert_eq!(after_first, before + 1);
        assert_eq!(after_second, after_first);
    }

    #[test]
    fn scoped_target_dir_separates_build_scopes() {
        let harness = PackagedPluginHarness {
            dist: PathBuf::from("dist"),
            artifact_cache: PathBuf::from("cache"),
            cargo_target: PathBuf::from("target-root"),
        };

        assert_eq!(
            harness.scoped_target_dir("variant-cache"),
            PathBuf::from("target-root").join("variant-cache")
        );
        assert_eq!(
            harness.scoped_target_dir("variant/cache"),
            PathBuf::from("target-root").join("variant_cache")
        );
    }

    #[test]
    fn seed_copy_keeps_plugin_manifest_detached_from_source_tree() {
        let source_root = tempdir().expect("source temp dir should be created");
        let destination_root = tempdir().expect("destination temp dir should be created");
        let source_plugin = source_root.path().join("je-5");
        let destination_plugin = destination_root.path().join("je-5");
        fs::create_dir_all(&source_plugin).expect("source plugin dir should be created");
        fs::write(
            source_plugin.join("plugin.toml"),
            "[plugin]\nid = \"je-5\"\nkind = \"protocol\"\n\n[artifacts]\n\"linux-x86_64\" = \"libsource.so\"\n",
        )
        .expect("source manifest should be written");
        fs::write(source_plugin.join("libsource.so"), "artifact")
            .expect("source artifact should be written");

        copy_packaged_plugin_tree(&source_plugin, &destination_plugin)
            .expect("plugin tree should copy");
        fs::write(
            destination_plugin.join("plugin.toml"),
            "[plugin]\nid = \"je-5\"\nkind = \"protocol\"\n\n[artifacts]\n\"linux-x86_64\" = \"libdestination.so\"\n",
        )
        .expect("destination manifest should be overwritten");

        assert!(
            fs::read_to_string(source_plugin.join("plugin.toml"))
                .expect("source manifest should remain readable")
                .contains("libsource.so"),
            "source manifest should stay unchanged when the seeded copy is edited"
        );
    }

    fn tempdir() -> std::io::Result<tempfile::TempDir> {
        let base_dir = super::packaged_plugin_test_workspace_root()
            .join("target")
            .join("test-tmp")
            .join("mc-plugin-test-support");
        fs::create_dir_all(&base_dir)?;
        tempfile::Builder::new()
            .prefix("mc-plugin-test-support-")
            .tempdir_in(base_dir)
    }
}
